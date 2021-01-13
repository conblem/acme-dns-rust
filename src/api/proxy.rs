use futures_util::future::{ready, BoxFuture, FutureExt};
use ppp::error::ParseError;
use ppp::model::Addresses;
use std::future::Future;
use std::io::{Cursor, IoSlice, Write};
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, Error as IoError, ErrorKind, ReadBuf, Result as IoResult};
use tokio::net::TcpStream;
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};

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
}

impl<'a> Future for PeerAddrFuture<'a> {
    type Output = IoResult<SocketAddr>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut self.get_mut().stream;
        // add option again to make impossible to pull future later
        let data = match &mut this.data {
            Some(ref mut data) => data,
            None => unreachable!("Future cannot be polled anymore"),
        };

        let mut buf = [MaybeUninit::<u8>::uninit(); 256];
        let mut buf = ReadBuf::uninit(&mut buf);

        let stream = Pin::new(&mut this.stream);
        let buf = match stream.poll_read(cx, &mut buf) {
            Poll::Ready(Ok(_)) => buf.filled_mut(),
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        };

        if let Err(e) = data.write_all(buf) {
            return Poll::Ready(Err(e));
        }

        let data = data.get_ref();
        let res = match ppp::parse_header(data) {
            Err(ParseError::Incomplete) => return Poll::Pending,
            Err(ParseError::Failure) => {
                return Poll::Ready(Err(IoError::new(
                    ErrorKind::InvalidData,
                    "Proxy Parser Error",
                )))
            }
            Ok((remaining, res)) => {
                this.start_of_data = data.len() - remaining.len();
                res
            }
        };

        let addr = match res.addresses {
            Addresses::IPv4 { source_address, source_port , .. } => {
                let port = source_port.unwrap_or_default();
                SocketAddrV4::new(source_address.into(), port).into()
            },
            Addresses::IPv6 { source_address, source_port ,.. } => {
                let port = source_port.unwrap_or_default();
                SocketAddrV6::new(source_address.into(), port, 0, 0).into()
            },
            address => {
                return Poll::Ready(Err(IoError::new(
                    ErrorKind::Other,
                    format!("Cannot convert {:?} to a SocketAddr", address),
                )))
            }
        };

        Poll::Ready(Ok(addr))
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
        // handle the case were the full data has no space in the first place
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
