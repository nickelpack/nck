use nck_core::base32;
use serde::{de::Visitor, Deserialize, Deserializer, Serialize, Serializer};

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

impl<const SIZE: usize> Serialize for Base32<SIZE> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = self.to_string();
        serializer.serialize_str(s.as_str())
    }
}

impl<'de, const SIZE: usize> Deserialize<'de> for Base32<SIZE> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(Base32Visitor::<SIZE>)
    }
}

struct Base32Visitor<const SIZE: usize>;

impl<'de, const SIZE: usize> Visitor<'de> for Base32Visitor<SIZE> {
    type Value = Base32<SIZE>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "a base 32 value containing {} bytes", SIZE)
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        let mut result = [0u8; SIZE];
        base32::decode_into(v, &mut &mut result[..])
            .map_err(|_| E::custom(format!("invalid {}-byte base 32 value", SIZE)))?;
        Ok(Base32(result))
    }
}
