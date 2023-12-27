use std::{marker::PhantomData, ops::Deref, str::FromStr};

use axum_core::response::{IntoResponse, Response};
use hyper::StatusCode;
use serde::de::Visitor;

#[derive(Debug)]
pub enum Error {
    Unknown(anyhow::Error),
    Response(Response),
}

impl Error {
    pub fn err(self) -> Result<!, Self> {
        Err(self)
    }

    pub fn status_code<R: IntoResponse>(s: StatusCode, r: R) -> Self {
        let mut response = r.into_response();
        *response.status_mut() = s;
        Self::Response(response)
    }

    pub fn not_found<R: IntoResponse>(r: R) -> Self {
        Self::status_code(StatusCode::NOT_FOUND, r)
    }

    pub fn bad_request<R: IntoResponse>(r: R) -> Self {
        Self::status_code(StatusCode::BAD_REQUEST, r)
    }
}

impl<E: Into<anyhow::Error>> From<E> for Error {
    fn from(error: E) -> Self {
        Error::Unknown(error.into())
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let correlation_id = uuid::Uuid::new_v4().hyphenated().to_string();
        let mut response = match self {
            Self::Unknown(error) => {
                tracing::error!(correlation_id, error = ?error, "{}", error);
                Response::builder()
                    .status(500)
                    .body(axum::body::Body::empty())
                    .unwrap()
            }
            Self::Response(response) => response,
        };
        response
            .headers_mut()
            .insert("X-Correlation-ID", correlation_id.parse().unwrap());
        response
    }
}

pub type Result<T = Response, E = Error> = std::result::Result<T, E>;

#[repr(transparent)]
#[derive(Debug)]
pub struct Mode(pub u32);

impl<'de> serde::Deserialize<'de> for Mode {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(ModeVisitor)
    }
}

struct ModeVisitor;

impl<'de> Visitor<'de> for ModeVisitor {
    type Value = Mode;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a filesystem mode")
    }

    fn visit_str<E>(self, v: &str) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if v.is_empty() {
            Err(E::custom("expected filesystem mode"))
        } else if v.starts_with('0') {
            u32::from_str_radix(v, 8)
                .map(Mode)
                .map_err(|_| E::custom("expected octal filesystem mode"))
        } else {
            v.parse()
                .map(Mode)
                .map_err(|_| E::custom("expected decimal filesystem mode"))
        }
    }
}

#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UrlValue<T: FromStr>(T)
where
    T::Err: std::fmt::Display;

impl<T: FromStr> UrlValue<T>
where
    T::Err: std::fmt::Display,
{
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T: FromStr> AsRef<T> for UrlValue<T>
where
    T::Err: std::fmt::Display,
{
    fn as_ref(&self) -> &T {
        &self.0
    }
}

impl<T: FromStr> Deref for UrlValue<T>
where
    T::Err: std::fmt::Display,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de, T: FromStr> serde::Deserialize<'de> for UrlValue<T>
where
    T::Err: std::fmt::Display,
{
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(UrlValueVisitor(PhantomData))
    }
}

struct UrlValueVisitor<T: FromStr>(PhantomData<T>)
where
    T::Err: std::fmt::Display;

impl<'de, T: FromStr> Visitor<'de> for UrlValueVisitor<T>
where
    T::Err: std::fmt::Display,
{
    type Value = UrlValue<T>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        let tn = std::any::type_name::<T>();
        let mut last_upper = true;
        formatter.write_str("a ")?;
        for c in tn.chars() {
            if c.is_uppercase() && !last_upper {
                formatter.write_fmt(format_args!(" {}", c.to_lowercase()))?;
            } else {
                formatter.write_fmt(format_args!("{}", c.to_lowercase()))?;
            }
            last_upper = c.is_uppercase();
        }

        Ok(())
    }

    fn visit_str<E>(self, v: &str) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        match T::from_str(v) {
            Ok(v) => Ok(UrlValue(v)),
            Err(e) => Err(E::custom(e)),
        }
    }
}
