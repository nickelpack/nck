#[cfg(unix)]
pub mod unix;

use std::{path::Path, time::Duration};

#[cfg(unix)]
pub use unix as current;

use thiserror::Error;

#[derive(Debug, Copy, Clone, Error)]
#[error("the remote endpoint has disconnected")]
pub struct DisconnectedError;

async fn io_timeout<R>(
    duration: Duration,
    f: impl std::future::Future<Output = std::io::Result<R>>,
) -> std::io::Result<R> {
    tokio::time::timeout(duration, f)
        .await
        .unwrap_or_else(|_| Err(std::io::Error::from(std::io::ErrorKind::TimedOut)))
}

async fn wait_for_file(path: impl AsRef<Path>) -> std::io::Result<()> {
    let mut duration = 1;
    let path = path.as_ref();
    while !path.try_exists()? {
        tokio::time::sleep(Duration::from_millis(duration));
        if duration < 64 {
            duration *= 2;
        }
    }
    Ok(())
}
