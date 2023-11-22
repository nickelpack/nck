use std::time::{Duration, Instant};

use thiserror::Error;

#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
#[error("the operation timed out")]
pub struct Timeout;

impl<F> From<tokio::time::Timeout<F>> for Timeout {
    fn from(_: tokio::time::Timeout<F>) -> Self {
        Timeout
    }
}

pub fn timeout<R>(duration: Duration, mut f: impl FnMut() -> Option<R>) -> Result<R, Timeout> {
    let timeout = Instant::now() + duration;
    let mut duration = 1;
    loop {
        if let Some(result) = f() {
            return Ok(result);
        }

        if Instant::now() > timeout {
            return Err(Timeout);
        }

        std::thread::sleep(Duration::from_millis(duration));
        if duration < 64 {
            duration *= 2;
        }
    }
}
