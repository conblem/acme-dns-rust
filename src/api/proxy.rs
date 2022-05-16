use futures_util::stream::Stream;
use futures_util::{ready, TryStreamExt};
use pin_project_lite::pin_project;
use ppp::error::ParseError;
use ppp::model::{Addresses, Header};
use ppp_stream::Ext;
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

pub(crate) fn wrap(
    listener: TcpListener,
    proxy: ProxyProtocol,
) -> impl Stream<
    Item = IoResult<
        impl Future<Output = IoResult<impl AsyncRead + AsyncWrite + Send + Unpin + 'static>>,
    >,
> + Send {
    TcpListenerStream::new(listener).map_ok(|mut conn| {
        let span = Span::current();
        span.record("remote.addr", &debug(conn.peer_addr()));
        let span_clone = span.clone();

        async move {
            match conn.remote_addr_unpin().await {
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
