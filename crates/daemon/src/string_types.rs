use std::{marker::PhantomData, ops::Deref, str::FromStr};

use axum_core::response::{IntoResponse, Response};
use hyper::StatusCode;
use nck_core::{base32, hashing::SupportedHash};
use serde::de::Visitor;
use thiserror::Error;

pub struct Base32<const SIZE: usize>([u8; SIZE]);

impl<const SIZE: usize> Base32<SIZE> {
    pub fn new(value: [u8; SIZE]) -> Self {
        Self(value)
    }
}

impl<const SIZE: usize> Default for Base32<SIZE> {
    fn default() -> Self {
        Self([0u8; SIZE])
    }
}

impl<const SIZE: usize> From<[u8; SIZE]> for Base32<SIZE> {
    fn from(value: [u8; SIZE]) -> Self {
        Self(value)
    }
}

impl<const SIZE: usize> From<&[u8; SIZE]> for Base32<SIZE> {
    fn from(value: &[u8; SIZE]) -> Self {
        Self(*value)
    }
}

impl<const SIZE: usize> std::fmt::Debug for Base32<SIZE> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        base32::encode_into(self.0, f)
    }
}

impl<const SIZE: usize> std::fmt::Display for Base32<SIZE> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        base32::encode_into(self.0, f)
    }
}

impl<const SIZE: usize> FromStr for Base32<SIZE> {
    type Err = InvalidHash;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let mut result = [0u8; SIZE];
        if base32::decode_into(s, &mut &mut result[..])
            .map_err(|_| InvalidHash::InvalidBase32(SIZE))?
            != SIZE
        {
            return Err(InvalidHash::InvalidBase32(SIZE));
        }
        Ok(Self(result))
    }
}

#[derive(Debug, Clone, Copy, Error)]
pub enum InvalidHash {
    #[error("the hash type must be one of the supported hash types (blake3)")]
    UnknownType,
    #[error("the hash value must be a base32 value that is {0} bytes long")]
    InvalidBase32(usize),
}

pub struct Hash(SupportedHash);

impl Hash {
    pub fn new(v: SupportedHash) -> Self {
        Self(v)
    }

    pub fn inner(&self) -> &SupportedHash {
        &self.0
    }
}

impl TryFrom<&str> for Hash {
    type Error = InvalidHash;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        value.as_bytes().try_into()
    }
}

impl TryFrom<&[u8]> for Hash {
    type Error = InvalidHash;

    fn try_from(value: &[u8]) -> std::result::Result<Self, Self::Error> {
        let value = value.as_ref();
        if value.starts_with(PREFIX_BLAKE32.as_bytes()) {
            let v = &value[PREFIX_BLAKE32.len()..];

            let mut result = [0u8; 32];
            if base32::decode_into(v, &mut &mut result[..])
                .map_err(|_| InvalidHash::InvalidBase32(32))?
                != 32
            {
                return Err(InvalidHash::InvalidBase32(32));
            }
            Ok(Hash(SupportedHash::Blake3(result)))
        } else {
            Err(InvalidHash::UnknownType)
        }
    }
}

const PREFIX_BLAKE32: &str = "blake32-";

impl std::fmt::Debug for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            SupportedHash::Blake3(h) => write!(f, "{}{:?}", PREFIX_BLAKE32, Base32::new(h)),
        }
    }
}

impl std::fmt::Display for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            SupportedHash::Blake3(h) => write!(f, "{}{}", PREFIX_BLAKE32, Base32::new(h)),
        }
    }
}

impl<'de> serde::Deserialize<'de> for Hash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(HashVisitor)
    }
}

struct HashVisitor;

impl<'de> Visitor<'de> for HashVisitor {
    type Value = Hash;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "a supported (blake32) hash value")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        v.as_bytes().try_into().map_err(|e| E::custom(e))
    }
}

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

    pub fn internal_server_error<R: IntoResponse>(r: R) -> Self {
        Self::status_code(StatusCode::INTERNAL_SERVER_ERROR, r)
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
        } else if v.starts_with("0") {
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
    pub fn inner(&self) -> &T {
        &self.0
    }

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
