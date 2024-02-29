use super::{tables, IdentOptions, Lexer, LocationRef, Scanner, Token, TokenKind, TokenLexer};

pub struct Ident<'src, 'bump> {
    start: LocationRef,
    scanner: Scanner<'src, 'bump>,
    options: IdentOptions,
}

impl<'src, 'bump> TokenLexer<'src, 'bump> for Ident<'src, 'bump> {
    fn lex(_: &Lexer<'src, 'bump>, mut scanner: Scanner<'src, 'bump>) -> Option<Self> {
        let start = scanner.location();

        let mut options = IdentOptions::empty();
        let has_start = scanner.match_start("_");
        if has_start {
            options |= IdentOptions::HIDDEN;
        }

        if scanner.match_start("#") {
            options |= IdentOptions::DECL;
        }

        let has_start = has_start
            || scanner
                .advance_while(tables::derived_property::XID_Start, Some(1))
                .is_some();

        if !has_start {
            return None;
        }

        scanner.advance_while(tables::derived_property::XID_Continue, None);

        Some(Self {
            start,
            scanner,
            options,
        })
    }

    fn is_error(&self) -> bool {
        false
    }

    fn accept(self, parent: &mut Lexer<'src, 'bump>) {
        let result = TokenKind::Ident(self.scanner.alloc_str_here(self.start), self.options);
        parent.token(self.scanner, [(self.start, result)].into_iter());
    }
}

#[cfg(test)]
mod test {
    use bumpalo::Bump;

    use crate::parser::lexer::{
        test::{make_token, test_lexer},
        IdentOptions, TokenKind,
    };

    use pretty_assertions::assert_eq;

    use super::Ident;

    #[test]
    fn no_ident() {
        let bump = Bump::new();
        let r = test_lexer::<Ident>(r#"123"#, &bump);
        assert_eq!(r, None);
    }

    #[test]
    fn simple_ident() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Ident>(r#"foo"#, bump).unwrap();
        assert_eq!(
            r,
            (
                3,
                vec![make_token(
                    bump,
                    0..3,
                    0,
                    0,
                    TokenKind::Ident(bump.alloc_str("foo"), IdentOptions::empty())
                )]
            )
        )
    }
}
