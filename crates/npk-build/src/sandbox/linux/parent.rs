use std::time::Duration;

use nix::sched::CloneFlags;
use tokio::{net::UnixListener, time::timeout};
use tokio_util::{compat::TokioAsyncReadCompatExt, sync::CancellationToken};

use crate::ipc::proto::{ChildMessage, ParentMessage};

use super::{proc, SandboxOptions, Shared};
use anyhow::{Context, Result};

pub async fn start(options: SandboxOptions) -> Result<(SandboxSender, SandboxReceiver)> {
    let shared: Shared = options.into();
    std::fs::create_dir_all(&shared.path)?;

    let child_shared = shared.clone();
    let (pid_result, pid_provide) = tokio::sync::oneshot::channel();

    let root_scope = tracing::span::Span::current();
    let thread = std::thread::spawn(move || {
        let parent_scope = tracing::span::Span::current();
        parent_scope.follows_from(root_scope);
        let clone_flags = CloneFlags::CLONE_NEWPID
            | CloneFlags::CLONE_NEWNET
            | CloneFlags::CLONE_NEWNS
            | CloneFlags::CLONE_NEWIPC
            | CloneFlags::CLONE_NEWUTS
            | CloneFlags::CLONE_NEWUSER
            | CloneFlags::CLONE_NEWCGROUP;
        let pid = proc::clone(clone_flags, || {
            tracing::span::Span::current().follows_from(parent_scope.clone());
            let shared = child_shared.clone();
            super::child::child_main(shared)
        });
        pid_result.send(pid).unwrap();
    });

    let pid_result = timeout(Duration::from_secs(5), async { pid_provide.await }).await;

    if thread.is_finished() {
        if let Err(e) = thread.join() {
            std::panic::resume_unwind(e);
        }
    }

    let pid = match pid_result {
        Ok(Ok(Ok(v))) => Ok(v),
        Ok(Ok(Err(e))) => Err(e),
        Ok(Err(e)) => Err(e).context("sender disconnected prematurely"),
        Err(e) => Err(e).context("timed out waiting for the child process to start"),
    }?;

    let listener = UnixListener::bind(&shared.socket_path)?;
    tracing::trace!("listening at {:?}", shared.socket_path);

    let receive_socket = match timeout(Duration::from_secs(1), listener.accept()).await {
        Ok(Ok(r)) => Ok(r.0),
        Ok(Err(e)) => {
            proc::kill_wait(pid).await?;
            Err(e).context("could not accept socket")
        }
        Err(e) => {
            proc::kill_wait(pid).await?;
            Err(e).context("timed out waiting for the child to connect")
        }
    }?;

    tracing::trace!("child connected");

    let (mut ipc_send, mut ipc_receive) = crate::ipc::create(receive_socket.compat()).split();

    let (receive_send, receive_receive) = flume::bounded(128);
    let (send_send, send_receive) = flume::bounded(128);
    let cancel = CancellationToken::new();
    let cancel1 = cancel.clone();
    let cancel2 = cancel.clone();
    let cancel3 = cancel.clone();

    tokio::task::spawn(async move {
        loop {
            let message = tokio::select! {
                message = ipc_receive.receive() => message,
                _ = cancel1.cancelled() => return,
            };

            let message = match message {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("socket encountered an error: {:?}", e);
                    break;
                }
            };

            let result = tokio::select! {
                r = receive_send.send_async(message) => r,
                _ = cancel1.cancelled() => return,
            };

            if result.is_err() {
                break;
            }
        }
        cancel1.cancel()
    });

    tokio::task::spawn(async move {
        loop {
            let message = tokio::select! {
                message = send_receive.recv_async() => message,
                _ = cancel2.cancelled() => break,
            };

            let message = match message {
                Ok(v) => v,
                Err(_) => break,
            };

            let result = tokio::select! {
                r = ipc_send.send(message) => r,
                _ = cancel2.cancelled() => break,
            };

            if let Err(e) = result {
                tracing::error!("socket encountered an error: {:?}", e);
                break;
            }
        }
        cancel2.cancel()
    });

    tokio::task::spawn(async move {
        cancel3.cancelled().await;
        if let Err(e) = proc::kill_wait(pid).await {
            tracing::error!("failed to kill child process: {:?}", e);
        }
    });

    Ok((
        SandboxSender { sender: send_send },
        SandboxReceiver {
            receiver: receive_receive,
        },
    ))
}

pub struct SandboxSender {
    sender: flume::Sender<ChildMessage>,
}

pub struct SandboxReceiver {
    receiver: flume::Receiver<ParentMessage>,
}

impl SandboxSender {
    pub async fn send(&self, message: ChildMessage) -> Result<(), ()> {
        self.sender.send_async(message).await.map_err(|_| ())
    }
}

impl SandboxReceiver {
    pub async fn next(&self) -> Result<ParentMessage, ()> {
        self.receiver.recv_async().await.map_err(|_| ())
    }
}
