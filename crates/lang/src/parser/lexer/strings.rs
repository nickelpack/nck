use super::{ErrorKind, Lexer, LocationRef, Scanner, Scope, TokenLexer};

struct Str<'src, 'bump> {
    start: LocationRef,
    scanner: Scanner<'src, 'bump>,
    result: Result<String, ErrorKind>,
}

impl<'src, 'bump> TokenLexer<'src, 'bump> for Str<'src, 'bump> {
    fn lex(lexer: &Lexer<'src, 'bump>, mut scanner: Scanner<'src, 'bump>) -> Option<Self> {
        let start = scanner.location();
        let hashes = scanner.advance_while(|c| c == '#', None).unwrap_or("");
        match (hashes, scanner.nth_char(0), lexer.peek_scope()) {
            ("", Some(')'), Some(Scope::Interpolation(unit, extent))) => todo!(),
            (hashes, Some('"'), _) if scanner.match_start(r#"""""#) => todo!(),
            (hashes, Some('"'), _) if scanner.match_start("\"") => todo!(),
            (hashes, Some('\''), _) if scanner.match_start("'''") => todo!(),
            (hashes, Some('\''), _) if scanner.match_start("'") => todo!(),
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
        todo!()
    }
}

#[cfg(never)]
mod never {
    impl<'src, 'bump> LexerFrame<'src, 'bump> {
        pub(super) fn take_string_or_ident(&mut self) {
            let start = self.loc;
            let mut count = 0;
            while self.match_start("#") {
                count += 1;
            }

            let c = if let Some(c) = self.nth_char(0) {
                c
            } else {
                self.error_at_offset(start, super::ErrorKind::UnterminatedString);
                return;
            };

            if self.match_start("\"\"\"") {
                self.take_string(count, "\"\"\"", MultiLineType::<str>::new(), StringCharType);
            } else if self.match_start("'''") {
                self.take_string(count, "'''", MultiLineType::<[u8]>::new(), BytesCharType);
            } else if self.match_start("\"") {
                self.take_string(count, "\"", SingleLineType::<str>::None, StringCharType);
            } else if self.match_start("'") {
                self.take_string(count, "'", SingleLineType::<[u8]>::None, BytesCharType);
            } else if tables::derived_property::XID_Start(c) && count < 2 {
                let options = if count == 0 {
                    IdentOptions::empty()
                } else {
                    IdentOptions::DECL
                };
                self.take_ident(options);
            } else {
                self.error_at_offset(start, ErrorKind::UnterminatedString);
            }
        }

        fn take_string(
            &mut self,
            pound_count: usize,
            terminator: &str,
            line: impl LineType<'src, 'bump>,
            ch: impl CharType<'src, 'bump>,
        ) {
            let mut start = self.loc;
            let quote = terminator.chars().next().unwrap();
            while let Some(c) = self.nth_char(0) {
                match c {
                    '\\' => {
                        if self
                            .remainder()
                            .chars()
                            .skip(1)
                            .take(pound_count)
                            .filter(|c| *c == '#')
                            .count()
                            == pound_count
                        {
                            self.advance_by(1 + pound_count);
                            self.take_string_escape(&mut start, quote);
                        }

                        self.take_string_escape(&mut start, quote);
                    }
                    _ if self.match_start(terminator) => {
                        self.string_token(start);
                        return;
                    }
                    c => {
                        self.string.push(c);
                        self.advance_by(1);
                    }
                }
            }
            self.error_at_offset(start, ErrorKind::UnterminatedString);
        }

        fn take_string_escape(&mut self, start: &mut LocationRef, quote: char) {
            let c = if let Some(c) = self.nth_char(0) {
                c
            } else {
                self.error_at_offset(*start, ErrorKind::BadEscapeSequence);
                return;
            };

            match c {
                'a' => {
                    self.advance_by(1);
                    self.string.push('\x07');
                }
                'b' => {
                    self.advance_by(1);
                    self.string.push('\x08');
                }
                'f' => {
                    self.advance_by(1);
                    self.string.push('\x0C');
                }
                'n' => {
                    self.advance_by(1);
                    self.string.push('\n');
                }
                'r' => {
                    self.advance_by(1);
                    self.string.push('\r');
                }
                't' => {
                    self.advance_by(1);
                    self.string.push('\t');
                }
                'v' => {
                    self.advance_by(1);
                    self.string.push('\x0B');
                }
                '/' => {
                    self.advance_by(1);
                    self.string.push('/');
                }
                '\\' => {
                    self.advance_by(1);
                    self.string.push('\\');
                }
                quote => {
                    self.advance_by(1);
                    self.string.push(quote);
                }
                'u' | 'U' => {
                    let escape = self.loc;
                    self.advance_by(1);
                    let len = (c == 'u').then_some(4).unwrap_or(8);
                    if let Some((_, val)) = self.advance_by(len) {
                        self.parse_unicode(escape, val)
                    } else {
                        self.error_at_offset(*start, ErrorKind::BadEscapeSequence)
                    }
                }
                'U' => {
                    let escape = self.loc;
                    self.advance_by(1);
                    if let Some((_, val)) = self.advance_by(8) {
                        self.parse_unicode(escape, val)
                    } else {
                        self.error_at_offset(*start, ErrorKind::BadEscapeSequence)
                    }
                }
                '(' => {
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
                self.string.push(val);
            } else {
                self.error_at_offset(start, ErrorKind::BadEscapeSequence);
            }
        }
    }
}
