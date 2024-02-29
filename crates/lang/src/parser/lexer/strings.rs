use super::{
    ErrorKind, InterpolationExtent, InterpolationUnit, Lexer, LocationRef, Scanner, Scope,
    TokenLexer,
};

const DELIM_MULTI_CHAR: &str = r#"""""#;
const DELIM_MULTI_BYTE: &str = "'''";
const DELIM_LINE_CHAR: &str = r#"""#;
const DELIM_LINE_BYTE: &str = "'";

struct Str<'src, 'bump> {
    start: LocationRef,
    scanner: Scanner<'src, 'bump>,
    result: Result<String, ErrorKind>,
    next: Option<Scope<'src>>,
}

impl<'src, 'bump> TokenLexer<'src, 'bump> for Str<'src, 'bump> {
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
            ) if scanner.match_start(")") => {
                Self::lex_common(lexer, scanner, start, hashes, extent, unit)
            }
            (hashes, Some('"'), _) if scanner.match_start(DELIM_MULTI_CHAR) => Self::lex_common(
                lexer,
                scanner,
                start,
                hashes,
                InterpolationExtent::Multi,
                InterpolationUnit::Char,
            ),
            (hashes, Some('"'), _) if scanner.match_start(DELIM_LINE_CHAR) => Self::lex_common(
                lexer,
                scanner,
                start,
                hashes,
                InterpolationExtent::Line,
                InterpolationUnit::Char,
            ),
            (hashes, Some('\''), _) if scanner.match_start(DELIM_MULTI_BYTE) => Self::lex_common(
                lexer,
                scanner,
                start,
                hashes,
                InterpolationExtent::Multi,
                InterpolationUnit::Byte,
            ),
            (hashes, Some('\''), _) if scanner.match_start(DELIM_LINE_BYTE) => Self::lex_common(
                lexer,
                scanner,
                start,
                hashes,
                InterpolationExtent::Line,
                InterpolationUnit::Byte,
            ),
            _ => None,
        }
    }

    fn is_error(&self) -> bool {
        self.result.is_err()
    }

    fn accept(
        self,
        lexer: &mut super::Lexer<'src, 'bump>,
    ) -> Result<super::Token<'bump>, super::Token<'bump>> {
        match self.result {
            Ok(s) => {
                let st = self.scanner.bump().alloc_str(&s);
                Ok(lexer.token(self.scanner, self.start, super::TokenKind::String(st)))
            }
            Err(_) => todo!(),
        }
    }
}

impl<'src, 'bump> Str<'src, 'bump> {
    fn lex_common(
        lexer: &Lexer<'src, 'bump>,
        mut scanner: Scanner<'src, 'bump>,
        location: LocationRef,
        hashes: &'src str,
        extent: InterpolationExtent,
        unit: InterpolationUnit,
    ) -> Option<Self> {
        let delim = match (extent, unit) {
            (InterpolationExtent::Line, InterpolationUnit::Char) => DELIM_LINE_CHAR,
            (InterpolationExtent::Line, InterpolationUnit::Byte) => DELIM_LINE_BYTE,
            (InterpolationExtent::Multi, InterpolationUnit::Char) => DELIM_MULTI_CHAR,
            (InterpolationExtent::Multi, InterpolationUnit::Byte) => DELIM_MULTI_BYTE,
        };
        let quote = delim.chars().next().unwrap();

        let allow_newline = extent == InterpolationExtent::Multi;
        let mut tmp = String::new();

        let mut start = location;
        while let Some(c) = scanner.nth_char(0) {
            while let Some(c) = scanner.nth_char(0) {
                match c {
                    '\\' if scanner.match_start(hashes) => {
                        Self::take_string_escape(&mut scanner, &mut start, &mut tmp, quote);
                    }
                    '\\' if hashes.is_empty() => {
                        todo!();
                    }
                    _ if scanner.match_start(delim) => {
                        return Some(Str {
                            start,
                            scanner,
                            result: Ok(tmp),
                            next: None,
                        })
                    }
                    c => {
                        tmp.push(c);
                        scanner.advance_by_chars(1);
                    }
                }
            }
        }
        None
    }

    fn take_string_escape(
        scanner: &mut Scanner<'src, 'bump>,
        start: &mut LocationRef,
        tmp: &mut String,
        quote: char,
    ) {
        let c = if let Some(c) = scanner.nth_char(0) {
            c
        } else {
            // self.error_at_offset(*start, ErrorKind::BadEscapeSequence);
            return;
        };

        match c {
            'a' if scanner.match_start("a") => tmp.push('\x07'),
            'b' if scanner.match_start("b") => tmp.push('\x08'),
            'f' if scanner.match_start("f") => tmp.push('\x0C'),
            'n' if scanner.match_start("n") => tmp.push('\n'),
            'r' if scanner.match_start("r") => tmp.push('\r'),
            't' if scanner.match_start("t'") => tmp.push('\t'),
            'v' if scanner.match_start("v") => tmp.push('\x0B'),
            '/' if scanner.match_start("/") => tmp.push('/'),
            '\\' if scanner.match_start("\\") => tmp.push('\\'),
            quote => {
                scanner.advance_by_chars(1);
                tmp.push(quote);
            }
            'u' | 'U' => {
                let escape = scanner.location();
                scanner.advance_by_chars(1);
                let len = if (c == 'u') { 4 } else { 8 };
                if let Some(val) = scanner.advance_by_chars(len) {
                    Self::parse_unicode(tmp, escape, val)
                } else {
                    todo!()
                }
            }
            '(' => {
                todo!();
            }
            _ => todo!(),
        }
    }

    fn parse_unicode(tmp: &mut String, start: LocationRef, val: &'src str) {
        if let Some(val) = u32::from_str_radix(val, 16).ok().and_then(char::from_u32) {
            tmp.push(val);
        } else {
            todo!();
        }
    }
}

#[cfg(test)]
mod test {
    use bumpalo::Bump;

    use crate::parser::lexer::{
        test::{make_token, test_lexer},
        IdentOptions, TokenKind,
    };

    use super::Str;
    use pretty_assertions::assert_eq;

    #[test]
    fn no_str() {
        let bump = Bump::new();
        let r = test_lexer::<Str>(r#"123"#, &bump);
        assert_eq!(r, None);
    }

    #[test]
    fn simple_ident() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Str>(r#""foo""#, bump).unwrap();
        assert_eq!(
            r,
            (
                5,
                make_token(bump, 0..5, 0, 0, TokenKind::String(bump.alloc_str("foo")))
            )
        )
    }
}
