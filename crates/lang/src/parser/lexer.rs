use std::{
    fmt::Display,
    iter::Peekable,
    ops::{Range, RemAssign},
    path::{Path, PathBuf},
    str::CharIndices,
};

use bumpalo::{collections::String, Bump};

use super::Location;

#[derive(Debug, PartialEq, Eq)]
pub struct Token<'bump> {
    loc: Location<'bump>,
    kind: TokenKind<'bump>,
}

impl<'bump> Token<'bump> {
    pub fn new(loc: Location<'bump>, kind: TokenKind<'bump>) -> Self {
        Self { loc, kind }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TokenKind<'bump> {
    Eof,
    Ident(&'bump str),
    String(&'bump str),
    InterpolationStart,
    InterpolationEnd,
    Error(ErrorKind<'bump>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ErrorKind<'bump> {
    UnterminatedString(&'bump str),
    BadEscapeSequence,
}

impl<'bump> Display for ErrorKind<'bump> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorKind::UnterminatedString(_) => f.write_str("unterminated string constant"),
            ErrorKind::BadEscapeSequence => f.write_str("invalid escape sequence"),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct LocationRef {
    line: usize,
    col: usize,
    offset: usize,
}

struct Lexer<'src, 'bump> {
    val: &'src str,
    bump: &'bump Bump,
    loc: LocationRef,
    path: PathBuf,
    working: std::string::String,
    result: Vec<Token<'bump>>,
}

pub fn lex<'bump>(path: impl AsRef<Path>, s: &str, bump: &'bump Bump) -> &'bump [Token<'bump>] {
    Lexer::new(path.as_ref().to_owned(), s, bump).lex()
}

impl<'src, 'bump> Lexer<'src, 'bump> {
    pub fn new(path: PathBuf, s: &'src str, bump: &'bump Bump) -> Self {
        Self {
            val: s,
            bump,
            loc: LocationRef {
                line: 0,
                col: 0,
                offset: 0,
            },
            path,
            working: std::string::String::new(),
            result: Vec::new(),
        }
    }

    #[inline(always)]
    fn clamp(&self, range: Range<usize>) -> Range<usize> {
        let start = range.start.min(self.val.len());
        let end = range.end.max(self.val.len());
        start..end
    }

    #[inline(always)]
    fn token(&mut self, start: LocationRef, end: usize, kind: TokenKind<'bump>) {
        self.result.push(Token {
            loc: Location::new_in(
                self.clamp(start.offset..end),
                start.line,
                start.col,
                &self.path,
                self.bump,
            ),
            kind,
        });
    }

    #[inline(always)]
    fn token_at_offset(&mut self, start: LocationRef, kind: TokenKind<'bump>) {
        self.token(start, self.loc.offset, kind)
    }

    #[inline(always)]
    fn error(&mut self, start: LocationRef, end: usize, kind: ErrorKind<'bump>) {
        self.token(start, end, TokenKind::Error(kind))
    }

    #[inline(always)]
    fn error_at_offset(&mut self, start: LocationRef, kind: ErrorKind<'bump>) {
        self.token_at_offset(start, TokenKind::Error(kind))
    }

    #[inline(always)]
    fn alloc_working(&mut self) -> &'bump str {
        let result = self.bump.alloc_str(&self.working);
        self.working.clear();
        result
    }

    #[inline(always)]
    fn alloc_str(&self, start: LocationRef) -> &'bump str {
        self.bump
            .alloc_str(&self.val[start.offset..self.loc.offset])
    }

    #[inline(always)]
    fn remainder(&self) -> &'src str {
        &self.val[self.loc.offset..]
    }

    #[inline(always)]
    fn nth_char(&self, n: usize) -> Option<char> {
        self.remainder().chars().nth(n)
    }

    #[inline(always)]
    fn advance_by(&mut self, len: usize) -> Option<(LocationRef, &'src str)> {
        if len == 0 {
            return Some((self.loc, ""));
        }

        if self.loc.offset + len > self.val.len() {
            return None;
        }

        let start = self.loc;
        self.loc.offset += len;

        let remainder = &self.val[start.offset..self.loc.offset];
        for c in remainder.chars() {
            if c == '\n' {
                self.loc.line += 1;
                self.loc.col = 0;
            } else {
                self.loc.col += 1;
            }
        }

        Some((start, remainder))
    }

    #[inline(always)]
    fn match_start(&mut self, value: &str) -> bool {
        if !self.remainder().starts_with(value) {
            return false;
        }

        if self.loc.offset >= self.val.len() {
            // Allow loops to terminate
            return true;
        }

        self.advance_by(value.len()).is_some()
    }

    #[inline(always)]
    fn take_len(&mut self, len: usize) -> Option<(LocationRef, &'src str)> {
        let start = self.loc;
        let remainder = self.remainder();
        if remainder.len() >= len {
            Some((start, &remainder[..len]))
        } else {
            None
        }
    }

    #[inline(always)]
    fn take_until(&mut self, pat: &str) -> Option<(LocationRef, &'src str)> {
        let remainder = self.remainder();
        remainder.find(pat).and_then(|i| self.advance_by(i))
    }

    #[inline(always)]
    fn take_whitespace(&mut self) {
        while let Some(c) = self.nth_char(0) {
            if c.is_whitespace() {
                self.advance_by(1);
            } else {
                break;
            }
        }
    }

    #[inline(always)]
    fn string_token(&mut self, start: LocationRef) {
        if !self.working.is_empty() {
            let s = self.alloc_working();
            self.token_at_offset(start, TokenKind::String(s));
        }
    }

    pub fn lex(mut self) -> &'bump [Token<'bump>] {
        while !self.remainder().is_empty() {
            self.root();
        }
        self.bump.alloc_slice_fill_iter(self.result)
    }

    fn root(&mut self) {
        self.take_whitespace();
        match self.nth_char(0) {
            Some('"') => self.take_string("\""),
            Some(c) if c.is_alphabetic() || c == '`' => self.take_ident(),
            _ => todo!(),
        }
        self.take_whitespace();
    }

    fn take_ident(&mut self) {
        let start = self.loc;

        self.match_start("`");
        let actual_start = self.loc;
        while let Some(c) = self.nth_char(0) {
            if !c.is_alphabetic() {
                break;
            }
            self.advance_by(1);
        }

        self.token_at_offset(start, TokenKind::Ident(self.alloc_str(actual_start)))
    }

    fn take_string(&mut self, terminator: &str) {
        let mut start = self.loc;
        self.match_start(terminator);
        while let Some(c) = self.nth_char(0) {
            match c {
                '\\' => {
                    self.advance_by(1);
                    self.take_string_escape(&mut start, terminator);
                }
                _ if self.match_start(terminator) => {
                    self.string_token(start);
                    return;
                }
                c => {
                    self.working.push(c);
                    self.advance_by(1);
                }
            }
        }
        self.error_at_offset(
            start,
            ErrorKind::UnterminatedString(self.bump.alloc_str(terminator)),
        );
    }

    fn take_string_escape(&mut self, start: &mut LocationRef, terminator: &str) {
        match self.nth_char(0) {
            Some('"') => {
                self.working.push('\"');
            }
            Some('n') => {
                self.working.push('\n');
            }
            Some('r') => {
                self.working.push('\r');
            }
            Some('t') => {
                self.working.push('\t');
            }
            Some('u' | 'x') => {
                let escape = self.loc;
                let val = if self.match_start("u{") {
                    self.take_until("}").inspect(|_| {
                        self.advance_by(1);
                    })
                } else if self.match_start("x") {
                    self.take_len(2)
                } else {
                    None
                };

                if let Some((_, val)) = val {
                    self.parse_unicode(escape, val);
                }
            }
            Some('(') => {
                self.string_token(*start);
                self.match_start("(");
                self.take_whitespace();
                while !self.match_start(")") {
                    self.root();
                }
                *start = self.loc;
            }
            _ => self.error_at_offset(self.loc, ErrorKind::BadEscapeSequence),
        }
    }

    fn parse_unicode(&mut self, start: LocationRef, val: &'src str) {
        if let Some(val) = u32::from_str_radix(val, 16).ok().and_then(char::from_u32) {
            self.working.push(val);
        } else {
            self.error_at_offset(start, ErrorKind::BadEscapeSequence);
        }
    }
}

#[cfg(test)]
mod test {
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;

    use bumpalo::Bump;

    use crate::parser::{
        lexer::{lex, Token, TokenKind},
        Location,
    };

    #[test]
    pub fn lex_unicode_escape() {
        let bump = Bump::new();
        let path = PathBuf::from("-");

        assert_eq!(
            lex(path.clone(), r#""test \u{7FFF}""#, &bump),
            &[Token::new(
                Location::new_in(0..15, 0, 0, &path, &bump),
                crate::parser::lexer::TokenKind::String("test \u{7FFF}")
            )]
        );
    }

    #[test]
    pub fn lex_ident() {
        let bump = Bump::new();
        let path = PathBuf::from("-");

        assert_eq!(
            lex(path.clone(), r#"foo"#, &bump),
            &[Token::new(
                Location::new_in(0..3, 0, 0, &path, &bump),
                TokenKind::Ident("foo")
            )]
        );
    }

    #[test]
    pub fn lex_inter_ident() {
        let bump = Bump::new();
        let path = PathBuf::from("-");

        assert_eq!(
            lex(path.clone(), r#""foo\(bar)test""#, &bump),
            &[
                Token::new(
                    Location::new_in(0..15, 0, 0, &path, &bump),
                    TokenKind::String("foo")
                ),
                Token::new(
                    Location::new_in(6..15, 0, 6, &path, &bump),
                    TokenKind::Ident("bar")
                ),
                Token::new(
                    Location::new_in(10..15, 0, 10, &path, &bump),
                    TokenKind::String("test")
                )
            ]
        );
    }
}
