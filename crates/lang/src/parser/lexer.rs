// Having a lexer return data through an iterator is an extremely interesting *academic* problem these days. Nearly all
// parsing frameworks that I have come across read the entire file up-front, rendering much of the iterative lexing
// pointless. Here we lex to a vec. Simple, probably faster.

use std::{
    cell::RefCell,
    fmt::Display,
    iter::Peekable,
    ops::{Deref, DerefMut, Range, RemAssign},
    path::{Path, PathBuf},
    str::CharIndices,
};

use bitflags::Flags;
use bumpalo::Bump;

use super::Location;

mod ident;
mod number;
mod operators;
mod strings;
mod surrounds;
mod tables;

#[derive(Debug, Clone, PartialEq)]
pub struct Token<'bump> {
    kind: TokenKind<'bump>,
    location: Location<'bump>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Error<'bump> {
    kind: ErrorKind,
    location: Location<'bump>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommaKind {
    Explicit,
    Automatic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,
    Sub,
    Div,
    Mul,
    LogicalAnd,
    And,
    LogicalOr,
    Or,
    Inequal,
    ApproxInequal,
    Equal,
    ApproxEqual,
    LessEqual,
    Less,
    GreaterEqual,
    Greater,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind<'bump> {
    String(&'bump str, StringOptions),
    Bytes(&'bump [u8], StringOptions),
    Ident(&'bump str, IdentOptions),
    Integer(i64),
    Float(f64),
    Null,
    True,
    False,
    For,
    In,
    Let,
    If,
    Colon,
    Question,
    Bang,
    Comma(CommaKind),
    Elipses,
    Dot,
    Bottom,
    BinaryOperator(BinaryOperator),
    Assign,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    LParen,
    RParen,
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

bitflags::bitflags! {
    #[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
    pub struct StringOptions: u8 {
        const OPEN = 0b0000_0001;
        const CLOSE = 0b0000_0010;
        const ENTER = 0b0000_0100; // Interpolation
        const EXIT = 0b0000_1000; // Interpolation
        const MULTI = 0b0001_0000;
    }
}

bitflags::bitflags! {
    #[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
    struct InterpOptions: u8 {
        const BYTE = 0b0000_0001;
        const MULTI = 0b0000_0010;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope<'src> {
    Paren,
    Brace,
    Bracket,
    Interpolation {
        options: InterpOptions,
        hashes: &'src str,
    },
}

impl<'src> Scope<'src> {
    fn is_interpolation(&self) -> bool {
        matches!(self, Self::Interpolation { .. })
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
            self.apply_char(c);
            Some(c)
        } else {
            None
        }
    }

    #[inline(always)]
    fn get_chars(&mut self, len: usize) -> Option<&'src str> {
        if len == 0 {
            return Some("");
        }

        let start = self.offset;
        let remainder = self.remainder();
        if remainder.len() >= len {
            Some(&remainder[..len])
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
        if value.is_empty() {
            return true;
        }

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
    fn bump(&self) -> &'bump Bump {
        self.inner.bump
    }

    #[inline(always)]
    fn alloc_str_here(&self, loc: LocationRef) -> &'bump str {
        self.bump().alloc_str(self.get_str(loc))
    }
}

trait TokenLexer<'src, 'bump>: Sized {
    fn lex(lexer: &Lexer<'src, 'bump>, scanner: Scanner<'src, 'bump>) -> Option<Self>;
    fn is_error(&self) -> bool;
    fn accept(self, lexer: &mut Lexer<'src, 'bump>);
}

#[derive(Debug, Clone)]
struct Lexer<'src, 'bump> {
    inner: Inner<'src, 'bump>,
    scanner: Scanner<'src, 'bump>,
    scopes: Vec<Scope<'src>>,
    tokens: Vec<Token<'bump>>,
    errors: Vec<Error<'bump>>,
}

impl<'src, 'bump> Lexer<'src, 'bump> {
    pub fn new(src: &'src str, path: &Path, bump: &'bump Bump) -> Self {
        let inner = Inner {
            val: src,
            path: bump.alloc_with(|| path.to_path_buf()),
            bump,
        };
        Self {
            inner: inner.clone(),
            scanner: Scanner {
                inner,
                line: 0,
                col: 0,
                offset: 0,
            },
            scopes: Vec::new(),
            tokens: Vec::new(),
            errors: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.scanner.offset == self.inner.val.len()
    }

    #[inline(always)]
    fn push_scope(&mut self, scope: Scope<'src>) {
        self.scopes.push(scope)
    }

    #[inline(always)]
    fn peek_scope(&self) -> Option<Scope<'src>> {
        self.scopes.last().copied()
    }

    #[inline(always)]
    fn pop_scope(&mut self) -> Option<Scope<'src>> {
        self.scopes.pop()
    }

    fn error(
        &mut self,
        scanner: &Scanner<'src, 'bump>,
        errors: impl Iterator<Item = (LocationRef, ErrorKind)>,
    ) {
        for (loc, kind) in errors {
            let end = loc.end.unwrap_or(scanner.offset);
            self.errors.push(Error {
                kind,
                location: Location::new_in(
                    loc.offset..end,
                    loc.line,
                    loc.col,
                    scanner.inner.path,
                    scanner.inner.bump,
                ),
            })
        }
    }

    fn token(
        &mut self,
        consume: Scanner<'src, 'bump>,
        tokens: impl Iterator<Item = (LocationRef, TokenKind<'bump>)>,
    ) {
        self.scanner.line = consume.line;
        self.scanner.col = consume.col;
        self.scanner.offset = consume.offset;
        for (loc, kind) in tokens {
            let end = loc.end.unwrap_or(consume.offset);
            self.tokens.push(Token {
                kind,
                location: Location::new_in(
                    loc.offset..end,
                    loc.line,
                    loc.col,
                    consume.inner.path,
                    consume.inner.bump,
                ),
            })
        }
    }

    fn lex<L: TokenLexer<'src, 'bump>>(&self) -> Option<L> {
        let scanner = self.scanner.clone();
        L::lex(self, scanner)
    }
}

#[cfg(test)]
mod test {
    use std::{ops::Range, path::PathBuf};

    use bumpalo::Bump;
    use pretty_assertions::assert_eq;

    use super::{
        Error, ErrorKind, Inner, Lexer, Location, LocationRef, Scanner, Token, TokenKind,
        TokenLexer,
    };

    static PATH: once_cell::sync::Lazy<PathBuf> = once_cell::sync::Lazy::new(|| "test".into());

    pub fn test_path() -> &'static PathBuf {
        &PATH
    }

    pub fn run_lexer<'src, 'bump, L: TokenLexer<'src, 'bump>>(
        src: &'src str,
        bump: &'bump Bump,
    ) -> Option<(usize, Vec<Token<'bump>>, Vec<Error<'bump>>)> {
        let mut lexer = Lexer::new(src, test_path(), bump);
        lexer.lex::<L>().map(move |r| {
            r.accept(&mut lexer);
            (lexer.scanner.offset, lexer.tokens, lexer.errors)
        })
    }

    pub fn make_token<'bump>(
        bump: &'bump Bump,
        range: Range<usize>,
        line: usize,
        col: usize,
        kind: TokenKind<'bump>,
    ) -> Token<'bump> {
        Token {
            kind,
            location: Location::new_in(range, line, col, test_path(), bump),
        }
    }

    pub fn make_error<'bump>(
        bump: &'bump Bump,
        range: Range<usize>,
        line: usize,
        col: usize,
        kind: ErrorKind,
    ) -> Error<'bump> {
        Error {
            kind,
            location: Location::new_in(range, line, col, test_path(), bump),
        }
    }
}
