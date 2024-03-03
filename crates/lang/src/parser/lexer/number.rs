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
    use crate::{
        parser::lexer::{
            test::{make_error, make_token},
            ErrorKind, TokenKind,
        },
        test_lexer,
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

    test_lexer!(no_integer, Number, r#"abc"#);
    test_lexer!(no_float, Number, r#".abc"#);

    test_lexer!(
        bad_hex,
        Number,
        r#"0xZ"#,
        2,
        |bump| [(0..2, 0, 0, TokenKind::Integer(0))],
        |bump| [(0..2, 0, 0, ErrorKind::InvalidNumberLiteral)]
    );

    test_lexer!(
        bad_oct,
        Number,
        r#"0o8"#,
        2,
        |bump| [(0..2, 0, 0, TokenKind::Integer(0))],
        |bump| [(0..2, 0, 0, ErrorKind::InvalidNumberLiteral)]
    );

    test_lexer!(
        bad_bin,
        Number,
        r#"0b2"#,
        2,
        |bump| [(0..2, 0, 0, TokenKind::Integer(0))],
        |bump| [(0..2, 0, 0, ErrorKind::InvalidNumberLiteral)]
    );

    test_lexer!(
        bad_float_1,
        Number,
        r#"04ea"#,
        3,
        |bump| [(0..3, 0, 0, TokenKind::Integer(0))],
        |bump| [(0..3, 0, 0, ErrorKind::InvalidNumberLiteral)]
    );

    test_lexer!(
        bad_float_2,
        Number,
        r#"04e__"#,
        5,
        |bump| [(0..5, 0, 0, TokenKind::Integer(0))],
        |bump| [(0..5, 0, 0, ErrorKind::InvalidNumberLiteral)]
    );

    test_lexer!(integer, Number, r#"1_234"#, 5, |bump| [(
        0..5,
        0,
        0,
        TokenKind::Integer(1234)
    )]);

    test_lexer!(integer_units, Number, r#"1_234K"#, 6, |bump| [(
        0..6,
        0,
        0,
        TokenKind::Integer(ten_pow(1234, 1))
    )]);

    test_lexer!(hex, Number, r#"0xAE0_1"#, 7, |bump| [(
        0..7,
        0,
        0,
        TokenKind::Integer(0xAE01)
    )]);

    test_lexer!(hex_units, Number, r#"0xAE0_1Ki"#, 9, |bump| [(
        0..9,
        0,
        0,
        TokenKind::Integer(two_pow(0xAE01, 1))
    )]);

    test_lexer!(oct, Number, r#"0o77_4"#, 6, |bump| [(
        0..6,
        0,
        0,
        TokenKind::Integer(0o774)
    )]);

    test_lexer!(oct_units, Number, r#"0o77_4M"#, 7, |bump| [(
        0..7,
        0,
        0,
        TokenKind::Integer(ten_pow(0o774, 2))
    )]);

    test_lexer!(bin, Number, r#"0b00_1001"#, 9, |bump| [(
        0..9,
        0,
        0,
        TokenKind::Integer(0b001001)
    )]);

    test_lexer!(bin_units, Number, r#"0b00_1001Mi"#, 11, |bump| [(
        0..11,
        0,
        0,
        TokenKind::Integer(two_pow(0b001001, 2))
    )]);

    test_lexer!(float, Number, r#"10_0.123"#, 8, |bump| [(
        0..8,
        0,
        0,
        TokenKind::Float(100.123)
    )]);

    test_lexer!(float_units, Number, r#"10_0.123G"#, 9, |bump| [(
        0..9,
        0,
        0,
        TokenKind::Integer(ten_pow(100.123, 3))
    )]);

    test_lexer!(small_float, Number, r#".123_4"#, 6, |bump| [(
        0..6,
        0,
        0,
        TokenKind::Float(0.1234)
    )]);

    test_lexer!(small_float_units, Number, r#".123_4G"#, 7, |bump| [(
        0..7,
        0,
        0,
        TokenKind::Integer(ten_pow(0.1234, 3))
    )]);

    test_lexer!(expo_float, Number, r#".123_4e_4"#, 9, |bump| [(
        0..9,
        0,
        0,
        TokenKind::Float(0.1234e4)
    )]);

    test_lexer!(expo_float_units, Number, r#".123_4e_4T"#, 10, |bump| [(
        0..10,
        0,
        0,
        TokenKind::Integer(ten_pow(0.1234e4, 4))
    )]);

    test_lexer!(pos_expo_float, Number, r#".123_4e+_4"#, 10, |bump| [(
        0..10,
        0,
        0,
        TokenKind::Float(0.1234e4)
    )]);

    test_lexer!(
        pos_expo_float_units,
        Number,
        r#".123_4e+_4Ti"#,
        12,
        |bump| [(0..12, 0, 0, TokenKind::Integer(two_pow(0.1234e4, 4)))]
    );

    test_lexer!(neg_expo_float, Number, r#".123_4e-_4"#, 10, |bump| [(
        0..10,
        0,
        0,
        TokenKind::Float(0.1234e-4)
    )]);

    test_lexer!(neg_expo_float_units, Number, r#".123_4e-_4P"#, 11, |bump| [
        (0..11, 0, 0, TokenKind::Integer(ten_pow(0.1234e-4, 5)))
    ]);

    test_lexer!(
        neg_expo_float_units_pebi,
        Number,
        r#".123_4e-_4Pi"#,
        12,
        |bump| [(0..12, 0, 0, TokenKind::Integer(two_pow(0.1234e-4, 5)))]
    );
}
