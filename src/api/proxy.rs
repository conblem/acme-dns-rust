use futures_util::future::{ready, BoxFuture, FutureExt};
use futures_util::stream::{Stream, TryStream};
use futures_util::TryStreamExt;
use ppp::error::ParseError;
use ppp::model::{Addresses, Header};
use std::future::Future;
use std::io::{Cursor, IoSlice, Write};
use std::mem::MaybeUninit;
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::ops::DerefMut;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, Error as IoError, ErrorKind, ReadBuf, Result as IoResult};
use tokio::net::TcpStream;
use tracing::field::{display, Empty};
use tracing::{debug_span, error, info, Instrument, Span};

pub(super) fn wrap<S, O, E, P>(
    stream: S,
) -> impl Stream<Item = Result<impl Future<Output = Result<impl AsyncRead + AsyncWrite, E>>, E>>
where
    P: std::error::Error,
    S: TryStream<Ok = O, Error = E>,
    O: PeerAddr<P>,
{
    stream.map_ok(|mut conn| {
        let span = debug_span!("ADDR", remote.addr = Empty);
        async move {
            let span = Span::current();
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
        .instrument(span)
    })
}

pub(super) trait PeerAddr<E: std::error::Error>: AsyncRead + AsyncWrite {
    fn proxy_peer<'a>(&'a mut self) -> BoxFuture<'a, Result<SocketAddr, E>>;
}

impl PeerAddr<tokio::io::Error> for TcpStream {
    fn proxy_peer(&mut self) -> BoxFuture<IoResult<SocketAddr>> {
        ready(self.peer_addr()).boxed()
    }
}

struct PeerAddrFuture<'a> {
    stream: &'a mut ProxyStream,
}

impl<'a> PeerAddrFuture<'a> {
    fn new(stream: &'a mut ProxyStream) -> Self {
        PeerAddrFuture { stream }
    }

    fn parse_header(&mut self) -> Poll<IoResult<Header>> {
        let data = match self.stream.data {
            Some(ref mut data) => data.get_ref(),
            None => unreachable!("Future cannot be pulled anymore"),
        };

        match ppp::parse_header(data) {
            Err(ParseError::Incomplete) => Poll::Pending,
            Err(ParseError::Failure) => Poll::Ready(Err(IoError::new(
                ErrorKind::InvalidData,
                "Proxy Parser Error",
            ))),
            Ok((remaining, res)) => {
                self.stream.start_of_data = data.len() - remaining.len();
                Poll::Ready(Ok(res))
            }
        }
    }

    fn format_header(res: Header) -> Poll<<Self as Future>::Output> {
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
        let res = match self.parse_header() {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(res)) => res,
        };

        PeerAddrFuture::format_header(res)
    }
}

impl<'a> Future for PeerAddrFuture<'a> {
    type Output = IoResult<SocketAddr>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        let data = match &mut this.stream.data {
            Some(ref mut data) => data,
            None => unreachable!("Future cannot be polled anymore"),
        };

        let mut buf = [MaybeUninit::<u8>::uninit(); 256];
        let mut buf = ReadBuf::uninit(&mut buf);

        let stream = Pin::new(&mut this.stream.stream);
        let buf = match stream.poll_read(cx, &mut buf) {
            Poll::Ready(Ok(_)) => buf.filled(),
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        };

        if let Err(e) = data.write_all(buf) {
            return Poll::Ready(Err(e));
        }

        this.get_header()
    }
}

pub(super) struct ProxyStream {
    stream: TcpStream,
    data: Option<Cursor<Vec<u8>>>,
    start_of_data: usize,
}

impl From<TcpStream> for ProxyStream {
    fn from(stream: TcpStream) -> Self {
        ProxyStream {
            stream,
            data: Some(Default::default()),
            start_of_data: 0,
        }
    }
}

impl PeerAddr<tokio::io::Error> for ProxyStream {
    fn proxy_peer(&mut self) -> BoxFuture<IoResult<SocketAddr>> {
        PeerAddrFuture::new(self).boxed()
    }
}

impl AsyncRead for ProxyStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<IoResult<()>> {
        let this = self.get_mut();
        // todo: handle the case were the data has no space in buf
        if let Some(data) = this.data.take() {
            buf.put_slice(&data.get_ref()[this.start_of_data..])
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

impl<E: std::error::Error> PeerAddr<E> for Box<(dyn PeerAddr<E> + Send + Unpin + 'static)> {
    fn proxy_peer<'a>(&'a mut self) -> BoxFuture<'a, Result<SocketAddr, E>> {
        self.deref_mut().proxy_peer()
    }
}
