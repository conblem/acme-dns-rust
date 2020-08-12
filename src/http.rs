use warp::{Filter, serve};
use tokio::net::{TcpListener, TcpStream};
use std::sync::Arc;
use parking_lot::RwLock;
use tokio_rustls::TlsAcceptor;
use rustls::{ServerConfig, NoClientAuth};
use futures_util::{StreamExt, TryStreamExt};


pub async fn test() {
    let mut https = TcpListener::bind("127.0.0.1:8080").await.unwrap();
    let https = Box::leak(Box::new(https));

    let config = Arc::new(ServerConfig::new(NoClientAuth::new()));
    let acceptor = Arc::new(RwLock::new(TlsAcceptor::from(config)));

    let acceptor_stream = futures_util::stream::repeat(Arc::clone(&acceptor));
    let stream = https
        .incoming()
        .zip(acceptor_stream)
        .map(Ok)
        .and_then(move |(stream, acceptor)| {
            acceptor.read().accept(stream.unwrap())
        });

    let test = warp::path("hello")
        .and(warp::path::param())
        .map(|map: String| {
            "test".to_string()
        });

    let https_server = serve(test).serve_incoming(stream);
    let https_spawn = tokio::spawn(async move {
        https_server.await
    });

    let http_server = serve(test).run(([127, 0, 0, 1], 8081));
    let http_spawn = tokio::spawn(async move {
        http_server.await
    });

    tokio::join!(https_spawn, http_spawn);
}