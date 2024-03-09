use std::ops::Range;

use bitflags::Flags;
use bumpalo::Bump;

use super::{
    ErrorKind, InterpOptions, Lexer, LocationRef, Scanner, Scope, StringOptions, TokenKind,
    TokenLexer,
};

pub type CharStr<'src, 'bump> = StringLexeme<'src, 'bump, String>;
pub type BytesStr<'src, 'bump> = StringLexeme<'src, 'bump, Vec<u8>>;

pub trait StrType: Default {
    const FLAG: InterpOptions;
    fn matches(options: InterpOptions) -> bool;
    fn delimiter(options: InterpOptions) -> &'static str;
    fn make_token<'bump>(
        &self,
        range: Range<usize>,
        bump: &'bump Bump,
        options: StringOptions,
    ) -> TokenKind<'bump>;
    fn push_slice(&mut self, val: &str);
    fn push_char(&mut self, val: char);
    fn push_byte(&mut self, val: u8) -> bool;
    fn len(&self) -> usize;
}

impl StrType for String {
    const FLAG: InterpOptions = InterpOptions::empty();

    #[inline(always)]
    fn matches(options: InterpOptions) -> bool {
        !options.contains(InterpOptions::BYTE)
    }

    #[inline(always)]
    fn delimiter(options: InterpOptions) -> &'static str {
        if options.contains(InterpOptions::MULTI) {
            r#"""""#
        } else {
            r#"""#
        }
    }

    #[inline(always)]
    fn make_token<'bump>(
        &self,
        range: Range<usize>,
        bump: &'bump Bump,
        options: StringOptions,
    ) -> TokenKind<'bump> {
        TokenKind::String(bump.alloc_str(&self[range]), options)
    }

    #[inline(always)]
    fn push_slice(&mut self, val: &str) {
        self.push_str(val)
    }

    #[inline(always)]
    fn push_char(&mut self, val: char) {
        self.push(val)
    }

    #[inline(always)]
    fn push_byte(&mut self, val: u8) -> bool {
        if let Some(c) = char::from_u32(val as u32) {
            self.push_char(c);
            true
        } else {
            false
        }
    }

    #[inline(always)]
    fn len(&self) -> usize {
        String::len(self)
    }
}

impl StrType for Vec<u8> {
    const FLAG: InterpOptions = InterpOptions::BYTE;

    #[inline(always)]
    fn matches(options: InterpOptions) -> bool {
        options.contains(InterpOptions::BYTE)
    }

    #[inline(always)]
    fn delimiter(options: InterpOptions) -> &'static str {
        if options.contains(InterpOptions::MULTI) {
            r#"'''"#
        } else {
            r#"'"#
        }
    }

    #[inline(always)]
    fn make_token<'bump>(
        &self,
        range: Range<usize>,
        bump: &'bump Bump,
        options: StringOptions,
    ) -> TokenKind<'bump> {
        TokenKind::Bytes(bump.alloc_slice_copy(&self[range]), options)
    }

    #[inline(always)]
    fn push_slice(&mut self, val: &str) {
        self.extend_from_slice(val.as_bytes())
    }

    #[inline(always)]
    fn push_char(&mut self, val: char) {
        let mut buffer = [0u8; 4];
        self.push_slice(val.encode_utf8(&mut buffer))
    }

    #[inline(always)]
    fn push_byte(&mut self, val: u8) -> bool {
        self.push(val);
        true
    }

    #[inline(always)]
    fn len(&self) -> usize {
        Vec::len(self)
    }
}

pub struct StringLexeme<'src, 'bump, T: StrType> {
    start: LocationRef,
    scanner: Scanner<'src, 'bump>,
    result: T,
    next: Option<Scope<'src>>,
    errors: Vec<(LocationRef, ErrorKind)>,
    options: StringOptions,
    multi_locs: Vec<(usize, LocationRef)>,
}

impl<'src, 'bump, T: StrType> TokenLexer<'src, 'bump> for StringLexeme<'src, 'bump, T> {
    fn lex(lexer: &Lexer<'src, 'bump>, mut scanner: Scanner<'src, 'bump>) -> Option<Self> {
        let start = scanner.location();
        let mut result = Self {
            start,
            scanner,
            result: T::default(),
            next: None,
            errors: Vec::new(),
            options: StringOptions::empty(),
            multi_locs: vec![(0, start)],
        };

        let hashes = if result.scanner.remainder().starts_with("r#") {
            result.scanner.advance_char();
            result
                .scanner
                .advance_while(|c| c == '#', None)
                .unwrap_or("")
        } else {
            ""
        };

        match (hashes, result.scanner.nth_char(0)?, lexer.peek_scope()) {
            ("", ')', Some(Scope::Interpolation { options, hashes }))
                if T::matches(options) && result.scanner.advance_char().is_some() =>
            {
                if options.contains(InterpOptions::MULTI) {
                    result.options.insert(StringOptions::MULTI)
                }
                result.options.insert(StringOptions::EXIT);
                result.lex_impl(hashes, options)
            }
            (hashes, '"' | '\'', _)
                if result
                    .scanner
                    .match_start(T::delimiter(InterpOptions::MULTI)) =>
            {
                result.options |= StringOptions::OPEN | StringOptions::MULTI;
                result.lex_impl(hashes, InterpOptions::MULTI | T::FLAG)
            }
            (hashes, '"' | '\'', _) if result.scanner.advance_char().is_some() => {
                result.options |= StringOptions::OPEN;
                result.lex_impl(hashes, T::FLAG)
            }
            _ => None,
        }
    }

    fn is_error(&self) -> bool {
        !self.errors.is_empty()
    }

    fn accept(mut self, lexer: &mut super::Lexer<'src, 'bump>) {
        lexer.error(&self.scanner, self.errors.into_iter());

        self.multi_locs
            .push((self.result.len(), self.scanner.location()));

        let last_index = self.multi_locs.len() - 2;
        let bump = self.scanner.bump();
        let toks = self
            .multi_locs
            .windows(2)
            .enumerate()
            .map(|(i, window)| {
                (
                    i == 0,
                    i == last_index,
                    window[0].0,
                    window[0].1,
                    window[1].0,
                    window[1].1,
                )
            })
            .map(|(first, last, prev_index, prev_loc, cur_index, cur_loc)| {
                let mut options = self.options;
                if !first {
                    options.remove(StringOptions::OPEN | StringOptions::EXIT);
                }
                if !last {
                    options.remove(StringOptions::CLOSE | StringOptions::ENTER);
                };
                let tok = self.result.make_token(prev_index..cur_index, bump, options);
                (prev_loc.with_end_from(cur_loc), tok)
            });

        lexer.token(self.scanner, toks);
        if let Some(next) = self.next {
            lexer.push_scope(next)
        }
    }
}

impl<'src, 'bump, T: StrType> StringLexeme<'src, 'bump, T> {
    fn lex_impl(mut self, hashes: &'src str, options: InterpOptions) -> Option<Self> {
        let delim = T::delimiter(options);
        let allow_newline = options.contains(InterpOptions::MULTI);

        let quote = delim.chars().next().unwrap();
        while let Some(c) = self.scanner.nth_char(0) {
            while let Some(c) = self.scanner.nth_char(0) {
                let start = self.scanner.location();
                match c {
                    '\\' if self.scanner.remainder()[1..].starts_with(hashes) => {
                        self.scanner.advance_char();
                        self.scanner.advance_by_bytes(hashes.len());
                        if self.scanner.match_start("(") {
                            self.options |= StringOptions::ENTER;
                            self.next = Some(Scope::Interpolation { options, hashes });
                            return Some(self);
                        }
                        self.take_string_escape(quote, start);
                    }
                    '\'' | '"' if self.scanner.match_start(delim) => {
                        if self.scanner.match_start(hashes) {
                            self.options |= StringOptions::CLOSE;
                            return Some(self);
                        }
                        self.result.push_slice(delim);
                    }
                    '\r' => {
                        self.scanner.advance_char();
                    }
                    '\n' if allow_newline => {
                        // Lines are split for simpler parsing
                        self.scanner.advance_char();
                        self.multi_locs
                            .push((self.result.len(), self.scanner.location()));
                    }
                    '\n' => {
                        self.errors
                            .push((self.scanner.location(), ErrorKind::NewLineInString));
                    }
                    c => {
                        self.result.push_char(c);
                        self.scanner.advance_char();
                    }
                }
            }
        }
        None
    }

    fn take_string_escape(&mut self, quote: char, escape_start: LocationRef) {
        let c = if let Some(c) = self.scanner.nth_char(0) {
            c
        } else {
            self.errors.push((
                escape_start.with_end_from(self.scanner.location()),
                ErrorKind::BadEscapeSequence,
            ));
            return;
        };

        match c {
            'n' if self.scanner.advance_char().is_some() => self.result.push_char('\n'),
            'r' if self.scanner.advance_char().is_some() => self.result.push_char('\r'),
            't' if self.scanner.advance_char().is_some() => self.result.push_char('\t'),
            '\\' if self.scanner.advance_char().is_some() => self.result.push_char('\\'),
            '0' if self.scanner.advance_char().is_some() => self.result.push_char('\0'),
            '#' if self.scanner.advance_char().is_some() => self.result.push_char('#'),
            _ if c == quote => {
                self.scanner.advance_char();
                self.result.push_char(quote);
            }
            'u' if self.scanner.advance_char().is_some() => {
                if !self.scanner.match_start("{") {
                    self.errors.push((
                        escape_start.with_end_from(self.scanner.location()),
                        ErrorKind::BadEscapeSequence,
                    ));
                    return;
                }

                let mut index = None;
                for (i, c) in self.scanner.remainder().char_indices() {
                    match c {
                        '0'..='9' | 'a'..='f' | 'A'..='F' => {}
                        '}' => index = Some(i),
                        _ => break,
                    }
                }

                // Don't immediately advance in case an invalid sequence contains the string end
                if let Some(val) = index.and_then(|index| self.scanner.get_chars(index)) {
                    self.scanner.advance_char(); // '}'
                    self.parse_unicode(escape_start, val)
                } else {
                    self.errors.push((
                        escape_start.with_end_from(self.scanner.location()),
                        ErrorKind::BadEscapeSequence,
                    ))
                }
            }
            'x' if self.scanner.advance_char().is_some() => {
                // Don't immediately advance in case an invalid sequence contains the string end
                if let Some(chars) = self.scanner.get_chars(2) {
                    if let Ok(val) = u8::from_str_radix(chars, 16) {
                        if self.result.push_byte(val) {
                            self.scanner.advance_by_bytes(chars.len());
                            return;
                        }
                    }
                }
                self.errors.push((
                    escape_start.with_end_from(self.scanner.location()),
                    ErrorKind::BadEscapeSequence,
                ))
            }
            _ => self.errors.push((
                escape_start.with_end_from(self.scanner.location()),
                ErrorKind::BadEscapeSequence,
            )),
        }
    }

    fn parse_unicode(&mut self, start: LocationRef, val: &'src str) {
        if let Some(chr) = u32::from_str_radix(val, 16).ok().and_then(char::from_u32) {
            self.result.push_char(chr);
            self.scanner.advance_by_bytes(val.len());
        } else {
            self.errors.push((
                start.with_end_from(self.scanner.location()),
                ErrorKind::BadEscapeSequence,
            ));
        }
    }
}

#[cfg(test)]
mod test {
    use crate::{
        parser::{
            lexer::{ErrorKind, Lexer, StringOptions, Token, TokenKind, TokenLexer as _},
            Location,
        },
        test_lexer,
    };

    use super::{BytesStr, CharStr};
    use pretty_assertions::assert_eq;

    test_lexer!(no_string, CharStr, r#"123"#);

    test_lexer!(
        simple_string,
        CharStr,
        r#""foo\x05\n\r\t\\\u{FEEE}\u{00010000}""#,
        37,
        |bump| [(
            0..37,
            0,
            0,
            TokenKind::String(
                bump.alloc_str("foo\x05\n\r\t\\\u{FEEE}\u{10000}"),
                StringOptions::OPEN | StringOptions::CLOSE
            )
        )]
    );

    test_lexer!(
        verbatim_string,
        CharStr,
        r####"r###"foo\x05\#n\##r\###t\\\###u{FEEE}\u{00010000}\#####"###"####,
        59,
        |bump| [(
            0..59,
            0,
            0,
            TokenKind::String(
                bump.alloc_str("foo\\x05\\#n\\##r\t\\\\\u{FEEE}\\u{00010000}##"),
                StringOptions::OPEN | StringOptions::CLOSE
            )
        )]
    );

    #[rustfmt::skip]
    test_lexer!(
        simple_string_multi,
        CharStr,
        r#""""
            foo\n
            \r\t\\\u{FEEE}\u{00010000}
            """"#,
        76,
        |bump| [
            (
                0..4, 0, 0,
                TokenKind::String(bump.alloc_str(""), StringOptions::OPEN | StringOptions::MULTI)
            ),
            (
                4..22, 1, 0,
                TokenKind::String(
                    bump.alloc_str("            foo\n"), StringOptions::MULTI,
                )
            ),
            (
                22..61, 2,
                0,
                TokenKind::String(
                    bump.alloc_str("            \r\t\\\u{FEEE}\u{10000}"), StringOptions::MULTI
                )
            ),
            (
                61..76, 3, 0,
                TokenKind::String(
                    bump.alloc_str("            "), StringOptions::CLOSE | StringOptions::MULTI
                )
            )
        ]
    );

    test_lexer!(
        simple_bad_escape,
        CharStr,
        r#""foo\z\uhell\uFEEEFEEE\ua"b"#,
        26,
        |bump| [(
            0..26,
            0,
            0,
            TokenKind::String(
                bump.alloc_str("foozhellFEEEFEEEa"),
                StringOptions::OPEN | StringOptions::CLOSE
            )
        )],
        |bump| [
            (4..5, 0, 4, ErrorKind::BadEscapeSequence),
            (6..8, 0, 6, ErrorKind::BadEscapeSequence),
            (12..14, 0, 12, ErrorKind::BadEscapeSequence),
            (22..24, 0, 22, ErrorKind::BadEscapeSequence)
        ]
    );

    #[test]
    fn interp_string_multi() {
        let bump = bumpalo::Bump::new();
        let bump = &bump;
        let path = std::path::PathBuf::from("test.cue");
        let mut lexer = Lexer::new(
            r#""""
            foo\n\("test")
            \r\t\\\u{FEEE}\u{00010000}
            """"#,
            &path,
            bump,
        );
        for _ in 0..3 {
            lexer
                .lex::<CharStr>()
                .map(|r| {
                    r.accept(&mut lexer);
                })
                .expect("successfully lexes");
        }
        assert_eq!(
            lexer.tokens,
            vec![
                Token {
                    kind: (TokenKind::String(
                        bump.alloc_str(""),
                        StringOptions::OPEN | StringOptions::MULTI
                    )),
                    location: Location::new_in((0..4), 0, 0, &path, bump),
                },
                Token {
                    kind: (TokenKind::String(
                        bump.alloc_str("            foo\n"),
                        StringOptions::ENTER | StringOptions::MULTI,
                    )),
                    location: Location::new_in((4..23), 1, 0, &path, bump),
                },
                Token {
                    kind: (TokenKind::String(
                        bump.alloc_str("test"),
                        StringOptions::OPEN | StringOptions::CLOSE
                    )),
                    location: Location::new_in((23..29), 1, 19, &path, bump),
                },
                Token {
                    kind: (TokenKind::String(
                        bump.alloc_str(""),
                        StringOptions::EXIT | StringOptions::MULTI
                    )),
                    location: Location::new_in((29..31), 1, 25, &path, bump),
                },
                Token {
                    kind: (TokenKind::String(
                        bump.alloc_str("            \r\t\\\u{FEEE}\u{10000}"),
                        StringOptions::MULTI
                    )),
                    location: Location::new_in((31..70), 2, 0, &path, bump),
                },
                Token {
                    kind: (TokenKind::String(
                        bump.alloc_str("            "),
                        StringOptions::CLOSE | StringOptions::MULTI
                    )),
                    location: Location::new_in((70..85), 3, 0, &path, bump),
                }
            ],
            "the tokens match",
        );

        assert_eq!(
            lexer.scanner.offset, 85,
            "the lexer end position must match",
        );
    }

    test_lexer!(no_bytes, BytesStr, r#"123"#);

    test_lexer!(
        bytes_simple,
        BytesStr,
        r#"'foo\n\r\t\\\u{FEEE}\u{00010000}\xDE\xae'"#,
        41,
        |bump| [(
            0..41,
            0,
            0,
            TokenKind::Bytes(
                bump.alloc_slice_copy(b"foo\n\r\t\\\xEF\xBB\xAE\xF0\x90\x80\x80\xde\xae"),
                StringOptions::OPEN | StringOptions::CLOSE
            )
        )]
    );

    test_lexer!(
        bytes_bad_escape,
        BytesStr,
        r#"'foo\z\uhell\uFEEEFEEE\ua\xe'b"#,
        29,
        |bump| [(
            0..29,
            0,
            0,
            TokenKind::Bytes(
                bump.alloc_slice_copy(b"foozhellFEEEFEEEae"),
                StringOptions::OPEN | StringOptions::CLOSE
            )
        )],
        |bump| [
            (4..5, 0, 4, ErrorKind::BadEscapeSequence),
            (6..8, 0, 6, ErrorKind::BadEscapeSequence),
            (12..14, 0, 12, ErrorKind::BadEscapeSequence),
            (22..24, 0, 22, ErrorKind::BadEscapeSequence),
            (25..27, 0, 25, ErrorKind::BadEscapeSequence)
        ]
    );
}
