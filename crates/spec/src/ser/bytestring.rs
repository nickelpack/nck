use std::{ffi::OsString, fmt::Write, os::unix::prelude::OsStringExt, path::PathBuf, rc::Rc};

use bytes::BytesMut;
use nck_io::pool::{Pooled, BUFFER_POOL};
use serde::{de::Visitor, Deserialize, Serialize};
use thiserror::Error;

/// A byte buffer that can usually consists of only valid UTF-8 data for the purposes of simple serialization to a
/// textual serialization format.
#[derive(Debug, Clone, PartialEq)]
pub struct ByteString(Rc<Pooled<'static, BytesMut>>);

impl ByteString {
    pub fn new(bytes: Pooled<'static, BytesMut>) -> Self {
        Self(Rc::new(bytes))
    }

    pub fn value(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for ByteString {
    fn from(value: Vec<u8>) -> Self {
        let mut buffer = BUFFER_POOL.take();
        buffer.extend_from_slice(&value[..]);
        Self(Rc::new(buffer))
    }
}

impl From<ByteString> for Vec<u8> {
    fn from(value: ByteString) -> Self {
        let mut result = Vec::with_capacity(value.0.len());
        result.extend_from_slice(&value.0[..]);
        result
    }
}

impl From<OsString> for ByteString {
    fn from(value: OsString) -> Self {
        value.into_vec().into()
    }
}

impl From<ByteString> for OsString {
    fn from(value: ByteString) -> Self {
        OsString::from_vec(value.into())
    }
}

impl From<PathBuf> for ByteString {
    fn from(value: PathBuf) -> Self {
        value.into_os_string().into()
    }
}

impl From<ByteString> for PathBuf {
    fn from(value: ByteString) -> Self {
        let str: OsString = value.into();
        Self::from(str)
    }
}

impl Serialize for ByteString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::Error;
        let mut target = BUFFER_POOL.take();
        target.reserve(self.0.len());

        let mut b = &self.0[..];
        loop {
            match b {
                [b'{', ..] => {
                    target.extend_from_slice(b"{{");
                    b = &b[1..];
                }
                [b'}', ..] => {
                    target.extend_from_slice(b"}}");
                    b = &b[1..];
                }
                [other, ..] => {
                    let width = b.len().min(printable_utf8_char_width(*other));
                    match std::str::from_utf8(&b[..width]) {
                        Ok("") | Err(_) => {
                            target
                                .write_fmt(format_args!("{{{:0<2x}}}", other))
                                .map_err(S::Error::custom)?;
                            b = &b[1..];
                        }
                        Ok(other) => {
                            target.extend_from_slice(other.as_bytes());
                            b = &b[width..];
                        }
                    }
                }
                [] => break,
            }
        }

        #[cfg(not(any(test, debug_assertions)))]
        let s = unsafe { std::str::from_utf8_unchecked(&target[..]) };
        #[cfg(any(test, debug_assertions))]
        let s =
            std::str::from_utf8(&target[..]).expect("byte string should produce valid UTF8 data");
        serializer.serialize_str(s)
    }
}

impl<'de> Deserialize<'de> for ByteString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(ByteStringVisitor)
    }
}

#[derive(Error, Debug, Copy, Clone, PartialEq, Eq)]
enum ByteStringError {
    #[error("unescaped '{{' or '}}' in byte string")]
    UnescapedBraces,
    #[error("invalid escape sequence in byte string")]
    InvalidEscape,
}

struct ByteStringVisitor;

impl<'de> Visitor<'de> for ByteStringVisitor {
    type Value = ByteString;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("byte string")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        let mut bytes = BUFFER_POOL.take();
        bytes.reserve(v.len());

        let mut v = v.as_bytes();
        loop {
            match v {
                [b'{', a, b, b'}', ..] => {
                    let b = hex_byte(*a, *b).map_err(E::custom)?;
                    bytes.extend_from_slice(&[b]);
                    v = &v[4..];
                }
                [b'{', b'{', ..] => {
                    bytes.extend_from_slice(b"{");
                    v = &v[2..];
                }
                [b'}', b'}', ..] => {
                    bytes.extend_from_slice(b"}");
                    v = &v[2..];
                }
                [b'{', ..] => return Err(E::custom(ByteStringError::UnescapedBraces)),
                [b'}', ..] => return Err(E::custom(ByteStringError::UnescapedBraces)),
                [other, ..] => {
                    bytes.extend_from_slice(&[*other]);
                    v = &v[1..];
                }
                [] => break,
            }
        }

        Ok(ByteString(Rc::new(bytes)))
    }
}

fn hex_byte(a: u8, b: u8) -> Result<u8, ByteStringError> {
    let a = hex(a)?;
    let b = hex(b)?;
    Ok(a.overflowing_shl(4).0 | b)
}

fn hex(b: u8) -> Result<u8, ByteStringError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(ByteStringError::InvalidEscape),
    }
}

// https://tools.ietf.org/html/rfc3629
// Also checking for printable characters (0x20..=0x7E)
const PRINTABLE_UTF8_CHAR_WIDTH: &[u8; 256] = &[
    // 1  2  3  4  5  6  7  8  9  A  B  C  D  E  F
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 1
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, // 2
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, // 3
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, // 4
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, // 5
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, // 6
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, // 7
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 8
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 9
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // A
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // B
    0, 0, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, // C
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, // D
    3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, // E
    4, 4, 4, 4, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // F
];

#[inline(always)]
fn printable_utf8_char_width(b: u8) -> usize {
    PRINTABLE_UTF8_CHAR_WIDTH[b as usize] as usize
}

#[cfg(test)]
mod test {
    use serde_test::{assert_tokens, Token};

    use super::*;

    fn byte_string(b: &[u8]) -> ByteString {
        let mut buffer = BUFFER_POOL.take();
        buffer.extend_from_slice(b);
        ByteString::new(buffer)
    }

    #[test]
    fn ser_de_printable() {
        let printable = byte_string("this is a test 😊".as_bytes());
        assert_tokens(&printable, &[Token::Str("this is a test 😊")])
    }

    #[test]
    fn ser_de_unprintable() {
        let printable = byte_string(b"\0\x17this is a test");
        assert_tokens(&printable, &[Token::Str("{00}{17}this is a test")])
    }

    #[test]
    fn ser_de_invalid_utf8() {
        let printable = byte_string(b"\xC8\xC9\xAAthis is a test\xE4\xAAo");
        assert_tokens(&printable, &[Token::Str("{c8}ɪthis is a test{e4}{aa}o")])
    }

    #[test]
    fn ser_de_escaped() {
        let printable = byte_string(b"{hello} world");
        assert_tokens(&printable, &[Token::Str("{{hello}} world")])
    }
}
