use std::{
    fmt::Display,
    iter::Peekable,
    ops::{Range, RemAssign},
    path::PathBuf,
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
}

#[derive(Debug, PartialEq, Eq)]
pub struct Error<'bump> {
    loc: Location<'bump>,
    kind: ErrorKind<'bump>,
}

impl<'bump> Display for Error<'bump> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.kind, f)
    }
}

impl<'bump> std::error::Error for Error<'bump> {}

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

pub struct Lexer<'src, 'bump> {
    val: &'src str,
    bump: &'bump Bump,
    iter: CharIndices<'src>,
    loc: LocationRef,
    path: PathBuf,
    working: std::string::String,
    state: State,
}

enum State {
    Root,
    String,
}

type LexerResult<'bump, T = Token<'bump>, E = Error<'bump>> = Result<T, E>;

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
            iter: s.char_indices(),
            working: std::string::String::new(),
            state: State::Root,
        }
    }

    #[inline(always)]
    fn clamp(&self, range: Range<usize>) -> Range<usize> {
        let start = range.start.min(self.val.len());
        let end = range.end.max(self.val.len());
        start..end
    }

    #[inline(always)]
    fn token(&self, start: LocationRef, end: usize, kind: TokenKind<'bump>) -> Token<'bump> {
        Token {
            loc: Location::new_in(
                self.clamp(start.offset..end),
                start.line,
                start.col,
                &self.path,
                self.bump,
            ),
            kind,
        }
    }

    #[inline(always)]
    fn token_at_offset(&self, start: LocationRef, kind: TokenKind<'bump>) -> Token<'bump> {
        self.token(start, self.loc.offset, kind)
    }

    #[inline(always)]
    fn error(&self, start: LocationRef, end: usize, kind: ErrorKind<'bump>) -> Error<'bump> {
        Error {
            loc: Location::new_in(
                self.clamp(start.offset..end),
                start.line,
                start.col,
                &self.path,
                self.bump,
            ),
            kind,
        }
    }

    #[inline(always)]
    fn error_at_offset(&self, start: LocationRef, kind: ErrorKind<'bump>) -> Error<'bump> {
        self.error(start, self.loc.offset, kind)
    }

    #[inline(always)]
    fn alloc_str(&mut self) -> &'bump str {
        let result = self.bump.alloc_str(&self.working);
        self.working.clear();
        result
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
    fn starts_with(&self, value: &str) -> bool {
        self.remainder().starts_with(value)
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
        self.iter = self.val[self.loc.offset..].char_indices();

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
        if !self.starts_with(value) {
            return false;
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

    pub fn next_token(&mut self) -> LexerResult<'bump> {
        match self.nth_char(0) {
            Some('"') => self.take_string("\""),
            _ => Ok(self.token(self.loc, self.loc.offset, TokenKind::Eof)),
        }
    }

    fn take_string(&mut self, terminator: &str) -> LexerResult<'bump> {
        let start = self.loc;
        self.match_start(terminator);
        while let Some(c) = self.nth_char(0) {
            match c {
                '\\' => {
                    self.advance_by(1);
                    self.take_string_escape(terminator)?;
                }
                _ if self.match_start(terminator) => {
                    let s = self.alloc_str();
                    return Ok(self.token_at_offset(start, TokenKind::String(s)));
                }
                c => {
                    self.working.push(c);
                    self.advance_by(1);
                }
            }
        }
        Err(self.error_at_offset(
            start,
            ErrorKind::UnterminatedString(self.bump.alloc_str(terminator)),
        ))
    }

    fn take_string_escape(&mut self, terminator: &str) -> LexerResult<'bump, ()> {
        match self.nth_char(0) {
            Some('"') => {
                self.working.push('\"');
                Ok(())
            }
            Some('n') => {
                self.working.push('\n');
                Ok(())
            }
            Some('r') => {
                self.working.push('\r');
                Ok(())
            }
            Some('t') => {
                self.working.push('\t');
                Ok(())
            }
            Some('u' | 'x') => {
                let escape = self.loc;
                let (_, val) = if self.match_start("u{") {
                    self.take_until("}").inspect(|_| {
                        self.advance_by(1);
                    })
                } else if self.match_start("x") {
                    self.take_len(2)
                } else {
                    None
                }
                .ok_or_else(|| self.error_at_offset(escape, ErrorKind::BadEscapeSequence))?;
                let val = self.parse_unicode(escape, val)?;
                self.working.push(val);
                Ok(())
            }
            _ => Err(self.error_at_offset(self.loc, ErrorKind::BadEscapeSequence)),
        }
    }

    fn parse_unicode(&self, start: LocationRef, val: &'src str) -> LexerResult<'bump, char> {
        let val = u32::from_str_radix(val, 16)
            .map_err(|_| self.error_at_offset(start, ErrorKind::BadEscapeSequence))?;
        let val = char::from_u32(val)
            .ok_or_else(|| self.error_at_offset(start, ErrorKind::BadEscapeSequence))?;
        Ok(val)
    }
}

#[cfg(test)]
mod test {
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;

    use bumpalo::Bump;

    use crate::parser::{lexer::Token, Location};

    #[test]
    pub fn lex_unicode_escape() {
        let bump = Bump::new();
        let path = PathBuf::from("-");

        let mut lexer = super::Lexer::new(path.clone(), r#""test \u{7FFF}""#, &bump);
        let token = lexer.next_token().unwrap();
        assert_eq!(
            token,
            Token::new(
                Location::new_in(0..15, 0, 0, &path, &bump),
                crate::parser::lexer::TokenKind::String("test \u{7FFF}")
            )
        );
    }
}
