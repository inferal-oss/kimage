use anyhow::{Context, Result};
use axum::Router;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{self, pki_types::PrivateKeyDer},
};
/// Wire-compatible serving release; the v1 endpoint challenge is unchanged.
pub const VERSION: &str = "0.2.0";
/// Load projected cert/key on each new connection, so Kubernetes atomic renewal
/// takes effect without terminating existing requests. Invalid replacements fail closed.
fn acceptor(cert: &PathBuf, key: &PathBuf) -> Result<TlsAcceptor> {
    let certificates =
        rustls_pemfile::certs(&mut std::io::BufReader::new(std::fs::File::open(cert)?))
            .collect::<std::io::Result<Vec<_>>>()?;
    let key: PrivateKeyDer<'static> =
        rustls_pemfile::private_key(&mut std::io::BufReader::new(std::fs::File::open(key)?))?
            .context("MissingTlsKey")?;
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certificates, key)?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}
pub async fn shutdown() {
    let mut term =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM");
    tokio::select! {_=term.recv()=>{},_=tokio::signal::ctrl_c()=>{}}
}
/// Set dual-stack explicitly for Pod/service addresses. Linux and macOS have
/// different default IPV6_V6ONLY values; API forwarding also needs IPv4 loopback.
pub fn listener(address: SocketAddr) -> Result<tokio::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(
        Domain::for_address(address),
        Type::STREAM,
        Some(Protocol::TCP),
    )?;
    if address.is_ipv6() {
        socket.set_only_v6(false)?;
    }
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&address.into())?;
    socket.listen(128)?;
    Ok(tokio::net::TcpListener::from_std(socket.into())?)
}
pub async fn tls(listen: SocketAddr, cert: PathBuf, key: PathBuf, router: Router) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    acceptor(&cert, &key)?;
    let listener = listener(listen)?;
    let permits = Arc::new(tokio::sync::Semaphore::new(64));
    let mut tasks = tokio::task::JoinSet::new();
    let stop = shutdown();
    tokio::pin!(stop);
    loop {
        tokio::select! {
            _=&mut stop=>break,
            Some(_)=tasks.join_next(), if !tasks.is_empty()=>{},
            incoming=listener.accept()=>{
                let (stream,_)=incoming?;
                let Ok(permit)=permits.clone().try_acquire_owned()else{continue;};
                let Ok(acceptor)=acceptor(&cert,&key)else{tracing::warn!(reason="TlsReloadFailed");continue;};
                let router=router.clone();
                tasks.spawn(async move{
                    let _permit=permit;
                    if let Ok(Ok(stream))=tokio::time::timeout(Duration::from_secs(5),acceptor.accept(stream)).await{
                        let builder=Builder::new(TokioExecutor::new());
                        let _=tokio::time::timeout(Duration::from_secs(180),builder.serve_connection(TokioIo::new(stream),TowerToHyperService::new(router))).await;
                    }
                });
            }
        }
    }
    if tokio::time::timeout(Duration::from_secs(10), async {
        while tasks.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        tasks.abort_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn service_listener_accepts_both_families_on_one_port() {
        let listener = super::listener("[::]:0".parse().unwrap()).unwrap();
        let port = listener.local_addr().unwrap().port();
        for address in [format!("127.0.0.1:{port}"), format!("[::1]:{port}")] {
            let connection = tokio::net::TcpStream::connect(address).await.unwrap();
            let (_, _) = listener.accept().await.unwrap();
            drop(connection);
        }
    }
}
