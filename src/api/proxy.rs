use futures_util::stream::Stream;
use futures_util::{ready, TryStreamExt};
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
use tracing::field::{display, Empty};
use tracing::{debug_span, error, info, Instrument, Span};

use crate::config::ProxyProtocol;

pub(super) fn wrap(
    listener: TcpListener,
    proxy: ProxyProtocol,
) -> impl Stream<Item = IoResult<impl Future<Output = IoResult<ProxyStream>>>> + Send {
    TcpListenerStream::new(listener)
        .map_ok(move |stream| {
            let span = Span::current();
            span.record_all()
            stream.source(proxy)
        })
        .map_ok(|mut conn| {
            let span = debug_span!("ADDR", remote.addr = Empty);
            let span_clone = span.clone();
            async move {
                match conn.proxy_peer().await {
                    Ok(addr) => {
                        span.record("remote.addr", &display(addr));
                        info!("Got addr {}", addr)
                    }
                    Err(e) => {
                        span.record("remote.addr", &"Unknown");
                        error!("Could net get remote.addr: {}", e);
                    }
                }
                Ok(conn)
            }
            .instrument(span_clone)
        })
}

pub(super) trait ToProxyStream {
    fn source(self, proxy: ProxyProtocol) -> ProxyStream;
}

impl ToProxyStream for TcpStream {
    fn source(self, proxy: ProxyProtocol) -> ProxyStream {
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

pub(super) struct ProxyStream {
    stream: TcpStream,
    data: Option<Vec<u8>>,
    start_of_data: usize,
}

impl ProxyStream {
    fn proxy_peer(&mut self) -> PeerAddrFuture<'_> {
        PeerAddrFuture { proxy_stream: self }
    }
}

impl AsyncRead for ProxyStream {
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

impl AsyncWrite for ProxyStream {
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

struct PeerAddrFuture<'a> {
    proxy_stream: &'a mut ProxyStream,
}

impl<'a> PeerAddrFuture<'a> {
    fn format_header(&self, res: Header) -> Poll<<Self as Future>::Output> {
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
                return Poll::Ready(Err(IoError::new(
                    ErrorKind::Other,
                    format!("Cannot convert {:?} to a SocketAddr", address),
                )))
            }
        };

        Poll::Ready(Ok(addr))
    }

    fn get_header(&mut self) -> Poll<<Self as Future>::Output> {
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

        self.format_header(res)
    }
}

impl Future for PeerAddrFuture<'_> {
    type Output = IoResult<SocketAddr>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let stream = Pin::new(&mut this.proxy_stream.stream);

        let data = match &mut this.proxy_stream.data {
            Some(data) => data,
            None => return Poll::Ready(stream.local_addr()),
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
