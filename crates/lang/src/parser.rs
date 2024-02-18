#![allow(unused, clippy::result_large_err)]

use std::{path::PathBuf, sync::Arc};

use bumpalo::collections::{String, Vec};
use bumpalo::Bump;

mod lexer;
mod location;

pub use location::Location;

#[cfg(never)]
pub mod test {
    use std::{path::PathBuf, sync::Arc};

    use anyhow::{anyhow, bail};
    use bumpalo::Bump;

    use crate::parser::StringType;

    use super::{ExpressionType, LangParser, ParseError, SourceLocation};
    use pretty_assertions::assert_eq;

    fn path() -> PathBuf {
        PathBuf::from("-")
    }

    fn loc(line: usize, col: usize, len: usize) -> SourceLocation {
        SourceLocation {
            line,
            col,
            len,
            path: Arc::new(path()),
        }
    }

    pub fn parse<'a>(
        value: &str,
        bump: &'a Bump,
    ) -> super::Result<super::Node<ExpressionType<'a>>> {
        super::parse(value, path(), bump)
    }

    pub fn parse_err(value: &str, bump: &Bump) -> anyhow::Result<String> {
        match parse(value, bump) {
            Err(ParseError::Parse(e)) => Ok(e.variant.message().into_owned()),
            Err(other) => Ok(format!("{other}")),
            Ok(_) => anyhow::bail!("expected the parser to fail"),
        }
    }

    #[test]
    pub fn test_number() -> anyhow::Result<()> {
        let bump = Bump::new();
        assert_eq!(
            parse("1234", &bump)?,
            ExpressionType::number("1234", loc(1, 1, 4))
        );
        assert_eq!(
            parse("1234.4567", &bump)?,
            ExpressionType::number("1234.4567", loc(1, 1, 9))
        );

        Ok(())
    }

    #[test]
    pub fn test_string() -> anyhow::Result<()> {
        let bump = Bump::new();
        assert_eq!(
            parse(r#"" some value \( 1234 ) test ""#, &bump)?,
            ExpressionType::string(
                &[
                    StringType::text(" some value ", loc(1, 2, 12)),
                    StringType::expression(
                        ExpressionType::number("1234", loc(1, 17, 4)),
                        loc(1, 14, 9)
                    ),
                    StringType::text(" test ", loc(1, 23, 6)),
                ],
                loc(1, 1, 29)
            )
        );

        assert_eq!(
            parse(r#"" test\r\n\t\"\u{2764}\x64value ""#, &bump)?,
            ExpressionType::string(
                &[StringType::text(
                    " test\r\n\t\"\u{2764}\x64value ",
                    loc(1, 2, 31)
                )],
                loc(1, 1, 33)
            )
        );

        assert_eq!(
            parse_err(r#"" test\'value ""#, &bump)?,
            "expected quoted_inner or interpolated"
        );

        assert_eq!(
            parse_err(r#"" test\u{d800}value ""#, &bump)?,
            "d800 is not a valid unicode codepoint"
        );

        Ok(())
    }

    #[test]
    pub fn test_multi_string() -> anyhow::Result<()> {
        let bump = Bump::new();
        assert_eq!(
            parse(
                r#"''
                some value
                foo \( 1234 ) bar
                 test 
                ''"#,
                &bump
            )?,
            ExpressionType::string(
                &[
                    StringType::text("some value", loc(2, 17, 10)),
                    StringType::text("\n", loc(2, 27, 1)),
                    StringType::text("foo ", loc(3, 17, 4)),
                    StringType::expression(
                        ExpressionType::number("1234", loc(3, 24, 4)),
                        loc(3, 21, 9)
                    ),
                    StringType::text(" bar", loc(3, 30, 4)),
                    StringType::text("\n", loc(3, 34, 1)),
                    StringType::text(" test ", loc(4, 17, 6)),
                    StringType::text("\n", loc(4, 23, 1)),
                ],
                loc(1, 1, 105)
            )
        );

        assert_eq!(
            parse(
                r#"''
                test\r\n\t\''\u{2764}\x64value''"#,
                &bump
            )?,
            ExpressionType::string(
                &[StringType::text(
                    "test\r\n\t''\u{2764}\x64value",
                    loc(2, 17, 30)
                )],
                loc(1, 1, 51)
            )
        );

        assert_eq!(
            parse_err(
                r#"''
                test\'value''"#,
                &bump
            )?,
            "expected multi_run_newline, multi_inner, or interpolated"
        );

        assert_eq!(
            parse_err(
                r#"''
                test\u{d800}value''"#,
                &bump
            )?,
            "d800 is not a valid unicode codepoint"
        );

        Ok(())
    }

    #[test]
    pub fn test_selector() -> anyhow::Result<()> {
        let mut bump = Bump::new();
        assert_eq!(
            parse(r#" foo. bar "#, &bump)?,
            ExpressionType::selector(
                &[
                    &[StringType::text("foo", loc(1, 2, 3)),],
                    &[StringType::text("bar", loc(1, 7, 3)),]
                ],
                loc(1, 2, 9)
            )
        );

        bump.reset();

        assert_eq!(
            parse(r#" \( 1 )foo. bar "#, &bump)?,
            ExpressionType::selector(
                &[
                    &[
                        StringType::expression(
                            ExpressionType::number("1", loc(1, 5, 1)),
                            loc(1, 2, 6)
                        ),
                        StringType::text("foo", loc(1, 8, 3)),
                    ],
                    &[StringType::text("bar", loc(1, 13, 3)),]
                ],
                loc(1, 2, 15)
            )
        );

        bump.reset();

        assert_eq!(
            parse_err(r#" \( 1 ) foo. bar "#, &bump)?,
            "expected ident or interpolated"
        );

        Ok(())
    }
}
