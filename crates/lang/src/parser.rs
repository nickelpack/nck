#![allow(unused)]

use std::env::consts;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::{path::PathBuf, sync::Arc};

use bumpalo::collections::Vec;
use bumpalo::Bump;

mod ident;
mod location;
mod number;
mod tables;

pub use location::Location;

#[derive(Debug, Clone, PartialEq)]
pub struct Node<'bump> {
    kind: NodeKind<'bump>,
    location: Location<'bump>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind<'bump> {
    String(&'bump str),
    Ident(&'bump str, IdentOptions),
    Error(ErrorKind),
    Integer(i64),
    Float(f64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    UnterminatedString,
    BadEscapeSequence,
    ExpectedNumber,
    InvalidNumberLiteral,
    NewLineInString,
    InvalidIdentifier,
}

bitflags::bitflags! {
    #[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
    pub struct IdentOptions: u8 {
        const HIDDEN = 0b0000_0001;
        const DECL = 0b0000_0010;
    }
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
struct LocationRef {
    line: usize,
    col: usize,
    offset: usize,
    end: Option<usize>,
}

impl LocationRef {
    #[inline(always)]
    fn with_end(mut self, end: usize) -> Self {
        self.end = Some(end);
        self
    }

    #[inline(always)]
    fn with_end_from(mut self, other: LocationRef) -> Self {
        self.with_end(other.offset)
    }

    #[inline(always)]
    fn or_with_end(mut self, end: usize) -> Self {
        self.end.get_or_insert(end);
        self
    }

    #[inline(always)]
    fn or_with_end_from(mut self, other: LocationRef) -> Self {
        self.or_with_end(other.offset)
    }
}

#[derive(Debug, Clone)]
struct Inner<'src, 'bump> {
    val: &'src str,
    path: &'bump PathBuf,
    bump: &'bump Bump,
}

impl<'src, 'bump> Inner<'src, 'bump> {
    fn new(val: &'src str, path: &'bump PathBuf, bump: &'bump Bump) -> Self {
        Self { val, path, bump }
    }

    #[inline(always)]
    fn node(&mut self, loc: LocationRef, kind: NodeKind<'bump>) -> Node<'bump> {
        let end = loc.end.unwrap_or(loc.offset);
        Node {
            location: Location::new_in(loc.offset..end, loc.line, loc.col, self.path, self.bump),
            kind,
        }
    }
}

#[derive(Debug, Clone)]
struct Scanner<'src, 'bump> {
    inner: Inner<'src, 'bump>,
    line: usize,
    col: usize,
    offset: usize,
}

impl<'src, 'bump> Scanner<'src, 'bump> {
    fn apply_str<'a>(&mut self, value: &'a str) -> &'a str {
        self.offset += value.len();
        for c in value.chars() {
            self.apply_char(c);
        }
        value
    }

    fn apply_char(&mut self, c: char) {
        if c == '\n' {
            self.line += 1;
            self.col = 0;
        } else {
            self.col += 1;
        }
    }

    fn node(
        &mut self,
        consume: Scanner<'src, 'bump>,
        loc: LocationRef,
        kind: NodeKind<'bump>,
    ) -> Node<'bump> {
        let end = loc.end.unwrap_or(consume.offset);
        self.line = consume.line;
        self.col = consume.col;
        self.offset = consume.offset;
        Node {
            kind,
            location: Location::new_in(
                loc.offset..end,
                loc.line,
                loc.col,
                consume.inner.path,
                consume.inner.bump,
            ),
        }
    }

    #[inline(always)]
    fn location(&self) -> LocationRef {
        LocationRef {
            line: self.line,
            col: self.col,
            offset: self.offset,
            end: None,
        }
    }

    #[inline(always)]
    fn remainder(&self) -> &'src str {
        &self.inner.val[self.offset..]
    }

    fn advance_by_bytes(&mut self, len: usize) -> Option<&'src str> {
        if len == 0 {
            return Some("");
        }

        let start = self.offset;
        let end = start + len;
        if end > self.inner.val.len() {
            return None;
        }

        let remainder = &self.inner.val[start..end];
        Some(self.apply_str(remainder))
    }

    fn advance_char(&mut self) -> Option<char> {
        let remainder = self.remainder();
        let mut iter = remainder.char_indices();
        if let Some((_, c)) = iter.next() {
            let len = iter.next().map(|(i, _)| i).unwrap_or(remainder.len());
            self.offset += len;
            Some(c)
        } else {
            None
        }
    }

    fn advance_by_chars(&mut self, len: usize) -> Option<&'src str> {
        if len == 0 {
            return Some("");
        }

        let start = self.offset;
        let remainder = self.remainder();
        let mut iter = remainder.char_indices().skip(len - 1);
        if iter.next().is_some() {
            let end_exclusive = iter.next().map(|(i, _)| i).unwrap_or(remainder.len());
            Some(self.apply_str(&remainder[..end_exclusive]))
        } else {
            None
        }
    }

    fn advance_while(
        &mut self,
        mut f: impl FnMut(char) -> bool,
        max: Option<usize>,
    ) -> Option<&'src str> {
        let start = self.offset;
        if start == self.inner.val.len() {
            return None;
        }

        let mut max = max.unwrap_or(usize::MAX);
        if max == 0 {
            return Some("");
        }

        let remainder = self.remainder();
        let mut iter = remainder.char_indices();

        while let Some((i, c)) = iter.next() {
            if max == 0 || !f(c) {
                if i == 0 {
                    return None;
                }

                let end_exclusive = iter.next().map(|(i, _)| i).unwrap_or(remainder.len());
                return Some(self.apply_str(&remainder[..end_exclusive]));
            }
            max -= 1;
        }

        Some(self.apply_str(remainder))
    }

    #[inline(always)]
    fn match_start(&mut self, value: &str) -> bool {
        if !self.remainder().starts_with(value) {
            return false;
        }

        self.apply_str(value);
        true
    }

    #[inline(always)]
    fn nth_char(&self, n: usize) -> Option<char> {
        self.remainder().chars().nth(n)
    }

    #[inline(always)]
    fn get_str(&self, loc: LocationRef) -> &'src str {
        let end = loc.end.unwrap_or(self.offset);
        &self.inner.val[loc.offset..end]
    }

    #[inline(always)]
    fn alloc_str_here(&self, loc: LocationRef) -> &'bump str {
        self.inner.bump.alloc_str(self.get_str(loc))
    }
}

trait NodeParser<'src, 'bump>: Sized {
    fn parse(scanner: Scanner<'src, 'bump>) -> Option<Self>;
    fn is_error(&self) -> bool;
    fn accept(self, next_location: &mut Scanner<'src, 'bump>) -> Result<Node<'bump>, Node<'bump>>;
}

#[cfg(test)]
mod test {
    use std::{ops::Range, path::PathBuf};

    use bumpalo::Bump;
    use pretty_assertions::assert_eq;

    use super::{ErrorKind, Inner, Location, LocationRef, Node, NodeKind, NodeParser, Scanner};

    static PATH: once_cell::sync::Lazy<PathBuf> = once_cell::sync::Lazy::new(|| "test".into());

    pub fn test_path() -> &'static PathBuf {
        &PATH
    }

    pub fn test_parser<'src, 'bump, P: NodeParser<'src, 'bump>>(
        src: &'src str,
        bump: &'bump Bump,
    ) -> Option<(usize, Result<Node<'bump>, Node<'bump>>)> {
        let inner = Inner::new(src, test_path(), bump);
        let mut scanner = Scanner {
            inner,
            line: 0,
            col: 0,
            offset: 0,
        };

        P::parse(scanner.clone()).map(move |r| {
            let e = r.is_error();
            let r = r.accept(&mut scanner);
            assert_eq!(e, r.is_err());
            (scanner.offset, r)
        })
    }

    pub fn make_token<'bump>(
        bump: &'bump Bump,
        range: Range<usize>,
        line: usize,
        col: usize,
        kind: NodeKind<'bump>,
    ) -> Result<Node<'bump>, Node<'bump>> {
        Ok(Node {
            kind,
            location: Location::new_in(range, line, col, test_path(), bump),
        })
    }

    pub fn make_error(
        bump: &Bump,
        range: Range<usize>,
        line: usize,
        col: usize,
        kind: ErrorKind,
    ) -> Result<Node<'_>, Node<'_>> {
        Err(Node {
            kind: NodeKind::Error(kind),
            location: Location::new_in(range, line, col, test_path(), bump),
        })
    }
}
