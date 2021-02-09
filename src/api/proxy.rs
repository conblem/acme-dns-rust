use futures_util::stream::Stream;
use futures_util::{ready, TryStreamExt};
use ppp::error::ParseError;
use ppp::model::{Addresses, Header};
use std::future::Future;
use std::io::IoSlice;
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::ops::{Deref, DerefMut};
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

pub(super) fn wrap(
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
                match conn.real_addr().await {
                    Ok(Some(addr)) => {
                        span.record("remote.real", &display(addr));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        error!("Could net get remote.addr: {}", e);
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
            ProxyProtocol::Enabled => Some(Default::default()),
            ProxyProtocol::Disabled => None,
        };
        ProxyStream {
            stream: self,
            data,
            start_of_data: 0,
        }
    }
}

pub(super) struct ProxyStream<T> {
    stream: T,
    data: Option<Vec<u8>>,
    start_of_data: usize,
}

impl<T> ProxyStream<T>
where
    T: AsyncRead + Unpin,
{
    fn real_addr(&mut self) -> RealAddrFuture<'_, T> {
        RealAddrFuture { proxy_stream: self }
    }
}

impl<T> AsyncRead for ProxyStream<T>
where
    T: AsyncRead + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<IoResult<()>> {
        let this = self.get_mut();
        if let Some(data) = this.data.take() {
            buf.put_slice(&data[this.start_of_data..])
        }
        Pin::new(&mut this.stream).poll_read(cx, buf)
    }
}

impl<T> AsyncWrite for ProxyStream<T>
where
    T: AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<IoResult<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<IoResult<usize>> {
        Pin::new(&mut self.stream).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
}

impl<T> Deref for ProxyStream<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.stream
    }
}

impl<T> DerefMut for ProxyStream<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.stream
    }
}

struct RealAddrFuture<'a, T> {
    proxy_stream: &'a mut ProxyStream<T>,
}

impl<'a, T> RealAddrFuture<'a, T> {
    fn format_header(&self, res: Header) -> IoResult<Option<SocketAddr>> {
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

        Ok(Some(addr))
    }

    fn get_header(&mut self) -> Poll<IoResult<Option<SocketAddr>>> {
        let data = match &mut self.proxy_stream.data {
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
                self.proxy_stream.start_of_data = data.len() - remaining.len();
                res
            }
        };

        Poll::Ready(self.format_header(res))
    }
}

impl<T> Future for RealAddrFuture<'_, T>
where
    T: AsyncRead + Unpin,
{
    type Output = IoResult<Option<SocketAddr>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let stream = Pin::new(&mut this.proxy_stream.stream);

        let data = match &mut this.proxy_stream.data {
            Some(data) => data,
            None => return Poll::Ready(Ok(None)),
        };

        match ready!(poll_read_buf(stream, cx, data)) {
            Ok(0) => {
                return Poll::Ready(Err(IoError::new(
                    ErrorKind::UnexpectedEof,
                    "Streamed finished before end of proxy protocol header",
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
    use ppp::model::{Addresses, Command, Header, Protocol, Version};
    use std::io::Cursor;

    use super::ToProxyStream;
    use crate::config::ProxyProtocol;
    use std::net::SocketAddr;

    #[tokio::test]
    async fn test_header_parsing() {
        let header = Header::new(
            Version::Two,
            Command::Proxy,
            Protocol::Stream,
            vec![],
            Addresses::from(([1, 1, 1, 1], [2, 2, 2, 2], 24034, 443)),
        );
        let header = ppp::to_bytes(header).unwrap();
        let mut header = Cursor::new(header).source(ProxyProtocol::Enabled);

        let actual = header.real_addr().await.unwrap().unwrap();

        assert_eq!(SocketAddr::from(([1, 1, 1, 1], 24034)), actual);
    }
}
