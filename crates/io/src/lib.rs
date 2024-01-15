#![feature(inline_const)]
#![feature(core_io_borrowed_buf)]
#![feature(read_buf)]
#![feature(thread_id_value)]

pub mod fs;
pub mod pool;

mod bytes_ext;
pub use bytes_ext::*;

mod sealed {
    pub trait Sealed {}
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub struct PrintableBuffer<'a>(pub &'a [u8]);

impl<'a> std::fmt::Debug for PrintableBuffer<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("b\"")?;

        for b in self.0 {
            match b {
                b'\0' => f.write_str("\\0")?,
                b'\r' => f.write_str("\\r")?,
                b'\n' => f.write_str("\\n")?,
                b'\t' => f.write_str("\\t")?,
                b'\\' => f.write_str("\\")?,
                b'\"' => f.write_str("\\\"")?,
                0x20..=0x7E => write!(f, "{}", *b as char)?,
                other => write!(f, "\\x{:0>2x}", other)?,
            }
        }

        f.write_str("\"")
    }
}

impl<'a> std::fmt::Display for PrintableBuffer<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in self.0 {
            match b {
                b'\0' => f.write_str("\\0")?,
                b'\r' => f.write_str("\\r")?,
                b'\n' => f.write_str("\\n")?,
                b'\t' => f.write_str("\\t")?,
                b'\\' => f.write_str("\\")?,
                0x20..=0x7E => write!(f, "{}", *b as char)?,
                other => write!(f, "\\x{:0>2x}", other)?,
            }
        }
        Ok(())
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub struct EscapedBuffer<'a>(&'a [u8]);

impl<'a> EscapedBuffer<'a> {
    pub fn new(value: &'a [u8]) -> Self {
        Self(value)
    }
}

impl<'a> std::fmt::Debug for EscapedBuffer<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("b\"")?;

        for b in self.0 {
            write!(f, "\\x{:0>2x}", b)?;
        }

        f.write_str("\"")
    }
}
