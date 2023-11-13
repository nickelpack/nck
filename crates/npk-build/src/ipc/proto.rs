use bitcode::{Decode, Encode};

/// A message that is sent to the parent
#[derive(Debug, Encode, Decode)]
pub enum ParentMessage {
    Hello,
    Exit { code: i32 },
    Fatal { reason: String },
    Panic { message: String, backtrace: String },
}

/// A message that is sent to the child
#[derive(Debug, Encode, Decode)]
pub enum ChildMessage {
    Hello,
}
