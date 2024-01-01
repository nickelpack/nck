use std::fmt::Display;

use anyhow::Context;
use axum::Json;
use axum_core::response::IntoResponse;
use castaway::cast;
use hyper::StatusCode;
use serde::Serialize;

#[derive(Debug)]
pub struct AppError {
    error: anyhow::Error,
    status_code: StatusCode,
    message: Option<String>,
}

impl AppError {
    pub fn new(error: anyhow::Error, status_code: StatusCode, message: Option<String>) -> Self {
        Self {
            error,
            status_code,
            message,
        }
    }

    pub fn err(self) -> Result<!, Self> {
        Err(self)
    }
}

#[macro_export]
macro_rules! app_error {
    ($msg:literal $(,)?) => { $crate::axum_extensions::AppError::new(::anyhow::anyhow!($msg), ::hyper::StatusCode::INTERNAL_SERVER_ERROR, None) };
    ($err:expr $(,)?) => { $crate::axum_extensions::AppError::new(::anyhow::anyhow!($err), ::hyper::StatusCode::INTERNAL_SERVER_ERROR, None) };
    ($fmt:expr, $($arg:tt)*) => { $crate::axum_extensions::AppError::new(::anyhow::anyhow!($fmt, $($arg)*), ::hyper::StatusCode::INTERNAL_SERVER_ERROR, None) };
}

pub trait AppErrorOption<T> {
    fn ok_or_error<C>(self, context: C) -> Result<T, AppError>
    where
        C: Display + Send + Sync + 'static;

    fn ok_or_else_error<C, F>(self, f: F) -> Result<T, AppError>
    where
        C: Display + Send + Sync + 'static,
        F: FnOnce() -> C;

    fn ok_or_else_message(self, m: impl FnOnce() -> String) -> Result<T, AppError>;
}

impl<T> AppErrorOption<T> for Option<T> {
    fn ok_or_error<C>(self, context: C) -> Result<T, AppError>
    where
        C: Display + Send + Sync + 'static,
    {
        self.context(context).map_err(|error| AppError {
            error,
            status_code: StatusCode::NOT_FOUND,
            message: None,
        })
    }

    fn ok_or_else_error<C, F>(self, f: F) -> Result<T, AppError>
    where
        C: Display + Send + Sync + 'static,
        F: FnOnce() -> C,
    {
        self.with_context(f).map_err(|error| AppError {
            error,
            status_code: StatusCode::NOT_FOUND,
            message: None,
        })
    }

    fn ok_or_else_message(self, m: impl FnOnce() -> String) -> Result<T, AppError> {
        match self {
            Some(v) => Ok(v),
            None => {
                let msg = m();
                Err(AppError {
                    error: anyhow::Error::msg(msg.clone()),
                    status_code: StatusCode::NOT_FOUND,
                    message: Some(msg),
                })
            }
        }
    }
}

pub trait AppErrorReason<T, E> {
    fn reason<C>(self, context: C) -> Result<T, AppError>
    where
        C: Display + Send + Sync + 'static;

    fn with_reason<C, F>(self, f: F) -> Result<T, AppError>
    where
        C: Display + Send + Sync + 'static,
        F: FnOnce() -> C;

    fn status_code(self, status_code: StatusCode) -> Result<T, AppError>;

    fn with_message(self, message: impl FnOnce() -> String) -> Result<T, AppError>;
}

impl From<AppError> for anyhow::Error {
    fn from(value: AppError) -> Self {
        anyhow::anyhow!(value)
    }
}

impl<T, E: Into<anyhow::Error> + 'static> AppErrorReason<T, E> for Result<T, E> {
    fn reason<C>(self, context: C) -> Result<T, AppError>
    where
        C: Display + Send + Sync + 'static,
    {
        self.with_reason(|| context)
    }

    fn with_reason<C, F>(self, f: F) -> Result<T, AppError>
    where
        C: Display + Send + Sync + 'static,
        F: FnOnce() -> C,
    {
        self.map_err(|app| match cast!(app, AppError) {
            Ok(error) => AppError {
                error: error.error.context(f()),
                ..error
            },
            Err(error) => AppError {
                error: error.into().context(f()),
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: None,
            },
        })
    }

    fn status_code(self, status_code: StatusCode) -> Result<T, AppError> {
        self.map_err(|app| match cast!(app, AppError) {
            Ok(error) => AppError {
                status_code,
                ..error
            },
            Err(error) => AppError {
                error: error.into(),
                status_code,
                message: None,
            },
        })
    }

    fn with_message(self, message: impl FnOnce() -> String) -> Result<T, AppError> {
        self.map_err(|app| match cast!(app, AppError) {
            Ok(error) => AppError {
                message: Some(message()),
                ..error
            },
            Err(error) => AppError {
                error: error.into(),
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: Some(message()),
            },
        })
    }
}

#[cfg(debug_assertions)]
fn to_serialized_error(error: anyhow::Error, message: Option<String>) -> DebugAppError {
    let skip = message.as_ref().map(|_| 1).unwrap_or_default();
    let because = error.chain().skip(skip).map(|v| format!("{}", v)).collect();
    DebugAppError {
        message: message.unwrap_or_else(|| error.to_string()),
        trace: error.backtrace().to_string(),
        because,
    }
}

#[cfg(not(debug_assertions))]
fn to_serialized_error(error: anyhow::Error, message: Option<String>) -> ReleaseAppError {
    let correlation_id = uuid::Uuid::new_v4().simple().to_string();
    if let Some(message) = message.as_ref() {
        tracing::error!(?error, correlation_id, "an error occurred: {}", message);
    } else {
        tracing::error!(?error, correlation_id, "an error occurred");
    }
    ReleaseAppError {
        message,
        correlation_id,
    }
}

#[derive(Serialize)]
#[serde(rename = "error")]
struct DebugAppError {
    message: String,
    because: Vec<String>,
    trace: String,
}

#[derive(Serialize)]
#[serde(rename = "error")]
struct ReleaseAppError {
    message: Option<String>,
    correlation_id: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum_core::response::Response {
        let data = to_serialized_error(self.error, self.message);
        (self.status_code, Json::from(data)).into_response()
    }
}
