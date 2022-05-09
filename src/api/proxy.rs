use futures_util::stream::Stream;
use futures_util::{ready, TryStreamExt};
use pin_project_lite::pin_project;
use ppp::error::ParseError;
use ppp::model::{Addresses, Header};
use std::future::Future;
use std::io::IoSlice;
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, Error as IoError, ErrorKind, ReadBuf, Result as IoResult};
use tokio::net::{TcpListener, TcpStream};
use tokio_stream::wrappers::TcpListenerStream;
use tokio_util::io::poll_read_buf;
use tracing::field::{debug, display};
use tracing::{error, Instrument, Span};

use crate::config::ProxyProtocol;

trait RemoteAddr {
    fn remote_addr(&self) -> IoResult<SocketAddr>;
}

impl RemoteAddr for TcpStream {
    fn remote_addr(&self) -> IoResult<SocketAddr> {
        self.peer_addr()
    }
}

impl<T: RemoteAddr> RemoteAddr for ProxyStream<T> {
    fn remote_addr(&self) -> IoResult<SocketAddr> {
        self.stream.remote_addr()
    }
}

pub(crate) fn wrap(
    listener: TcpListener,
    proxy: ProxyProtocol,
) -> impl Stream<
    Item = IoResult<
        impl Future<Output = IoResult<impl AsyncRead + AsyncWrite + Send + Unpin + 'static>>,
    >,
> + Send {
    TcpListenerStream::new(listener)
        .map_ok(move |conn| conn.source(proxy))
        .map_ok(|mut conn| {
            let span = Span::current();
            span.record("remote.addr", &debug(conn.remote_addr()));
            let span_clone = span.clone();

            async move {
                match Pin::new(&mut conn).real_addr().await {
                    Ok(Some(addr)) => {
                        span.record("remote.real", &display(addr));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        error!("Could net get remote.real: {}", e);
                    }
                }
                Ok(conn)
            }
            .instrument(span_clone)
        })
}

trait ToProxyStream: Sized {
    fn source(self, proxy: ProxyProtocol) -> ProxyStream<Self>;
}

impl<T> ToProxyStream for T
where
    T: AsyncRead + Unpin,
{
    fn source(self, proxy: ProxyProtocol) -> ProxyStream<T> {
        let data = match proxy {
            ProxyProtocol::Enabled => Some(Vec::with_capacity(256)),
            ProxyProtocol::Disabled => None,
        };
        ProxyStream {
            stream: self,
            data,
            start_of_data: 0,
        }
    }
}

pin_project! {
    struct ProxyStream<T> {
        #[pin]
        stream: T,
        data: Option<Vec<u8>>,
        start_of_data: usize,
    }
}

impl<T> ProxyStream<T>
where
    T: AsyncRead,
{
    async fn real_addr(self: Pin<&mut Self>) -> IoResult<Option<SocketAddr>> {
        RealAddrFuture { proxy_stream: self }.await
    }
}

impl<T> AsyncRead for ProxyStream<T>
where
    T: AsyncRead,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<IoResult<()>> {
        let this = self.project();
        if let Some(data) = this.data.take() {
            buf.put_slice(&data[*this.start_of_data..])
        }
        this.stream.poll_read(cx, buf)
    }
}

impl<T> AsyncWrite for ProxyStream<T>
where
    T: AsyncWrite,
{
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<IoResult<usize>> {
        self.project().stream.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        self.project().stream.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        self.project().stream.poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<IoResult<usize>> {
        self.project().stream.poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
}

struct RealAddrFuture<'a, T> {
    proxy_stream: Pin<&'a mut ProxyStream<T>>,
}

impl<'a, T> RealAddrFuture<'a, T> {
    fn get_header(&mut self) -> Poll<IoResult<Option<SocketAddr>>> {
        let proxy_stream = self.proxy_stream.as_mut().project();
        let data = match proxy_stream.data {
            Some(data) => data,
            None => unreachable!("Future cannot be pulled anymore"),
        };

        let res = match ppp::parse_header(data) {
            Err(ParseError::Incomplete) => return Poll::Pending,
            Err(ParseError::Failure) => {
                return Poll::Ready(Err(IoError::new(
                    ErrorKind::InvalidData,
                    "Proxy Parser Error",
                )))
            }
            Ok((remaining, res)) => {
                *proxy_stream.start_of_data = data.len() - remaining.len();
                res
            }
        };

        Poll::Ready(Self::format_header(res).map(Some))
    }

    fn format_header(res: Header) -> IoResult<SocketAddr> {
        let addr = match res.addresses {
            Addresses::IPv4 {
                source_address,
                source_port,
                ..
            } => {
                let port = source_port.unwrap_or_default();
                SocketAddrV4::new(source_address.into(), port).into()
            }
            Addresses::IPv6 {
                source_address,
                source_port,
                ..
            } => {
                let port = source_port.unwrap_or_default();
                SocketAddrV6::new(source_address.into(), port, 0, 0).into()
            }
            address => {
                return Err(IoError::new(
                    ErrorKind::Other,
                    format!("Cannot convert {:?} to a SocketAddr", address),
                ))
            }
        };

        Ok(addr)
    }
}

impl<T> Future for RealAddrFuture<'_, T>
where
    T: AsyncRead,
{
    type Output = IoResult<Option<SocketAddr>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let proxy_stream = this.proxy_stream.as_mut().project();

        let data = match proxy_stream.data {
            Some(data) => data,
            None => return Poll::Ready(Ok(None)),
        };

        match ready!(poll_read_buf(proxy_stream.stream, cx, data)) {
            Ok(0) => {
                return Poll::Ready(Err(IoError::new(
                    ErrorKind::UnexpectedEof,
                    "Stream finished before end of proxy protocol header",
                )))
            }
            Ok(_) => {}
            Err(e) => return Poll::Ready(Err(e)),
        };

        this.get_header()
    }
}

#[cfg(test)]
mod tests {
    use crate::api::proxy::RealAddrFuture;
    use futures_util::future;
    use ppp::model::{Addresses, Command, Header, Protocol, Version};
    use std::io::{Error as IoError, ErrorKind, IoSlice, Result as IoResult};
    use std::net::SocketAddr;
    use std::pin::Pin;
    use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio_test::io::Builder;

    use super::{RemoteAddr, ToProxyStream};
    use crate::config::ProxyProtocol;

    #[tokio::test]
    async fn test_disabled() {
        let mut proxy_stream = Builder::new().build().source(ProxyProtocol::Disabled);
        let proxy_stream = Pin::new(&mut proxy_stream);

        assert!(proxy_stream.real_addr().await.unwrap().is_none());
    }

    fn generate_header(addresses: Addresses) -> Header {
        Header::new(
            Version::Two,
            Command::Proxy,
            Protocol::Stream,
            vec![],
            addresses,
        )
    }

    fn generate_ipv4() -> Header {
        let adresses = Addresses::from(([1, 1, 1, 1], [2, 2, 2, 2], 24034, 443));
        generate_header(adresses)
    }

    #[tokio::test]
    async fn test_header_parsing() {
        let mut header = ppp::to_bytes(generate_ipv4()).unwrap();
        header.extend_from_slice("Test".as_ref());

        let mut proxy_stream = header.source(ProxyProtocol::Enabled);
        let mut proxy_stream = Pin::new(&mut proxy_stream);

        let actual = proxy_stream.as_mut().real_addr().await.unwrap().unwrap();

        assert_eq!(SocketAddr::from(([1, 1, 1, 1], 24034)), actual);

        let mut actual = String::new();
        let size = proxy_stream.read_to_string(&mut actual).await.unwrap();
        assert_eq!(4, size);
        assert_eq!("Test", actual);
    }

    #[tokio::test]
    #[ignore]
    async fn test_incomplete() {
        let header = ppp::to_bytes(generate_ipv4()).unwrap();

        let mut header = header[..10].source(ProxyProtocol::Enabled);
        let header = Pin::new(&mut header);
        let actual = header.real_addr().await.unwrap_err();

        assert_eq!(
            format!("{}", actual),
            "Stream finished before end of proxy protocol header"
        );
    }

    #[tokio::test]
    async fn test_failure() {
        let invalid = Vec::from("invalid header");
        let mut invalid = invalid.source(ProxyProtocol::Enabled);
        let invalid = Pin::new(&mut invalid);

        let actual = invalid.real_addr().await.unwrap_err();
        assert_eq!(format!("{}", actual), "Proxy Parser Error");
    }

    #[tokio::test]
    #[ignore]
    async fn test_io_error() {
        // builder needs to be dropped before stream can be used
        // otherwise the internal tokio arc error has 2 strong references
        let mut proxy_stream = {
            let header = ppp::to_bytes(generate_ipv4()).unwrap();
            let mut builder = Builder::new();
            builder.read(&header[..10]);
            builder.read_error(IoError::new(ErrorKind::Other, "Error on IO"));
            builder.build().source(ProxyProtocol::Enabled)
        };
        let proxy_stream = Pin::new(&mut proxy_stream);

        let error = proxy_stream.real_addr().await.unwrap_err();
        assert_eq!("Error on IO", format!("{}", error));
    }

    #[test]
    fn test_addresses() {
        let address = [1, 1, 1, 1, 1, 1, 1, 1];
        let addresses = Addresses::from((address, address, 24034, 443));

        let actual = RealAddrFuture::<()>::format_header(generate_header(addresses)).unwrap();
        assert_eq!(SocketAddr::from((address, 24034)), actual);

        let address = [
            1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        ];
        let addresses = Addresses::from((address, address));

        assert!(RealAddrFuture::<()>::format_header(generate_header(addresses)).is_err());
    }

    #[test]
    fn test_remote_addr_delegation() {
        impl RemoteAddr for &[u8] {
            fn remote_addr(&self) -> IoResult<SocketAddr> {
                Ok(SocketAddr::from(([1, 1, 1, 1], 443)))
            }
        }

        let proxy_stream = &mut &[].source(ProxyProtocol::Enabled);
        let actual = proxy_stream.remote_addr().unwrap();
        assert_eq!(SocketAddr::from(([1, 1, 1, 1], 443)), actual)
    }

    #[tokio::test]
    async fn test_async_write_delegation() {
        let mut builder = Builder::new();
        builder.write("Test1".as_ref());
        builder.write("Test2".as_ref());

        let mut proxy_stream = builder.build().source(ProxyProtocol::Disabled);
        assert_eq!(false, proxy_stream.is_write_vectored());

        proxy_stream.write_all("Test1".as_ref()).await.unwrap();

        let slice = IoSlice::new("Test2".as_ref());
        let size = future::poll_fn(move |cx| {
            Pin::new(&mut proxy_stream).poll_write_vectored(cx, &[slice])
        })
        .await
        .unwrap();
        assert_eq!(5, size);

        let mut proxy_stream = Builder::new().build().source(ProxyProtocol::Disabled);
        assert_eq!((), proxy_stream.flush().await.unwrap());
        assert_eq!((), proxy_stream.shutdown().await.unwrap());
    }
}
