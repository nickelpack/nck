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
    fn make_token(self, bump: &Bump, options: StringOptions) -> TokenKind<'_>;
    fn push_slice(&mut self, val: &str);
    fn push_char(&mut self, val: char);
    #[inline(always)]
    fn read_escape(st: &mut StringLexeme<Self>, c: char, escape_start: LocationRef) -> bool {
        false
    }
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
    fn make_token(self, bump: &Bump, options: StringOptions) -> TokenKind<'_> {
        TokenKind::String(bump.alloc_str(&self), options)
    }

    #[inline(always)]
    fn push_slice(&mut self, val: &str) {
        self.push_str(val)
    }

    #[inline(always)]
    fn push_char(&mut self, val: char) {
        self.push(val)
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
    fn make_token(self, bump: &Bump, options: StringOptions) -> TokenKind<'_> {
        TokenKind::Bytes(bump.alloc_slice_copy(&self), options)
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
}

pub struct StringLexeme<'src, 'bump, T: StrType> {
    start: LocationRef,
    scanner: Scanner<'src, 'bump>,
    result: T,
    next: Option<Scope<'src>>,
    errors: Vec<(LocationRef, ErrorKind)>,
    options: StringOptions,
}

impl<'src, 'bump, T: StrType> TokenLexer<'src, 'bump> for StringLexeme<'src, 'bump, T> {
    fn lex(lexer: &Lexer<'src, 'bump>, mut scanner: Scanner<'src, 'bump>) -> Option<Self> {
        let mut result = Self {
            start: scanner.location(),
            scanner,
            result: T::default(),
            next: None,
            errors: Vec::new(),
            options: StringOptions::empty(),
        };

        let hashes = result
            .scanner
            .advance_while(|c| c == '#', None)
            .unwrap_or("");

        match (hashes, result.scanner.nth_char(0), lexer.peek_scope()) {
            ("", Some(')'), Some(Scope::Interpolation { options, hashes }))
                if T::matches(options) && result.scanner.advance_char().is_some() =>
            {
                result.lex_impl(hashes, options)
            }
            (hashes, Some('"' | '\''), _)
                if result
                    .scanner
                    .match_start(T::delimiter(InterpOptions::MULTI)) =>
            {
                result.options |= StringOptions::OPEN;
                result.lex_impl(hashes, InterpOptions::MULTI | T::FLAG)
            }
            (hashes, Some('"' | '\''), _) if result.scanner.advance_char().is_some() => {
                result.options |= StringOptions::OPEN;
                result.lex_impl(hashes, T::FLAG)
            }
            _ => None,
        }
    }

    fn is_error(&self) -> bool {
        !self.errors.is_empty()
    }

    fn accept(self, lexer: &mut super::Lexer<'src, 'bump>) {
        let tok = self.result.make_token(self.scanner.bump(), self.options);
        lexer.error(&self.scanner, self.errors.into_iter());
        lexer.token(self.scanner, [(self.start, tok)].into_iter());
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
                    '\n' if !allow_newline => {
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
            'a' if self.scanner.advance_char().is_some() => self.result.push_char('\x07'),
            'b' if self.scanner.advance_char().is_some() => self.result.push_char('\x08'),
            'f' if self.scanner.advance_char().is_some() => self.result.push_char('\x0C'),
            'n' if self.scanner.advance_char().is_some() => self.result.push_char('\n'),
            'r' if self.scanner.advance_char().is_some() => self.result.push_char('\r'),
            't' if self.scanner.advance_char().is_some() => self.result.push_char('\t'),
            'v' if self.scanner.advance_char().is_some() => self.result.push_char('\x0B'),
            '/' if self.scanner.advance_char().is_some() => self.result.push_char('/'),
            '\\' if self.scanner.advance_char().is_some() => self.result.push_char('\\'),
            _ if c == quote => {
                self.scanner.advance_char();
                self.result.push_char(quote);
            }
            'u' | 'U' => {
                self.scanner.advance_char();
                let len = if (c == 'u') { 4 } else { 8 };

                // Don't immediately advance in case an invalid sequence contains the string end
                if let Some(val) = self.scanner.get_chars(len) {
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
    use bumpalo::Bump;

    use crate::parser::lexer::{
        test::{make_error, make_token, test_lexer},
        ErrorKind, IdentOptions, StringOptions, TokenKind,
    };

    use super::{BytesStr, CharStr};
    use pretty_assertions::assert_eq;

    #[test]
    fn string_none() {
        let bump = Bump::new();
        let r = test_lexer::<CharStr>(r#"123"#, &bump);
        assert_eq!(r, None);
    }

    #[test]
    fn string_simple() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<CharStr>(r#""foo\a\b\f\n\r\t\v\/\\\uFEEE\U00010000""#, bump).unwrap();
        assert_eq!(
            r,
            (
                39,
                vec![make_token(
                    bump,
                    0..39,
                    0,
                    0,
                    TokenKind::String(
                        bump.alloc_str("foo\x07\x08\x0C\n\r\t\x0B/\\\u{FEEE}\u{10000}"),
                        StringOptions::OPEN | StringOptions::CLOSE
                    )
                )],
                vec![]
            )
        )
    }

    #[test]
    fn string_simple_multi() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<CharStr>(
            r#""""
            foo\a\b\f\n
            \r\t\v\/\\\uFEEE\U00010000
            """"#,
            bump,
        )
        .unwrap();
        assert_eq!(
            r,
            (
                82,
                vec![make_token(
                    bump,
                    0..82,
                    0,
                    0,
                    TokenKind::String(
                        bump.alloc_str(
                            "\n            foo\u{7}\u{8}\u{c}\n\n            \r\t\u{b}/\\ﻮ𐀀\n            "
                        ),
                        StringOptions::OPEN | StringOptions::CLOSE
                    )
                )],
                vec![]
            )
        )
    }

    #[test]
    fn string_bad_escape() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<CharStr>(r#""foo\z\uhell\UFEEEFEEE\ua"b"#, bump).unwrap();
        assert_eq!(
            r,
            (
                26,
                vec![make_token(
                    bump,
                    0..26,
                    0,
                    0,
                    TokenKind::String(
                        bump.alloc_str("foozhellFEEEFEEEa"),
                        StringOptions::OPEN | StringOptions::CLOSE
                    )
                )],
                vec![
                    make_error(bump, 4..5, 0, 4, ErrorKind::BadEscapeSequence),
                    make_error(bump, 6..8, 0, 6, ErrorKind::BadEscapeSequence),
                    make_error(bump, 12..14, 0, 12, ErrorKind::BadEscapeSequence),
                    make_error(bump, 22..24, 0, 22, ErrorKind::BadEscapeSequence),
                ]
            )
        )
    }

    #[test]
    fn bytes_none() {
        let bump = Bump::new();
        let r = test_lexer::<BytesStr>(r#"123"#, &bump);
        assert_eq!(r, None);
    }

    #[test]
    fn bytes_simple() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<BytesStr>(r#"'foo\a\b\f\n\r\t\v\/\\\uFEEE\U00010000\xDE\xae'"#, bump)
            .unwrap();
        assert_eq!(
            r,
            (
                47,
                vec![make_token(
                    bump,
                    0..47,
                    0,
                    0,
                    TokenKind::Bytes(
                        bump.alloc_slice_copy(
                            b"foo\x07\x08\x0C\n\r\t\x0B/\\\xEF\xBB\xAE\xF0\x90\x80\x80\xde\xae"
                        ),
                        StringOptions::OPEN | StringOptions::CLOSE
                    )
                )],
                vec![]
            )
        )
    }

    #[test]
    fn bytes_bad_escape() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<BytesStr>(r#"'foo\z\uhell\UFEEEFEEE\ua\xe'b"#, bump).unwrap();
        assert_eq!(
            r,
            (
                29,
                vec![make_token(
                    bump,
                    0..29,
                    0,
                    0,
                    TokenKind::Bytes(
                        bump.alloc_slice_copy(b"foozhellFEEEFEEEae"),
                        StringOptions::OPEN | StringOptions::CLOSE
                    )
                )],
                vec![
                    make_error(bump, 4..5, 0, 4, ErrorKind::BadEscapeSequence),
                    make_error(bump, 6..8, 0, 6, ErrorKind::BadEscapeSequence),
                    make_error(bump, 12..14, 0, 12, ErrorKind::BadEscapeSequence),
                    make_error(bump, 22..24, 0, 22, ErrorKind::BadEscapeSequence),
                    make_error(bump, 25..27, 0, 25, ErrorKind::BadEscapeSequence),
                ]
            )
        )
    }
}
