use std::{iter::Peekable, ops::Range, path::PathBuf, str::CharIndices};

use bumpalo::{collections::String, Bump};

use super::Location;

pub struct Token<'bump> {
    loc: Location<'bump>,
    kind: TokenKind<'bump>,
}

pub enum TokenKind<'bump> {
    Eof,
    Ident(&'bump str),
    String(&'bump str),
}

pub struct Error<'bump> {
    loc: Location<'bump>,
    kind: ErrorKind<'bump>,
}

pub enum ErrorKind<'bump> {
    UnterminatedString(&'bump str),
}

pub struct Lexer<'src, 'bump> {
    s: &'src str,
    bump: &'bump Bump,
    line: usize,
    col: usize,
    chars: Peekable<CharIndices<'src>>,
    path: PathBuf,
    previous: Option<char>,
    working: std::string::String,
}

type LexerResult<'bump, T = Token<'bump>, E = Error<'bump>> = Result<T, E>;

impl<'src, 'bump> Lexer<'src, 'bump> {
    pub fn new(path: PathBuf, s: &'src str, bump: &'bump Bump) -> Self {
        Self {
            s,
            bump,
            line: 0,
            col: 0,
            chars: s.char_indices().peekable(),
            path,
            previous: None,
            working: std::string::String::new(),
        }
    }

    #[inline(always)]
    fn clamp(&self, range: Range<usize>) -> Range<usize> {
        let start = range.start.min(self.s.len());
        let end = range.end.max(self.s.len());
        start..end
    }

    #[inline(always)]
    fn token(&self, range: Range<usize>, kind: TokenKind<'bump>) -> Token<'bump> {
        Token {
            loc: Location::new_in(
                self.clamp(range),
                self.line,
                self.col,
                &self.path,
                self.bump,
            ),
            kind,
        }
    }

    #[inline(always)]
    fn error(&self, range: Range<usize>, kind: ErrorKind<'bump>) -> Error<'bump> {
        Error {
            loc: Location::new_in(
                self.clamp(range),
                self.line,
                self.col,
                &self.path,
                self.bump,
            ),
            kind,
        }
    }

    fn alloc_str(&mut self) -> &'bump str {
        let result = self.bump.alloc_str(&self.working);
        self.working.clear();
        result
    }

    fn end(&self) -> usize {
        self.s.len()
    }

    fn next_char(&mut self) -> Option<(usize, char)> {
        if let Some('\n') = self.previous.take() {
            self.line += 1;
            self.col = 0;
        };
        let (i, c) = self.chars.next()?;
        self.previous = Some(c);
        Some((i, c))
    }

    pub fn next_token(&mut self) -> LexerResult<'bump> {
        let (index, c) = match self.next_char() {
            Some(v) => v,
            None => return Ok(self.token(self.s.len()..self.s.len(), TokenKind::Eof)),
        };

        let next = self.chars.peek().map(|(_, v)| *v);

        match (c, next) {
            ('"', _) => self.take_string(index),
        }
    }

    fn take_string(&mut self, index: usize) -> LexerResult<'bump> {
        while let Some((i, c)) = self.next_char() {
            match c {
                '"' => {
                    return Ok(self.token(index..(i + 1), TokenKind::String(self.alloc_str())));
                }
                '\\' => {
                    match self.chars.peek() {
                        Some((_, '\\' | '"' | ))
                    }
                }
            }
        }
        Err(self.error(
            index..self.end(),
            ErrorKind::UnterminatedString(self.bump.alloc("\"")),
        ))
    }
}
