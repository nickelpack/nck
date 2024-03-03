use super::{tables, IdentOptions, Lexer, LocationRef, Scanner, Token, TokenKind, TokenLexer};

pub struct Ident<'src, 'bump> {
    start: LocationRef,
    scanner: Scanner<'src, 'bump>,
    options: IdentOptions,
}

fn is_start(c: char) -> bool {
    tables::derived_property::XID_Start(c) || c == '$' || c == '_'
}

fn is_continue(c: char) -> bool {
    tables::derived_property::XID_Continue(c) || c == '$' || c == '_'
}

impl<'src, 'bump> TokenLexer<'src, 'bump> for Ident<'src, 'bump> {
    fn lex(_: &Lexer<'src, 'bump>, mut scanner: Scanner<'src, 'bump>) -> Option<Self> {
        let start = scanner.location();

        let mut options = IdentOptions::empty();
        if scanner.match_start("_#") {
            options |= IdentOptions::DECL | IdentOptions::HIDDEN;
        } else if scanner.match_start("#") {
            options |= IdentOptions::DECL;
        }

        if scanner.nth_char(0) == Some('_') {
            options |= IdentOptions::HIDDEN;
        }

        dbg!(&scanner.remainder());
        scanner.advance_while(is_start, Some(1))?;
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

    fn accept(self, lexer: &mut Lexer<'src, 'bump>) {
        let result = TokenKind::Ident(self.scanner.alloc_str_here(self.start), self.options);
        lexer.token(self.scanner, [(self.start, result)].into_iter());
    }
}

#[cfg(test)]
mod test {
    use crate::{
        parser::lexer::{IdentOptions, TokenKind},
        test_lexer,
    };

    use super::Ident;

    test_lexer!(no_ident, Ident, r#"123"#);

    test_lexer!(simple_ident, Ident, r#"foo"#, 3, |bump| [(
        0..3,
        0,
        0,
        TokenKind::Ident(bump.alloc_str("foo"), IdentOptions::empty())
    )]);

    test_lexer!(hidden_ident, Ident, r#"_foo"#, 4, |bump| [(
        0..4,
        0,
        0,
        TokenKind::Ident(bump.alloc_str("_foo"), IdentOptions::HIDDEN)
    )]);

    test_lexer!(decl_ident, Ident, r#"#foo"#, 4, |bump| [(
        0..4,
        0,
        0,
        TokenKind::Ident(bump.alloc_str("#foo"), IdentOptions::DECL)
    )]);

    test_lexer!(hidden_decl_ident, Ident, r#"_#foo"#, 5, |bump| [(
        0..5,
        0,
        0,
        TokenKind::Ident(
            bump.alloc_str("_#foo"),
            IdentOptions::HIDDEN | IdentOptions::DECL
        )
    )]);

    test_lexer!(hidden_decl_ident_2, Ident, r#"#_foo"#, 5, |bump| [(
        0..5,
        0,
        0,
        TokenKind::Ident(
            bump.alloc_str("#_foo"),
            IdentOptions::HIDDEN | IdentOptions::DECL
        )
    )]);
}
