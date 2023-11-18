use bitcode::{Decode, Encode};
use crc::Crc;

const CRC: Crc<u64> = Crc::<u64>::new(&crc::CRC_64_REDIS);

pub fn crc(buf: impl AsRef<[u8]>) -> u64 {
    CRC.checksum(buf.as_ref())
}

#[derive(Debug, Encode, Decode)]
pub enum ParentMessage {
    Hello,
}

#[derive(Debug, Encode, Decode)]
pub enum ChildMessage {
    Hello,
}
