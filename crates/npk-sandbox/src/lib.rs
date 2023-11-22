#![feature(result_option_inspect)]
#[cfg(unix)]
pub mod unix;

#[cfg(unix)]
pub use unix as current;
