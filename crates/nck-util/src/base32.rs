// A direct copy of: https://github.com/andreasots/base32/tree/master
// except with lowercase and a fixed alphabet. See BASE32-LICENSE

use std::io::Write;

use bytes::BytesMut;

use crate::{pool::Pooled, BUFFER_POOL};

const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";

pub fn encode(data: &[u8]) -> String {
    let mut ret = String::with_capacity((data.len() + 3) / 4 * 5);
    encode_into(data, &mut ret);
    ret
}

pub fn encode_into(data: &[u8], string: &mut String) {
    string.reserve((data.len() + 3) / 4 * 5);
    for chunk in data.chunks(5) {
        let buf = {
            let mut buf = [0u8; 5];
            for (i, &b) in chunk.iter().enumerate() {
                buf[i] = b;
            }
            buf
        };
        string.push(ALPHABET[((buf[0] & 0xF8) >> 3) as usize] as char);
        string.push(ALPHABET[(((buf[0] & 0x07) << 2) | ((buf[1] & 0xC0) >> 6)) as usize] as char);
        string.push(ALPHABET[((buf[1] & 0x3E) >> 1) as usize] as char);
        string.push(ALPHABET[(((buf[1] & 0x01) << 4) | ((buf[2] & 0xF0) >> 4)) as usize] as char);
        string.push(ALPHABET[(((buf[2] & 0x0F) << 1) | (buf[3] >> 7)) as usize] as char);
        string.push(ALPHABET[((buf[3] & 0x7C) >> 2) as usize] as char);
        string.push(ALPHABET[(((buf[3] & 0x03) << 3) | ((buf[4] & 0xE0) >> 5)) as usize] as char);
        string.push(ALPHABET[(buf[4] & 0x1F) as usize] as char);
    }

    if data.len() % 5 != 0 {
        let len = string.len();
        let num_extra = 8 - (data.len() % 5 * 8 + 4) / 5;
        string.truncate(len - num_extra);
    }
}

const INV_ALPHABET: [i8; 43] = [
    -1, -1, 26, 27, 28, 29, 30, 31, -1, -1, -1, -1, -1, 0, -1, -1, -1, 0, 1, 2, 3, 4, 5, 6, 7, 8,
    9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
];

#[inline]
fn calculate_data_length(data: impl AsRef<[u8]>) -> usize {
    let data = data.as_ref();
    let mut unpadded_data_length = data.len();
    for i in 1..6.min(data.len()) + 1 {
        if data[data.len() - i] != b'=' {
            break;
        }
        unpadded_data_length -= 1;
    }
    unpadded_data_length * 5 / 8
}

pub fn decode(data: impl AsRef<[u8]>) -> std::io::Result<Pooled<'static, BytesMut>> {
    let data = data.as_ref();
    let length = calculate_data_length(data);
    let mut buf = BUFFER_POOL.take();
    buf.resize(length, 0u8);
    decode_into(data, &mut &mut buf[..])?;
    Ok(buf)
}

pub fn decode_into(data: impl AsRef<[u8]>, dest: &mut impl Write) -> std::io::Result<usize> {
    let data = data.as_ref();
    let output_length = calculate_data_length(data);
    let mut wrote = 0;
    for chunk in data.chunks(8) {
        let buf = {
            let mut buf = [0u8; 8];
            for (i, c) in chunk.iter().map(|v| *v as usize).enumerate() {
                // Approximately convert to uppercase, subsequent validation will fail if not alphanum.
                let c = if c >= 0x60 { c - 0x20 } else { c };
                match INV_ALPHABET.get(c.wrapping_sub(0x30)) {
                    Some(&-1) | None => {
                        return Err(std::io::Error::from(std::io::ErrorKind::InvalidData))
                    }
                    Some(&value) => buf[i] = value as u8,
                };
            }
            buf
        };
        let vals = [
            (buf[0] << 3) | (buf[1] >> 2),
            (buf[1] << 6) | (buf[2] << 1) | (buf[3] >> 4),
            (buf[3] << 4) | (buf[4] >> 1),
            (buf[4] << 7) | (buf[5] << 2) | (buf[6] >> 3),
            (buf[6] << 5) | buf[7],
        ];
        let to_copy = (output_length - wrote).min(vals.len());
        dest.write_all(&vals[..to_copy])?;
        wrote += to_copy;
    }
    Ok(wrote)
}

#[cfg(test)]
#[allow(dead_code, unused_attributes)]
mod test {
    use super::{decode, decode_into, encode};
    use std::{self, io::ErrorKind};

    #[derive(Clone)]
    struct B32 {
        c: u8,
    }

    impl std::fmt::Debug for B32 {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
            (self.c as char).fmt(f)
        }
    }

    #[test]
    fn masks_rfc4648() {
        assert_eq!(encode(&[0xF8, 0x3E, 0x7F, 0x83, 0xE7]), "7a7h7a7h");
        assert_eq!(encode(&[0x77, 0xC1, 0xF7, 0x7C, 0x1F]), "o7a7o7a7");
        assert_eq!(
            decode("7a7H7a7h").unwrap().as_ref().as_ref(),
            &[0xF8, 0x3E, 0x7F, 0x83, 0xE7]
        );
        assert_eq!(
            decode("o7a7O7a7").unwrap().as_ref().as_ref(),
            &[0x77, 0xC1, 0xF7, 0x7C, 0x1F]
        );
        assert_eq!(encode(&[0xF8, 0x3E, 0x7F, 0x83]), "7a7h7ay");
    }

    #[test]
    fn encode_decode_into() {
        for i in 0..=10u8 {
            let src = [
                0xA + i,
                0xB + i,
                0xC + i,
                0xD + i,
                0xE + i,
                0xA0 + i,
                0xB0 + i,
                0xC0 + i,
                0xD0 + i,
                0xE0 + i,
            ];
            let val = &src[..(i as usize)];
            let enc = encode(val);
            let mut dest = Vec::new();
            decode_into(&enc, &mut dest).unwrap();

            assert_eq!(val, &dest[..]);
        }
    }

    #[test]
    fn masks_unpadded_rfc4648() {
        assert_eq!(encode(&[0xF8, 0x3E, 0x7F, 0x83, 0xE7]), "7a7h7a7h");
        assert_eq!(encode(&[0x77, 0xC1, 0xF7, 0x7C, 0x1F]), "o7a7o7a7");
        assert_eq!(
            decode("7a7H7a7h").unwrap().as_ref().as_ref(),
            &[0xF8, 0x3E, 0x7F, 0x83, 0xE7]
        );
        assert_eq!(
            decode("o7a7O7a7").unwrap().as_ref().as_ref(),
            &[0x77, 0xC1, 0xF7, 0x7C, 0x1F]
        );
        assert_eq!(encode(&[0xF8, 0x3E, 0x7F, 0x83]), "7a7h7ay");
    }

    #[test]
    fn invalid_chars_rfc4648() {
        assert_eq!(decode(",").unwrap_err().kind(), ErrorKind::InvalidData)
    }

    #[test]
    fn invalid_chars_unpadded_rfc4648() {
        assert_eq!(decode(",").unwrap_err().kind(), ErrorKind::InvalidData)
    }

    #[test]
    fn too_small() {
        let mut buf = [0u8; 1];
        assert_eq!(
            decode_into("o7a7O7a7", &mut &mut buf[..])
                .unwrap_err()
                .kind(),
            ErrorKind::WriteZero
        )
    }
}
