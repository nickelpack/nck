#![allow(unused)]

use std::env::consts;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::{path::PathBuf, sync::Arc};

use bumpalo::collections::Vec;
use bumpalo::Bump;

mod lexer;
mod location;

pub use location::Location;
