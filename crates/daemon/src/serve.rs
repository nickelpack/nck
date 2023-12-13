use std::{net::ToSocketAddrs, sync::Arc};

use axum::extract::connect_info::Connected;
use color_eyre::eyre::{self, Context};
use hyper::{
    body::Incoming,
    rt::{Read, Write},
    Request,
};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tower_service::Service;

use crate::settings::{TcpSettings, SOCKET_PATH};

enum Client {
    Tcp {
        stream: TokioIo<TcpStream>,
        remote_addr: std::net::SocketAddr,
    },
    Unix {
        stream: TokioIo<UnixStream>,
        remote_addr: Arc<tokio::net::unix::SocketAddr>,
    },
}

impl hyper::rt::Read for Client {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        match self.get_mut() {
            Client::Tcp { stream, .. } => Read::poll_read(std::pin::pin!(stream), cx, buf),
            Client::Unix { stream, .. } => Read::poll_read(std::pin::pin!(stream), cx, buf),
        }
    }
}

impl Write for Client {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        match self.get_mut() {
            Client::Tcp { stream, .. } => Write::poll_write(std::pin::pin!(stream), cx, buf),
            Client::Unix { stream, .. } => Write::poll_write(std::pin::pin!(stream), cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        match self.get_mut() {
            Client::Tcp { stream, .. } => Write::poll_flush(std::pin::pin!(stream), cx),
            Client::Unix { stream, .. } => Write::poll_flush(std::pin::pin!(stream), cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        match self.get_mut() {
            Client::Tcp { stream, .. } => Write::poll_shutdown(std::pin::pin!(stream), cx),
            Client::Unix { stream, .. } => Write::poll_shutdown(std::pin::pin!(stream), cx),
        }
    }
}

impl From<(TcpStream, std::net::SocketAddr)> for Client {
    fn from((stream, remote_addr): (TcpStream, std::net::SocketAddr)) -> Self {
        Self::Tcp {
            stream: TokioIo::new(stream),
            remote_addr,
        }
    }
}

impl From<(UnixStream, tokio::net::unix::SocketAddr)> for Client {
    fn from((stream, remote_addr): (UnixStream, tokio::net::unix::SocketAddr)) -> Self {
        Self::Unix {
            stream: TokioIo::new(stream),
            remote_addr: Arc::new(remote_addr),
        }
    }
}

#[derive(Debug, Clone)]
enum ClientInfo {
    Tcp(std::net::SocketAddr),
    Unix(Arc<tokio::net::unix::SocketAddr>),
}

impl Connected<&Client> for ClientInfo {
    fn connect_info(target: &Client) -> Self {
        match target {
            Client::Tcp { remote_addr, .. } => ClientInfo::Tcp(*remote_addr),
            Client::Unix { remote_addr, .. } => ClientInfo::Unix(remote_addr.clone()),
        }
    }
}

pub async fn serve(tcp: TcpSettings, router: axum::Router) -> eyre::Result<()>
where
{
    if tokio::fs::try_exists(SOCKET_PATH).await.is_ok() {
        tracing::trace!("cleaning up previous socket at {}", SOCKET_PATH);
        tokio::fs::remove_file(SOCKET_PATH)
            .await
            .wrap_err_with(|| format!("failed to bind to {}", SOCKET_PATH))?;
    }

    tracing::trace!("binding to {}", SOCKET_PATH);
    let unix = UnixListener::bind(SOCKET_PATH)?;

    let mut socket_addrs = Vec::with_capacity(tcp.bind.len());
    for bind in tcp.bind {
        for addr in bind.to_socket_addrs()? {
            socket_addrs.push(addr);
        }
    }
    let tcp = if socket_addrs.is_empty() {
        None
    } else {
        tracing::trace!("binding tcp to {:?}", &socket_addrs);
        Some(TcpListener::bind(&socket_addrs[..]).await?)
    };

    let mut make = router.into_make_service_with_connect_info::<ClientInfo>();

    loop {
        let socket = if let Some(tcp) = tcp.as_ref() {
            tokio::select! {
                result = unix.accept() =>  result.map(Into::into),
                result = tcp.accept() => result.map(Into::into)
            }
        } else {
            unix.accept().await.map(Into::into)
        };

        let socket = match socket {
            Err(e) if is_connection_error(&e) => continue,
            other => other,
        }?;

        let tower_service = make.call(&socket).await.unwrap_or_else(|err| match err {});

        tokio::spawn(async move {
            let hyper_service = hyper::service::service_fn(move |request: Request<Incoming>| {
                tower_service.clone().call(request)
            });

            if let Err(err) = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(socket, hyper_service)
                .await
            {
                tracing::info!(?err, "error responding to request")
            }
        });
    }
}

fn is_connection_error(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
    )
}
