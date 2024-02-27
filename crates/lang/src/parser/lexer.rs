// Having a lexer return data through an iterator is an extremely interesting *academic* problem these days. Nearly all
// parsing frameworks that I have come across read the entire file up-front, rendering much of the iterative lexing
// pointless. Here we lex to a vec. Simple, probably faster.

use std::{
    fmt::Display,
    iter::Peekable,
    ops::{Deref, DerefMut, Range, RemAssign},
    path::{Path, PathBuf},
    str::CharIndices,
};

use bitflags::Flags;
use bumpalo::Bump;

use super::Location;
mod strings;
mod tables;

#[derive(Debug, PartialEq)]
pub struct Token<'bump> {
    loc: Location<'bump>,
    kind: TokenKind<'bump>,
}

impl<'bump> Token<'bump> {
    pub fn new(loc: Location<'bump>, kind: TokenKind<'bump>) -> Self {
        Self { loc, kind }
    }
}

bitflags::bitflags! {
    #[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
    pub struct IdentOptions: u8 {
        const HIDDEN = 0b0000_0001;
        const DECL = 0b0000_0010;
    }
}

#[derive(Debug, PartialEq)]
pub enum TokenKind<'src> {
    Eof,
    Ident(&'src str, IdentOptions),
    String(&'src str),
    Bytes(&'src [u8]),
    InterpolationStart,
    InterpolationEnd,
    Error(ErrorKind),
    Integer(u64),
    Floating(f64),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ErrorKind {
    UnterminatedString,
    BadEscapeSequence,
    ExpectedNumber,
    InvalidNumberLiteral,
    NewLineInString,
}

impl Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorKind::UnterminatedString => f.write_str("unterminated string constant"),
            ErrorKind::BadEscapeSequence => f.write_str("invalid escape sequence"),
            ErrorKind::ExpectedNumber => f.write_str("expected a number"),
            ErrorKind::InvalidNumberLiteral => f.write_str("invalid number literal"),
            ErrorKind::NewLineInString => todo!(),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct LocationRef {
    line: usize,
    col: usize,
    offset: usize,
}

// pub fn lex<'bump>(path: impl AsRef<Path>, s: &str, bump: &'bump Bump) -> &'bump [Token<'bump>] {
//     Lexer::new(path.as_ref().to_owned(), s, bump).lex()
// }

struct Lexer<'src> {
    val: &'src str,
    bump: &'bump Bump,
    loc: LocationRef,
    path: PathBuf,
    string: String,
    bytes: Vec<u8>,
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
            string: std::string::String::new(),
            bytes: Vec::new(),
        }
    }

    #[inline(always)]
    fn clamp(&self, range: Range<usize>) -> Range<usize> {
        let start = range.start.min(self.val.len());
        let end = range.end.max(self.val.len());
        start..end
    }

    #[inline(always)]
    fn alloc_working(&mut self) -> &'bump str {
        let result = self.bump.alloc_str(&self.string);
        self.string.clear();
        result
    }

    #[inline(always)]
    fn get_str(&self, start: LocationRef) -> &'src str {
        &self.val[start.offset..self.loc.offset]
    }

    #[inline(always)]
    fn alloc_str(&self, start: LocationRef) -> &'bump str {
        self.bump.alloc_str(self.get_str(start))
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
}

struct LexerFrame<'src, 'bump> {
    lexer: &'src mut Lexer<'src, 'bump>,
    result: Vec<Token<'bump>>,
}

impl<'src, 'bump> Deref for LexerFrame<'src, 'bump> {
    type Target = Lexer<'src, 'bump>;

    fn deref(&self) -> &Self::Target {
        self.lexer
    }
}

impl<'src, 'bump> DerefMut for LexerFrame<'src, 'bump> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.lexer
    }
}

impl<'src, 'bump> LexerFrame<'src, 'bump> {
    pub fn new(lexer: &'src mut Lexer<'src, 'bump>) -> Self {
        Self {
            lexer,
            result: Vec::new(),
        }
    }

    #[inline(always)]
    fn token(&mut self, start: LocationRef, end: usize, kind: TokenKind<'bump>) {
        self.result.push(Token {
            loc: Location::new_in(
                self.lexer.clamp(start.offset..end),
                start.line,
                start.col,
                &self.lexer.path,
                self.lexer.bump,
            ),
            kind,
        });
    }

    #[inline(always)]
    fn token_at_offset(&mut self, start: LocationRef, kind: TokenKind<'bump>) {
        self.token(start, self.lexer.loc.offset, kind)
    }

    #[inline(always)]
    fn error(&mut self, start: LocationRef, end: usize, kind: ErrorKind) {
        self.token(start, end, TokenKind::Error(kind))
    }

    #[inline(always)]
    fn error_at_offset(&mut self, start: LocationRef, kind: ErrorKind) {
        self.token_at_offset(start, TokenKind::Error(kind))
    }

    pub fn lex(&mut self) {
        while !self.remainder().is_empty() {
            self.root();
        }
    }

    fn root(&mut self) {
        self.take_whitespace();
        match self.nth_char(0) {
            Some('"' | '#' | '\'' | '_') => self.take_string_or_ident(),
            Some('0'..='9') => self.take_num(),
            Some('.')
                if self
                    .nth_char(1)
                    .map(|v| v.is_ascii_digit())
                    .unwrap_or_default() =>
            {
                self.take_num()
            }
            Some(c) if tables::derived_property::XID_Start(c) => {
                self.take_ident(IdentOptions::empty())
            }
            Some(c) => todo!("{c}"),
            _ => (),
        }
        self.take_whitespace();
    }

    fn take_num(&mut self) {
        let start = self.loc;
        if self.match_start("0x") || self.match_start("0X") {
            self.take_radix_num(start, 16, char::is_ascii_hexdigit);
        } else if self.match_start("0o") {
            self.take_radix_num(start, 8, |c| ('0'..='7').contains(c));
        } else if self.match_start("0b") {
            self.take_radix_num(start, 2, |c| ('0'..='1').contains(c));
        } else {
            self.take_decimal_num();
        }
    }

    fn take_decimal_num(&mut self) {
        let start = self.loc;

        let mut denom = false;
        let mut expo = false;

        while let Some(c) = self.nth_char(0) {
            let loc = self.loc;
            match c {
                '0'..='9' => (),
                '.' if !denom && !expo => {
                    denom = true;
                }
                'e' | 'E' if !expo => {
                    expo = true;
                    self.advance_by(1);
                    if !self.match_start("+") {
                        self.match_start("-");
                    }
                    self.lexer.string.push_str(self.get_str(loc));
                    continue;
                }
                '_' => {
                    self.advance_by(1);
                    continue;
                }
                _ => break,
            }
            self.advance_by(1);
            self.string.push(c);
        }

        let u = self.take_num_unit();

        if !denom && !expo {
            if let Some(v) = self.string.parse().ok().and_then(|v| u.checked_mul(v)) {
                self.token_at_offset(start, TokenKind::Integer(v))
            } else {
                self.error_at_offset(start, ErrorKind::InvalidNumberLiteral);
            }
        } else if let Ok(v) = self.string.parse() {
            if u == 1 {
                self.token_at_offset(start, TokenKind::Floating(v))
            } else {
                let v = v * u as f64;
                if v.is_finite() {
                    self.token_at_offset(start, TokenKind::Integer(v as u64))
                } else {
                    self.error_at_offset(start, ErrorKind::InvalidNumberLiteral);
                }
            }
        } else {
            self.error_at_offset(start, ErrorKind::InvalidNumberLiteral);
        }

        self.string.clear();
    }

    fn take_radix_num(
        &mut self,
        orig_start: LocationRef,
        radix: u32,
        valid_char: impl Fn(&char) -> bool,
    ) {
        let start = self.loc;
        while let Some(c) = self.nth_char(0) {
            if c == '_' {
                self.advance_by(1);
                continue;
            }
            if !valid_char(&c) {
                break;
            }
            self.advance_by(1);
            self.string.push(c);
        }

        if self.string.is_empty() {
            self.error_at_offset(orig_start, ErrorKind::ExpectedNumber);
            return;
        }

        let unit = self.take_num_unit();

        if let Some(s) = u64::from_str_radix(&self.string, radix)
            .ok()
            .and_then(|v| v.checked_mul(unit))
        {
            self.token_at_offset(orig_start, TokenKind::Integer(s));
        } else {
            self.error_at_offset(orig_start, ErrorKind::InvalidNumberLiteral);
        }
        self.string.clear();
    }

    fn take_num_unit(&mut self) -> u64 {
        match self.nth_char(0) {
            Some('K') if self.match_start("Ki") => 1_024,
            Some('K') if self.match_start("K") => 1_000,
            Some('G') if self.match_start("Gi") => 1_048_576,
            Some('G') if self.match_start("G") => 1_000_000,
            Some('T') if self.match_start("Ti") => 1_073_741_824,
            Some('T') if self.match_start("T") => 1_000_000_000,
            Some('P') if self.match_start("Pi") => 1_099_511_627_776,
            Some('P') if self.match_start("P") => 1_000_000_000_000,
            _ => 1,
        }
    }

    fn take_ident(&mut self, mut options: IdentOptions) {
        let start = self.loc;

        if self.match_start("_") {
            options |= IdentOptions::HIDDEN;
        }
        if self.match_start("#") {
            options |= IdentOptions::DECL;
        }

        let actual_start = self.loc;
        while let Some(c) = self.nth_char(0) {
            if !tables::derived_property::XID_Continue(c) {
                break;
            }
            self.advance_by(1);
        }

        self.token_at_offset(
            start,
            TokenKind::Ident(self.alloc_str(actual_start), options),
        )
    }
}

#[cfg(test)]
mod test {
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;

    use bumpalo::Bump;

    use crate::parser::{
        lexer::{lex, ErrorKind, IdentOptions, Token, TokenKind},
        Location,
    };

    macro_rules! test_tokens {
        ($name: ident, $src: literal, [$(($range: expr, $line: literal, $col: literal, $kind: expr)),+]) => {
            #[test]
            pub fn $name() {
                let bump = Bump::new();
                let path = PathBuf::from("src");
                assert_eq!(
                    lex(path.clone(), $src, &bump),
                    &[
                        $(Token::new(
                            Location::new_in($range, $line, $col, &path, &bump),
                            $kind
                        )),+
                    ]
                );
            }
        };
    }

    test_tokens!(
        ident,
        r#"foo"#,
        [(0..3, 0, 0, TokenKind::Ident("foo", IdentOptions::empty()))]
    );

    test_tokens!(
        hidden_ident,
        r#"_foo"#,
        [(0..4, 0, 0, TokenKind::Ident("foo", IdentOptions::HIDDEN))]
    );

    test_tokens!(
        decl_ident,
        r#"#foo"#,
        [(0..4, 0, 0, TokenKind::Ident("foo", IdentOptions::DECL))]
    );

    test_tokens!(
        all_ident,
        r#"_#1foo"#,
        [(
            0..6,
            0,
            0,
            TokenKind::Ident("1foo", IdentOptions::HIDDEN | IdentOptions::DECL)
        )]
    );

    test_tokens!(
        string_escape_unicode,
        r#""test \u{7FFF}""#,
        [(0..15, 0, 0, TokenKind::String("test \u{7FFF}"))]
    );

    test_tokens!(
        string_escape_hex,
        r#""test \x61""#,
        [(0..11, 0, 0, TokenKind::String("test a"))]
    );

    test_tokens!(
        string_escape_hex_bad,
        r#""test \x61\x1Z""#,
        [
            (
                11..15,
                0,
                11,
                TokenKind::Error(ErrorKind::BadEscapeSequence)
            ),
            (0..15, 0, 0, TokenKind::String("test a"))
        ]
    );

    test_tokens!(
        string_escape_interpolate,
        r#""foo\(bar)test""#,
        [
            (0..15, 0, 0, TokenKind::String("foo")),
            (6..15, 0, 6, TokenKind::Ident("bar", IdentOptions::empty())),
            (10..15, 0, 10, TokenKind::String("test"))
        ]
    );

    test_tokens!(
        number_hex,
        r#"0xBad_Cafe"#,
        [(0..10, 0, 0, TokenKind::Integer(0xBAD_CAFE))]
    );

    test_tokens!(
        number_hex_k,
        r#"0xBadCafeK"#,
        [(0..10, 0, 0, TokenKind::Integer(0xBAD_CAFE * 1000))]
    );

    test_tokens!(
        number_hex_ki,
        r#"0xBadCafeKi"#,
        [(0..11, 0, 0, TokenKind::Integer(0xBAD_CAFE * 1024))]
    );

    test_tokens!(
        number_oct,
        r#"0o1234"#,
        [(0..6, 0, 0, TokenKind::Integer(0o1234))]
    );

    test_tokens!(
        number_bin,
        r#"0b0101_0101"#,
        [(0..11, 0, 0, TokenKind::Integer(0b0101_0101))]
    );

    test_tokens!(
        number_dec,
        r#"0_100"#,
        [(0..5, 0, 0, TokenKind::Integer(100))]
    );

    test_tokens!(
        number_dec_unit,
        r#"0_100Ki"#,
        [(0..7, 0, 0, TokenKind::Integer(100 * 1024))]
    );

    test_tokens!(
        number_dec_exp,
        r#"0_100e2"#,
        [(0..7, 0, 0, TokenKind::Floating(100e2))]
    );

    test_tokens!(
        number_dec_exp_neg,
        r#"0_100e-2"#,
        [(0..8, 0, 0, TokenKind::Floating(100e-2))]
    );

    test_tokens!(
        number_dec_exp_neg_dec,
        r#"0_100.123e-2"#,
        [(0..12, 0, 0, TokenKind::Floating(100.123e-2))]
    );

    test_tokens!(
        number_dec_exp_unit,
        r#"0_100.3e2Ki"#,
        [(0..11, 0, 0, TokenKind::Integer(10030 * 1024))]
    );

    test_tokens!(
        number_dec_exp_unit_case1,
        r#"0_100.e2Ki"#,
        [(0..10, 0, 0, TokenKind::Integer(10000 * 1024))]
    );

    test_tokens!(
        number_dec_exp_unit_case2,
        r#".2Ki"#,
        [(0..4, 0, 0, TokenKind::Integer(204))]
    );
}
