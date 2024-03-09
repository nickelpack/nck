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
    #[inline(always)]
    fn read_escape(st: &mut StringLexeme<Self>, c: char, escape_start: LocationRef) -> bool {
        false
    }
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

    fn read_escape(st: &mut StringLexeme<Self>, c: char, escape_start: LocationRef) -> bool {
        if c != 'x' {
            return false;
        }

        st.scanner.advance_char();

        // Don't immediately advance in case an invalid sequence contains the string end
        if let Some(chars) = st.scanner.get_chars(2) {
            if let Ok(val) = u8::from_str_radix(chars, 16) {
                st.result.push(val);
                st.scanner.advance_by_bytes(chars.len());
                return true;
            }
        }

        false
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

        let hashes = result
            .scanner
            .advance_while(|c| c == '#', None)
            .unwrap_or("");

        match (hashes, result.scanner.nth_char(0)?, lexer.peek_scope()) {
            ("", ')', Some(Scope::Interpolation { options, hashes }))
                if T::matches(options) && result.scanner.advance_char().is_some() =>
            {
                result.lex_impl(hashes, options)
            }
            (hashes, '"' | '\'', _)
                if result
                    .scanner
                    .match_start(T::delimiter(InterpOptions::MULTI)) =>
            {
                result.options |= StringOptions::OPEN;
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
                    options.remove(StringOptions::OPEN);
                }
                if !last {
                    options.remove(StringOptions::CLOSE);
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
                match c {
                    '\\' if self.scanner.match_start(hashes) => {
                        self.take_string_escape(quote);
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

    fn take_string_escape(&mut self, quote: char) {
        let escape_start = self.scanner.location();
        self.scanner.advance_char();

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
            '(' => {
                todo!();
            }
            _ if T::read_escape(self, c, escape_start) => {}
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
        parser::lexer::{ErrorKind, StringOptions, TokenKind},
        test_lexer,
    };

    use super::{BytesStr, CharStr};
    use pretty_assertions::assert_eq;

    test_lexer!(no_string, CharStr, r#"123"#);

    test_lexer!(
        simple_string,
        CharStr,
        r#""foo\n\r\t\\\u{FEEE}\u{00010000}""#,
        33,
        |bump| [(
            0..33,
            0,
            0,
            TokenKind::String(
                bump.alloc_str("foo\n\r\t\\\u{FEEE}\u{10000}"),
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
                TokenKind::String(bump.alloc_str(""), StringOptions::OPEN)
            ),
            (
                4..22, 1, 0,
                TokenKind::String(
                    bump.alloc_str("            foo\n"), StringOptions::empty(),
                )
            ),
            (
                22..61, 2,
                0,
                TokenKind::String(
                    bump.alloc_str("            \r\t\\\u{FEEE}\u{10000}"), StringOptions::empty()
                )
            ),
            (
                61..76, 3, 0,
                TokenKind::String(
                    bump.alloc_str("            "), StringOptions::CLOSE
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
