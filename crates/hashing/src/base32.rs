use std::str::FromStr;

use data_encoding::Encoding;
use data_encoding_macro::new_encoding;
use thiserror::Error;

const BASE32: Encoding = new_encoding! {
    symbols: "abcdefghijklmnopqrstuvwxyz234567",
    translate_from: "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
    translate_to: "abcdefghijklmnopqrstuvwxyz",
    padding: None,
};

pub(crate) struct Base32<const SIZE: usize>(pub [u8; SIZE]);

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
        BASE32.encode_write(&self.0, f)
    }
}

impl<const SIZE: usize> std::fmt::Display for Base32<SIZE> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        BASE32.encode_write(&self.0, f)
    }
}

#[derive(Debug, Error)]
#[error("expected base32 of {} bytes", _0)]
pub(crate) struct InvalidBase32(usize);

impl<const SIZE: usize> FromStr for Base32<SIZE> {
    type Err = InvalidBase32;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let mut result = [0u8; SIZE];
        match BASE32.decode_mut(s.as_bytes(), &mut result) {
            Ok(v) if v == SIZE => Ok(Self(result)),
            _ => Err(InvalidBase32(SIZE)),
        }
    }
}
