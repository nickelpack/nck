use bumpalo::Bump;

use super::{
    ErrorKind, InterpolationExtent, InterpolationUnit, Lexer, LocationRef, Scanner, Scope,
    TokenKind, TokenLexer,
};

pub type CharStr<'src, 'bump> = Str<'src, 'bump, String>;
pub type BytesStr<'src, 'bump> = Str<'src, 'bump, Vec<u8>>;

pub trait StrType: Default {
    const UNIT: InterpolationUnit;
    fn delimiter(extent: InterpolationExtent) -> &'static str;
    fn make_token(self, bump: &Bump) -> TokenKind<'_>;
    fn push_slice(&mut self, val: &str);
    fn push_char(&mut self, val: char);
    #[inline(always)]
    fn read_escape(st: &mut Str<Self>, c: char, escape_start: LocationRef) -> bool {
        false
    }
}

impl StrType for String {
    const UNIT: InterpolationUnit = InterpolationUnit::Char;

    #[inline(always)]
    fn delimiter(extent: InterpolationExtent) -> &'static str {
        match extent {
            InterpolationExtent::Line => r#"""#,
            InterpolationExtent::Multi => r#"""""#,
        }
    }

    #[inline(always)]
    fn make_token(self, bump: &Bump) -> TokenKind<'_> {
        TokenKind::String(bump.alloc_str(&self))
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
    const UNIT: InterpolationUnit = InterpolationUnit::Byte;

    #[inline(always)]
    fn delimiter(extent: InterpolationExtent) -> &'static str {
        match extent {
            InterpolationExtent::Line => r#"'"#,
            InterpolationExtent::Multi => r#"'''"#,
        }
    }

    #[inline(always)]
    fn make_token(self, bump: &Bump) -> TokenKind<'_> {
        TokenKind::Bytes(bump.alloc_slice_copy(&self))
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

    fn read_escape(st: &mut Str<Self>, c: char, escape_start: LocationRef) -> bool {
        if c != 'x' {
            return false;
        }

        st.scanner.advance_by_chars(1);

        // Don't immediately advance in case an invalid sequence contains the string end
        if let Some(chars) = st.scanner.get_chars(2) {
            if let Ok(val) = u8::from_str_radix(chars, 16) {
                st.result.push(val);
                st.scanner.match_start(chars);
                return true;
            }
        }

        false
    }
}

pub struct Str<'src, 'bump, T: StrType> {
    start: LocationRef,
    scanner: Scanner<'src, 'bump>,
    result: T,
    next: Option<Scope<'src>>,
    errors: Vec<(LocationRef, ErrorKind)>,
}

impl<'src, 'bump, T: StrType> TokenLexer<'src, 'bump> for Str<'src, 'bump, T> {
    fn lex(lexer: &Lexer<'src, 'bump>, mut scanner: Scanner<'src, 'bump>) -> Option<Self> {
        let start = scanner.location();
        let hashes = scanner.advance_while(|c| c == '#', None).unwrap_or("");
        match (hashes, scanner.nth_char(0), lexer.peek_scope()) {
            (
                "",
                Some(')'),
                Some(Scope::Interpolation {
                    unit,
                    extent,
                    hashes,
                }),
            ) if unit == T::UNIT && scanner.match_start(")") => {
                Self::lex_common(lexer, scanner, start, hashes, extent)
            }
            (hashes, Some('"' | '\''), _)
                if scanner.match_start(T::delimiter(InterpolationExtent::Multi)) =>
            {
                Self::lex_common(lexer, scanner, start, hashes, InterpolationExtent::Multi)
            }
            (hashes, Some('"' | '\''), _)
                if scanner.match_start(T::delimiter(InterpolationExtent::Line)) =>
            {
                Self::lex_common(lexer, scanner, start, hashes, InterpolationExtent::Line)
            }
            _ => None,
        }
    }

    fn is_error(&self) -> bool {
        !self.errors.is_empty()
    }

    fn accept(self, lexer: &mut super::Lexer<'src, 'bump>) {
        let tok = self.result.make_token(self.scanner.bump());
        let tokens = self
            .errors
            .into_iter()
            .map(|(l, e)| (l, super::TokenKind::Error(e)))
            .chain([(self.start, tok)]);
        lexer.token(self.scanner, tokens);
        if let Some(next) = self.next {
            lexer.push_scope(next)
        }
    }
}

impl<'src, 'bump, T: StrType> Str<'src, 'bump, T> {
    fn lex_common(
        lexer: &Lexer<'src, 'bump>,
        mut scanner: Scanner<'src, 'bump>,
        location: LocationRef,
        hashes: &'src str,
        extent: InterpolationExtent,
    ) -> Option<Self> {
        let delim = T::delimiter(extent);
        let allow_newline = extent == InterpolationExtent::Multi;

        let mut result = Self {
            start: location,
            scanner,
            result: T::default(),
            next: None,
            errors: Vec::new(),
        };
        result.lex_on_self(location, hashes, delim, allow_newline)
    }

    fn lex_on_self(
        mut self,
        mut start: LocationRef,
        hashes: &'src str,
        delim: &str,
        allow_newline: bool,
    ) -> Option<Self> {
        let quote = delim.chars().next().unwrap();
        while let Some(c) = self.scanner.nth_char(0) {
            while let Some(c) = self.scanner.nth_char(0) {
                match c {
                    '\\' if self.scanner.match_start(hashes) => {
                        self.take_string_escape(&mut start, quote);
                    }
                    quote if self.scanner.match_start(delim) => {
                        if self.scanner.match_start(hashes) {
                            return Some(self);
                        }
                        self.result.push_slice(delim);
                    }
                    c => {
                        self.result.push_char(c);
                        self.scanner.advance_by_chars(1);
                    }
                }
            }
        }
        None
    }

    fn take_string_escape(&mut self, start: &mut LocationRef, quote: char) {
        let escape_start = self.scanner.location();
        self.scanner.match_start("\\");

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
            'a' if self.scanner.match_start("a") => self.result.push_char('\x07'),
            'b' if self.scanner.match_start("b") => self.result.push_char('\x08'),
            'f' if self.scanner.match_start("f") => self.result.push_char('\x0C'),
            'n' if self.scanner.match_start("n") => self.result.push_char('\n'),
            'r' if self.scanner.match_start("r") => self.result.push_char('\r'),
            't' if self.scanner.match_start("t") => self.result.push_char('\t'),
            'v' if self.scanner.match_start("v") => self.result.push_char('\x0B'),
            '/' if self.scanner.match_start("/") => self.result.push_char('/'),
            '\\' if self.scanner.match_start("\\") => self.result.push_char('\\'),
            _ if c == quote => {
                self.scanner.advance_by_chars(1);
                self.result.push_char(quote);
            }
            'u' | 'U' => {
                self.scanner.advance_by_chars(1);
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
            self.scanner.match_start(val);
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
        ErrorKind, IdentOptions, TokenKind,
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
                        bump.alloc_str("foo\x07\x08\x0C\n\r\t\x0B/\\\u{FEEE}\u{10000}")
                    )
                )]
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
                39,
                vec![make_token(
                    bump,
                    0..39,
                    0,
                    0,
                    TokenKind::String(
                        bump.alloc_str("foo\x07\x08\x0C\n\r\t\x0B/\\\u{FEEE}\u{10000}")
                    )
                )]
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
                vec![
                    make_error(bump, 4..5, 0, 4, ErrorKind::BadEscapeSequence),
                    make_error(bump, 6..8, 0, 6, ErrorKind::BadEscapeSequence),
                    make_error(bump, 12..14, 0, 12, ErrorKind::BadEscapeSequence),
                    make_error(bump, 22..24, 0, 22, ErrorKind::BadEscapeSequence),
                    make_token(
                        bump,
                        0..26,
                        0,
                        0,
                        TokenKind::String(bump.alloc_str("foozhellFEEEFEEEa"))
                    )
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
                    TokenKind::Bytes(bump.alloc_slice_copy(
                        b"foo\x07\x08\x0C\n\r\t\x0B/\\\xEF\xBB\xAE\xF0\x90\x80\x80\xde\xae"
                    ))
                )]
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
                vec![
                    make_error(bump, 4..5, 0, 4, ErrorKind::BadEscapeSequence),
                    make_error(bump, 6..8, 0, 6, ErrorKind::BadEscapeSequence),
                    make_error(bump, 12..14, 0, 12, ErrorKind::BadEscapeSequence),
                    make_error(bump, 22..24, 0, 22, ErrorKind::BadEscapeSequence),
                    make_error(bump, 25..27, 0, 25, ErrorKind::BadEscapeSequence),
                    make_token(
                        bump,
                        0..29,
                        0,
                        0,
                        TokenKind::Bytes(bump.alloc_slice_copy(b"foozhellFEEEFEEEae"))
                    )
                ]
            )
        )
    }
}
