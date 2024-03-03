use super::{
    BinaryOperator, CommaKind, ErrorKind, Lexer, LocationRef, Scanner, Token, TokenKind, TokenLexer,
};

pub struct Operator<'src, 'bump> {
    start: LocationRef,
    scanner: Scanner<'src, 'bump>,
    value: TokenKind<'bump>,
}

impl<'src, 'bump> TokenLexer<'src, 'bump> for Operator<'src, 'bump> {
    fn lex(_: &Lexer<'src, 'bump>, mut scanner: Scanner<'src, 'bump>) -> Option<Self> {
        let start = scanner.location();

        match scanner.nth_char(0)? {
            '+' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::BinaryOperator(BinaryOperator::Add),
            }),
            '-' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::BinaryOperator(BinaryOperator::Sub),
            }),
            '/' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::BinaryOperator(BinaryOperator::Div),
            }),
            '*' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::BinaryOperator(BinaryOperator::Mul),
            }),
            ':' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::Colon,
            }),
            '?' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::Question,
            }),
            ',' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::Comma(CommaKind::Explicit),
            }),
            '_' if scanner.match_start("_|_") => Some(Self {
                start,
                scanner,
                value: TokenKind::Bottom,
            }),
            '.' if scanner.match_start("...") => Some(Self {
                start,
                scanner,
                value: TokenKind::Elipses,
            }),
            '.' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::Dot,
            }),
            '&' if scanner.match_start("&&") => Some(Self {
                start,
                scanner,
                value: TokenKind::BinaryOperator(BinaryOperator::LogicalAnd),
            }),
            '&' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::BinaryOperator(BinaryOperator::And),
            }),
            '|' if scanner.match_start("||") => Some(Self {
                start,
                scanner,
                value: TokenKind::BinaryOperator(BinaryOperator::LogicalOr),
            }),
            '|' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::BinaryOperator(BinaryOperator::Or),
            }),
            '!' if scanner.match_start("!=") => Some(Self {
                start,
                scanner,
                value: TokenKind::BinaryOperator(BinaryOperator::Inequal),
            }),
            '!' if scanner.match_start("!~") => Some(Self {
                start,
                scanner,
                value: TokenKind::BinaryOperator(BinaryOperator::ApproxInequal),
            }),
            '!' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::Bang,
            }),
            '=' if scanner.match_start("==") => Some(Self {
                start,
                scanner,
                value: TokenKind::BinaryOperator(BinaryOperator::Equal),
            }),
            '=' if scanner.match_start("=~") => Some(Self {
                start,
                scanner,
                value: TokenKind::BinaryOperator(BinaryOperator::ApproxEqual),
            }),
            '=' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::Assign,
            }),
            '<' if scanner.match_start("<=") => Some(Self {
                start,
                scanner,
                value: TokenKind::BinaryOperator(BinaryOperator::LessEqual),
            }),
            '<' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::BinaryOperator(BinaryOperator::Less),
            }),
            '>' if scanner.match_start(">=") => Some(Self {
                start,
                scanner,
                value: TokenKind::BinaryOperator(BinaryOperator::GreaterEqual),
            }),
            '>' if scanner.advance_char().is_some() => Some(Self {
                start,
                scanner,
                value: TokenKind::BinaryOperator(BinaryOperator::Greater),
            }),
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

    use super::Operator;
}
