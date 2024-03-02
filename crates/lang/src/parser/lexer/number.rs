use super::{ErrorKind, Lexer, LocationRef, Scanner, Token, TokenKind, TokenLexer};

enum NumberValue {
    Integer(i64),
    Float(f64),
    Err,
}

pub struct Number<'src, 'bump> {
    start: LocationRef,
    scanner: Scanner<'src, 'bump>,
    value: NumberValue,
}

impl<'src, 'bump> TokenLexer<'src, 'bump> for Number<'src, 'bump> {
    fn lex(_: &Lexer<'src, 'bump>, mut scanner: Scanner<'src, 'bump>) -> Option<Self> {
        let start = scanner.location();
        if scanner.match_start("0x") || scanner.match_start("0X") {
            Some(Self::take_radix_num(
                scanner,
                start,
                16,
                char::is_ascii_hexdigit,
            ))
        } else if scanner.match_start("0o") {
            Some(Self::take_radix_num(scanner, start, 8, |c| {
                ('0'..='7').contains(c)
            }))
        } else if scanner.match_start("0b") {
            Some(Self::take_radix_num(scanner, start, 2, |c| {
                ('0'..='1').contains(c)
            }))
        } else {
            match scanner.remainder().as_bytes() {
                [b'.', b'0'..=b'9', ..] | [b'0'..=b'9', ..] => {
                    Some(Self::take_decimal_num(scanner))
                }
                _ => None,
            }
        }
    }

    fn is_error(&self) -> bool {
        matches!(self.value, NumberValue::Err)
    }

    fn accept(self, lexer: &mut Lexer<'src, 'bump>) {
        match self.value {
            NumberValue::Integer(i) => lexer.token(
                self.scanner,
                [(self.start, TokenKind::Integer(i))].into_iter(),
            ),
            NumberValue::Float(f) => lexer.token(
                self.scanner,
                [(self.start, TokenKind::Float(f))].into_iter(),
            ),
            NumberValue::Err => {
                lexer.error(
                    &self.scanner,
                    [(self.start, ErrorKind::InvalidNumberLiteral)].into_iter(),
                );
                lexer.token(
                    self.scanner,
                    [(self.start, TokenKind::Integer(0))].into_iter(),
                )
            }
        }
    }
}

impl<'src, 'bump> Number<'src, 'bump> {
    fn take_decimal_num(mut scanner: Scanner<'src, 'bump>) -> Self {
        let start = scanner.location();
        let mut tmp = String::new();

        let mut denom = false;
        let mut expo = false;

        while let Some(c) = scanner.nth_char(0) {
            let loc = scanner.location();
            match c {
                '0'..='9' => (),
                '.' if !denom && !expo => {
                    denom = true;
                }
                'e' | 'E' if !expo => {
                    expo = true;
                    scanner.advance_char();
                    if !scanner.match_start("+") {
                        scanner.match_start("-");
                    }
                    tmp.push_str(scanner.get_str(loc));
                    continue;
                }
                '_' => {
                    scanner.advance_char();
                    continue;
                }
                _ => break,
            }
            scanner.advance_char();
            tmp.push(c);
        }

        let u = Self::take_num_unit(&mut scanner);

        if !denom && !expo {
            if let Some(v) = tmp.parse().ok().and_then(|v| u.checked_mul(v)) {
                Self {
                    scanner,
                    start,
                    value: NumberValue::Integer(v),
                }
            } else {
                Self {
                    scanner,
                    start,
                    value: NumberValue::Err,
                }
            }
        } else if let Ok(v) = tmp.parse() {
            if u == 1 {
                Self {
                    scanner,
                    start,
                    value: NumberValue::Float(v),
                }
            } else {
                let v = v * u as f64;
                if v.is_finite() {
                    Self {
                        scanner,
                        start,
                        value: NumberValue::Integer(v as i64),
                    }
                } else {
                    Self {
                        scanner,
                        start,
                        value: NumberValue::Err,
                    }
                }
            }
        } else {
            Self {
                scanner,
                start,
                value: NumberValue::Err,
            }
        }
    }

    fn take_radix_num(
        mut scanner: Scanner<'src, 'bump>,
        orig_start: LocationRef,
        radix: u32,
        valid_char: impl Fn(&char) -> bool,
    ) -> Self {
        let mut tmp = String::new();
        while let Some(c) = scanner.nth_char(0) {
            if c == '_' {
                scanner.advance_char();
                continue;
            }
            if !valid_char(&c) {
                break;
            }
            scanner.advance_char();
            tmp.push(c);
        }

        if tmp.is_empty() {
            return Self {
                start: orig_start,
                scanner,
                value: NumberValue::Err,
            };
        }

        let unit = Self::take_num_unit(&mut scanner);
        dbg!(unit);

        if let Some(s) = i64::from_str_radix(&tmp, radix)
            .ok()
            .and_then(|v| v.checked_mul(unit))
        {
            Self {
                start: orig_start,
                scanner,
                value: NumberValue::Integer(s),
            }
        } else {
            Self {
                start: orig_start,
                scanner,
                value: NumberValue::Err,
            }
        }
    }

    fn take_num_unit(scanner: &mut Scanner<'src, 'bump>) -> i64 {
        match scanner.nth_char(0) {
            Some('K') if scanner.match_start("Ki") => 1_024,
            Some('K') if scanner.advance_char().is_some() => 1_000,
            Some('M') if scanner.match_start("Mi") => 1_048_576,
            Some('M') if scanner.advance_char().is_some() => 1_000_000,
            Some('G') if scanner.match_start("Gi") => 1_073_741_824,
            Some('G') if scanner.advance_char().is_some() => 1_000_000_000,
            Some('T') if scanner.match_start("Ti") => 1_099_511_627_776,
            Some('T') if scanner.advance_char().is_some() => 1_000_000_000_000,
            Some('P') if scanner.match_start("Pi") => 1_125_899_906_842_624,
            Some('P') if scanner.advance_char().is_some() => 1_000_000_000_000_000,
            _ => 1,
        }
    }
}

#[cfg(test)]
mod test {
    use bumpalo::Bump;

    use crate::parser::lexer::{
        test::{make_error, make_token, test_lexer},
        ErrorKind, TokenKind,
    };

    use pretty_assertions::assert_eq;

    use super::Number;

    fn ten_pow(v: impl Into<f64>, i: i32) -> i64 {
        let v: f64 = v.into();
        (v * 10f64.powi(i * 3)) as i64
    }

    fn two_pow(v: impl Into<f64>, i: i32) -> i64 {
        let v: f64 = v.into();
        (v * 2f64.powi(i * 10)) as i64
    }

    #[test]
    fn no_number() {
        let bump = Bump::new();
        let r = test_lexer::<Number>(r#"abc"#, &bump);
        assert_eq!(r, None);
    }

    #[test]
    fn no_float_number() {
        let bump = Bump::new();
        let r = test_lexer::<Number>(r#".abc"#, &bump);
        assert_eq!(r, None);
    }

    #[test]
    fn bad_hex() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#"0xZ"#, bump).unwrap();
        assert_eq!(
            r,
            (
                2,
                vec![make_token(bump, 0..2, 0, 0, TokenKind::Integer(0))],
                vec![make_error(
                    bump,
                    0..2,
                    0,
                    0,
                    ErrorKind::InvalidNumberLiteral
                )]
            )
        );
    }

    #[test]
    fn bad_oct() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#"0o8"#, bump).unwrap();
        assert_eq!(
            r,
            (
                2,
                vec![make_token(bump, 0..2, 0, 0, TokenKind::Integer(0))],
                vec![make_error(
                    bump,
                    0..2,
                    0,
                    0,
                    ErrorKind::InvalidNumberLiteral
                )]
            )
        );
    }

    #[test]
    fn bad_bin() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#"0b2"#, bump).unwrap();
        assert_eq!(
            r,
            (
                2,
                vec![make_token(bump, 0..2, 0, 0, TokenKind::Integer(0))],
                vec![make_error(
                    bump,
                    0..2,
                    0,
                    0,
                    ErrorKind::InvalidNumberLiteral
                )]
            )
        );
    }

    #[test]
    fn bad_float_1() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#"04ea"#, bump).unwrap();
        assert_eq!(
            r,
            (
                3,
                vec![make_token(bump, 0..3, 0, 0, TokenKind::Integer(0))],
                vec![make_error(
                    bump,
                    0..3,
                    0,
                    0,
                    ErrorKind::InvalidNumberLiteral
                )]
            )
        );
    }

    #[test]
    fn bad_float_2() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#"04e__"#, bump).unwrap();
        assert_eq!(
            r,
            (
                5,
                vec![make_token(bump, 0..5, 0, 0, TokenKind::Integer(0))],
                vec![make_error(
                    bump,
                    0..5,
                    0,
                    0,
                    ErrorKind::InvalidNumberLiteral
                )]
            )
        );
    }

    #[test]
    fn integer() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#"1_234"#, bump).unwrap();
        assert_eq!(
            r,
            (
                5,
                vec![make_token(bump, 0..5, 0, 0, TokenKind::Integer(1234))],
                vec![]
            )
        )
    }

    #[test]
    fn integer_units() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#"123_4K"#, bump).unwrap();
        assert_eq!(
            r,
            (
                6,
                vec![make_token(
                    bump,
                    0..6,
                    0,
                    0,
                    TokenKind::Integer(ten_pow(1234, 1))
                )],
                vec![]
            )
        )
    }

    #[test]
    fn hex() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#"0xAE0_1"#, bump).unwrap();
        assert_eq!(
            r,
            (
                7,
                vec![make_token(bump, 0..7, 0, 0, TokenKind::Integer(0xAE01))],
                vec![]
            )
        )
    }

    #[test]
    fn hex_units() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#"0xA_E01Ki"#, bump).unwrap();
        assert_eq!(
            r,
            (
                9,
                vec![make_token(
                    bump,
                    0..9,
                    0,
                    0,
                    TokenKind::Integer(two_pow(0xAE01, 1))
                )],
                vec![]
            )
        )
    }

    #[test]
    fn oct() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#"0o77_4"#, bump).unwrap();
        assert_eq!(
            r,
            (
                6,
                vec![make_token(bump, 0..6, 0, 0, TokenKind::Integer(0o774))],
                vec![]
            )
        )
    }

    #[test]
    fn oct_units() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#"0o7_74M"#, bump).unwrap();
        assert_eq!(
            r,
            (
                7,
                vec![make_token(
                    bump,
                    0..7,
                    0,
                    0,
                    TokenKind::Integer(ten_pow(0o774, 2))
                )],
                vec![]
            )
        )
    }

    #[test]
    fn bin() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#"0b00_1001"#, bump).unwrap();
        assert_eq!(
            r,
            (
                9,
                vec![make_token(bump, 0..9, 0, 0, TokenKind::Integer(0b001001))],
                vec![]
            )
        )
    }

    #[test]
    fn bin_units() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#"0b001001_Mi"#, bump).unwrap();
        assert_eq!(
            r,
            (
                11,
                vec![make_token(
                    bump,
                    0..11,
                    0,
                    0,
                    TokenKind::Integer(two_pow(0b001001, 2))
                )],
                vec![]
            )
        )
    }

    #[test]
    fn float() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#"10_0.123"#, bump).unwrap();
        assert_eq!(
            r,
            (
                8,
                vec![make_token(bump, 0..8, 0, 0, TokenKind::Float(100.123))],
                vec![]
            )
        )
    }

    #[test]
    fn float_units() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#"100._123G"#, bump).unwrap();
        assert_eq!(
            r,
            (
                9,
                vec![make_token(
                    bump,
                    0..9,
                    0,
                    0,
                    TokenKind::Integer(ten_pow(100.123, 3))
                )],
                vec![]
            )
        )
    }

    #[test]
    fn small_float() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#".123_4"#, bump).unwrap();
        assert_eq!(
            r,
            (
                6,
                vec![make_token(bump, 0..6, 0, 0, TokenKind::Float(0.1234))],
                vec![]
            )
        )
    }

    #[test]
    fn small_float_units() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#".1_234Gi"#, bump).unwrap();
        assert_eq!(
            r,
            (
                8,
                vec![make_token(
                    bump,
                    0..8,
                    0,
                    0,
                    TokenKind::Integer(two_pow(0.1234, 3))
                )],
                vec![]
            )
        )
    }

    #[test]
    fn expo_float() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#".123_4e_4"#, bump).unwrap();
        assert_eq!(
            r,
            (
                9,
                vec![make_token(bump, 0..9, 0, 0, TokenKind::Float(0.1234e4))],
                vec![]
            )
        )
    }

    #[test]
    fn expo_float_units() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#".1_234e4_T"#, bump).unwrap();
        assert_eq!(
            r,
            (
                10,
                vec![make_token(
                    bump,
                    0..10,
                    0,
                    0,
                    TokenKind::Integer(ten_pow(0.1234e4, 4))
                )],
                vec![]
            )
        )
    }

    #[test]
    fn pos_expo_float() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#".123_4e+_4"#, bump).unwrap();
        assert_eq!(
            r,
            (
                10,
                vec![make_token(bump, 0..10, 0, 0, TokenKind::Float(0.1234e4))],
                vec![]
            )
        )
    }

    #[test]
    fn pos_expo_float_units() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#".1_234e+4_Ti"#, bump).unwrap();
        assert_eq!(
            r,
            (
                12,
                vec![make_token(
                    bump,
                    0..12,
                    0,
                    0,
                    TokenKind::Integer(two_pow(0.1234e4, 4))
                )],
                vec![]
            )
        )
    }

    #[test]
    fn neg_expo_float() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#".123_4e-_4"#, bump).unwrap();
        assert_eq!(
            r,
            (
                10,
                vec![make_token(bump, 0..10, 0, 0, TokenKind::Float(0.1234e-4))],
                vec![]
            )
        )
    }

    #[test]
    fn neg_expo_float_units() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#".1_234e-4_P"#, bump).unwrap();
        assert_eq!(
            r,
            (
                11,
                vec![make_token(
                    bump,
                    0..11,
                    0,
                    0,
                    TokenKind::Integer(ten_pow(0.1234e-4, 5))
                )],
                vec![]
            )
        )
    }

    #[test]
    fn neg_expo_float_units_pebi() {
        let bump = Bump::new();
        let bump = &bump;
        let r = test_lexer::<Number>(r#".1_234e-4_Pi"#, bump).unwrap();
        assert_eq!(
            r,
            (
                12,
                vec![make_token(
                    bump,
                    0..12,
                    0,
                    0,
                    TokenKind::Integer(two_pow(0.1234e-4, 5))
                )],
                vec![]
            )
        )
    }
}
