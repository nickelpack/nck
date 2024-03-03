use super::{ErrorKind, Lexer, LocationRef, Scanner, Token, TokenKind, TokenLexer};

pub struct Keyword<'src, 'bump> {
    start: LocationRef,
    scanner: Scanner<'src, 'bump>,
    value: TokenKind<'bump>,
}

impl<'src, 'bump> TokenLexer<'src, 'bump> for Keyword<'src, 'bump> {
    fn lex(_: &Lexer<'src, 'bump>, mut scanner: Scanner<'src, 'bump>) -> Option<Self> {
        let start = scanner.location();
        if scanner.match_start("null") {
            Some(Self {
                start,
                scanner,
                value: TokenKind::Null,
            })
        } else if scanner.match_start("true") {
            Some(Self {
                start,
                scanner,
                value: TokenKind::True,
            })
        } else if scanner.match_start("false") {
            Some(Self {
                start,
                scanner,
                value: TokenKind::False,
            })
        } else if scanner.match_start("for") {
            Some(Self {
                start,
                scanner,
                value: TokenKind::For,
            })
        } else if scanner.match_start("in") {
            Some(Self {
                start,
                scanner,
                value: TokenKind::In,
            })
        } else if scanner.match_start("let") {
            Some(Self {
                start,
                scanner,
                value: TokenKind::Let,
            })
        } else if scanner.match_start("if") {
            Some(Self {
                start,
                scanner,
                value: TokenKind::If,
            })
        } else {
            None
        }
    }

    fn is_error(&self) -> bool {
        false
    }

    fn accept(self, lexer: &mut Lexer<'src, 'bump>) {
        lexer.token(self.scanner, [(self.start, self.value)].into_iter())
    }
}

#[cfg(test)]
mod test {
    use bumpalo::Bump;

    use crate::{
        parser::lexer::{
            test::{make_error, make_token},
            ErrorKind, TokenKind,
        },
        test_lexer,
    };

    use pretty_assertions::assert_eq;

    use super::Keyword;

    test_lexer!(no_keyword, Keyword, r#"abc"#);

    test_lexer!(null_keyword, Keyword, r#"null"#, 4, |bump| [(
        0..4,
        0,
        0,
        TokenKind::Null
    )]);

    test_lexer!(true_keyword, Keyword, r#"true"#, 4, |bump| [(
        0..4,
        0,
        0,
        TokenKind::True
    )]);

    test_lexer!(false_keyword, Keyword, r#"false"#, 5, |bump| [(
        0..5,
        0,
        0,
        TokenKind::False
    )]);

    test_lexer!(for_keyword, Keyword, r#"for"#, 3, |bump| [(
        0..3,
        0,
        0,
        TokenKind::For
    )]);

    test_lexer!(in_keyword, Keyword, r#"in"#, 2, |bump| [(
        0..2,
        0,
        0,
        TokenKind::In
    )]);

    test_lexer!(let_keyword, Keyword, r#"let"#, 3, |bump| [(
        0..3,
        0,
        0,
        TokenKind::Let
    )]);

    test_lexer!(if_keyword, Keyword, r#"if"#, 2, |bump| [(
        0..2,
        0,
        0,
        TokenKind::If
    )]);
}
