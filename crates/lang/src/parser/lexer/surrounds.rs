use super::{ErrorKind, Lexer, LocationRef, Scanner, Scope, Token, TokenKind, TokenLexer};

enum ScopeAction<'src> {
    Push(Scope<'src>),
    Pop(Scope<'src>),
}

pub struct Surround<'src, 'bump> {
    start: LocationRef,
    scanner: Scanner<'src, 'bump>,
    value: TokenKind<'bump>,
    action: ScopeAction<'src>,
}

impl<'src, 'bump> TokenLexer<'src, 'bump> for Surround<'src, 'bump> {
    fn lex(lexer: &Lexer<'src, 'bump>, mut scanner: Scanner<'src, 'bump>) -> Option<Self> {
        let start = scanner.location();
        match scanner.nth_char(0)? {
            '[' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::LBracket,
                action: ScopeAction::Push(Scope::Bracket),
            }),
            ']' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::RBracket,
                action: ScopeAction::Pop(Scope::Bracket),
            }),
            '{' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::LBrace,
                action: ScopeAction::Push(Scope::Brace),
            }),
            '}' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::RBrace,
                action: ScopeAction::Pop(Scope::Brace),
            }),
            '(' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::LParen,
                action: ScopeAction::Push(Scope::Paren),
            }),
            ')' if !lexer
                .peek_scope()
                .map(|v| v.is_interpolation())
                .unwrap_or_default() =>
            {
                Some(Self {
                    start,
                    scanner,
                    value: TokenKind::RParen,
                    action: ScopeAction::Pop(Scope::Paren),
                })
            }
            _ => None,
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

    use crate::parser::lexer::{
        test::{make_error, make_token},
        ErrorKind, TokenKind,
    };

    use pretty_assertions::assert_eq;

    use super::Surround;
}
