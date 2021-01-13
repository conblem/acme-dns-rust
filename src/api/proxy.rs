use futures_util::future::{ready, BoxFuture, FutureExt};
use std::future::Future;
use std::mem::MaybeUninit;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, Result as IoResult};
use tokio::net::TcpStream;

pub(super) trait PeerAddr<E: std::error::Error> {
    fn proxy_peer<'a>(&'a mut self) -> BoxFuture<'a, Result<SocketAddr, E>>;
}

struct PeerAddrFuture<'a> {
    stream: Pin<&'a mut TcpStream>,
    data: Vec<u8>,
}

impl<'a> PeerAddrFuture<'a> {
    fn new(stream: &'a mut TcpStream) -> Self {
        PeerAddrFuture {
            stream: Pin::new(stream),
            data: vec![],
        }
    }
}

impl<'a> Future for PeerAddrFuture<'a> {
    type Output = Result<SocketAddr, tokio::io::Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let peer_addr_future: &mut Self = self.get_mut();
        let stream = peer_addr_future.stream.as_mut();
        let mut buf = [MaybeUninit::<u8>::uninit(); 1024];
        let mut buf = ReadBuf::uninit(&mut buf);

        let data = match stream.poll_read(cx, &mut buf) {
            Poll::Ready(Ok(_)) => buf.filled_mut(),
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        };

        peer_addr_future.data.copy_from_slice(data);

        Poll::Pending
    }
}

impl PeerAddr<tokio::io::Error> for TcpStream {
    fn proxy_peer(&mut self) -> BoxFuture<Result<SocketAddr, tokio::io::Error>> {
        ready(self.peer_addr()).boxed()
    }
}

pub(super) struct ProxyStream {
    stream: TcpStream,
}

impl From<TcpStream> for ProxyStream {
    fn from(stream: TcpStream) -> Self {
        ProxyStream { stream }
    }
}

impl PeerAddr<tokio::io::Error> for ProxyStream {
    fn proxy_peer(&mut self) -> BoxFuture<Result<SocketAddr, tokio::io::Error>> {
        PeerAddrFuture::new(&mut self.stream).boxed()
    }
}

impl AsyncRead for ProxyStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<IoResult<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for ProxyStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, tokio::io::Error>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
