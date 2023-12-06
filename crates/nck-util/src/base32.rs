// A direct copy of: https://github.com/andreasots/base32/tree/master
// except with lowercase and a fixed alphabet. See BASE32-LICENSE

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

pub fn decode(data: &str) -> Option<Vec<u8>> {
    let data = data.as_bytes();
    let mut unpadded_data_length = data.len();
    for i in 1..6.min(data.len()) + 1 {
        if data[data.len() - i] != b'=' {
            break;
        }
        unpadded_data_length -= 1;
    }
    let output_length = unpadded_data_length * 5 / 8;
    let mut ret = Vec::with_capacity((output_length + 4) / 5 * 5);
    for chunk in data.chunks(8) {
        let buf = {
            let mut buf = [0u8; 8];
            for (i, &c) in chunk.iter().enumerate() {
                match INV_ALPHABET.get(c.to_ascii_uppercase().wrapping_sub(b'0') as usize) {
                    Some(&-1) | None => return None,
                    Some(&value) => buf[i] = value as u8,
                };
            }
            buf
        };
        ret.push((buf[0] << 3) | (buf[1] >> 2));
        ret.push((buf[1] << 6) | (buf[2] << 1) | (buf[3] >> 4));
        ret.push((buf[3] << 4) | (buf[4] >> 1));
        ret.push((buf[4] << 7) | (buf[5] << 2) | (buf[6] >> 3));
        ret.push((buf[6] << 5) | buf[7]);
    }
    ret.truncate(output_length);
    Some(ret)
}

pub fn decode_into(data: &str, dest: &mut [u8]) -> std::fmt::Result {
    let data = data.as_bytes();
    let output_length = data.len() * 5 / 8;
    if dest.len() != output_length {
        return Err(std::fmt::Error);
    }
    let mut index = 0;
    for chunk in data.chunks(8) {
        let buf = {
            let mut buf = [0u8; 8];
            for (i, &c) in chunk.iter().enumerate() {
                match INV_ALPHABET.get(c.to_ascii_uppercase().wrapping_sub(b'0') as usize) {
                    Some(&-1) | None => return Err(std::fmt::Error),
                    Some(&value) => buf[i] = value as u8,
                };
            }
            buf
        };
        dest[index] = (buf[0] << 3) | (buf[1] >> 2);
        index += 1;
        if index == dest.len() {
            break;
        }
        dest[index] = (buf[1] << 6) | (buf[2] << 1) | (buf[3] >> 4);
        index += 1;
        if index == dest.len() {
            break;
        }
        dest[index] = (buf[3] << 4) | (buf[4] >> 1);
        index += 1;
        if index == dest.len() {
            break;
        }
        dest[index] = (buf[4] << 7) | (buf[5] << 2) | (buf[6] >> 3);
        index += 1;
        if index == dest.len() {
            break;
        }
        dest[index] = (buf[6] << 5) | buf[7];
        index += 1;
        if index == dest.len() {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(dead_code, unused_attributes)]
mod test {
    use super::{decode, encode};
    use std;

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
        assert_eq!(decode("7a7h7a7h").unwrap(), [0xF8, 0x3E, 0x7F, 0x83, 0xE7]);
        assert_eq!(decode("o7a7o7a7").unwrap(), [0x77, 0xC1, 0xF7, 0x7C, 0x1F]);
        assert_eq!(encode(&[0xF8, 0x3E, 0x7F, 0x83]), "7a7h7ay=");
    }

    #[test]
    fn masks_unpadded_rfc4648() {
        assert_eq!(encode(&[0xF8, 0x3E, 0x7F, 0x83, 0xE7]), "7a7h7a7h");
        assert_eq!(encode(&[0x77, 0xC1, 0xF7, 0x7C, 0x1F]), "o7a7o7a7");
        assert_eq!(decode("7a7h7a7h").unwrap(), [0xF8, 0x3E, 0x7F, 0x83, 0xE7]);
        assert_eq!(decode("o7a7o7a7").unwrap(), [0x77, 0xC1, 0xF7, 0x7C, 0x1F]);
        assert_eq!(encode(&[0xF8, 0x3E, 0x7F, 0x83]), "7a7h7ay");
    }

    #[test]
    fn invalid_chars_rfc4648() {
        assert_eq!(decode(","), None)
    }

    #[test]
    fn invalid_chars_unpadded_rfc4648() {
        assert_eq!(decode(","), None)
    }
}
