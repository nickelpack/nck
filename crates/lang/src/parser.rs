#![allow(unused)]

use std::{path::PathBuf, sync::Arc};

use pest::{
    error::{InputLocation, LineColLocation},
    fails_with,
    iterators::Pair,
    Parser, RuleType,
};
use pest_derive::Parser;
use thiserror::Error;

type Result<T, E = ParseError> = std::result::Result<T, E>;

#[derive(Parser)]
#[grammar = "parser/grammar.pest"]
struct LangParser;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SourceLocation {
    line: usize,
    col: usize,
    len: usize,
    path: Arc<PathBuf>,
}

impl SourceLocation {
    pub fn new(path: PathBuf) -> Self {
        Self {
            line: 0,
            col: 0,
            len: 0,
            path: Arc::new(path),
        }
    }

    fn with_location<R: RuleType>(&self, pair: &Pair<R>) -> Self {
        let len = pair.as_str().len();
        let (line, col) = pair.line_col();
        Self {
            line,
            col,
            len,
            path: self.path.clone(),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Node<T> {
    node: Arc<T>,
    source_location: SourceLocation,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ExpressionType<'a> {
    Number(&'a str),
    String(Arc<[Node<StringType<'a>>]>),
}

impl<'a> ExpressionType<'a> {
    pub fn number(value: &'a str, source_location: SourceLocation) -> Node<Self> {
        Node {
            node: Arc::new(Self::Number(value)),
            source_location,
        }
    }

    pub fn string(
        value: Arc<[Node<StringType<'a>>]>,
        source_location: SourceLocation,
    ) -> Node<Self> {
        Node {
            node: Arc::new(ExpressionType::String(value)),
            source_location,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum StringType<'a> {
    Text(&'a str),
    Expression(Node<ExpressionType<'a>>),
}

impl<'a> StringType<'a> {
    pub fn text(value: &'a str, source_location: SourceLocation) -> Node<Self> {
        Node {
            node: Arc::new(Self::Text(value)),
            source_location,
        }
    }

    pub fn expression(
        value: Node<ExpressionType<'a>>,
        source_location: SourceLocation,
    ) -> Node<Self> {
        Node {
            node: Arc::new(Self::Expression(value)),
            source_location,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum ParseError {
    #[error(transparent)]
    Parse(#[from] pest::error::Error<Rule>),
    #[error("bad multi")]
    BadMultiString,
}

fn parse_root<'a>(
    mut pair: Pair<'a, Rule>,
    location: &SourceLocation,
) -> Result<Node<ExpressionType<'a>>> {
    loop {
        let location = location.with_location(&pair);
        match pair.as_rule() {
            Rule::number => break Ok(ExpressionType::number(pair.as_str(), location)),
            Rule::quoted_string => {
                break Ok(ExpressionType::string(
                    parse_quoted_string(pair, &location)?,
                    location,
                ))
            }
            Rule::multi_string => {
                break Ok(ExpressionType::string(
                    parse_multi_string(pair, &location)?,
                    location,
                ))
            }
            Rule::interpolated | Rule::expression => pair = pair.into_inner().next().unwrap(),
            Rule::main
            | Rule::multi_inner
            | Rule::multi_opt
            | Rule::multi_run
            | Rule::multi_run_newline
            | Rule::quoted_inner
            | Rule::quoted_opt
            | Rule::quoted_run
            | Rule::WHITESPACE
            | Rule::EOI => unreachable!(),
        }
    }
}

#[inline(always)]
fn parse_quoted_string<'a>(
    pair: Pair<'a, Rule>,
    location: &SourceLocation,
) -> Result<Arc<[Node<StringType<'a>>]>> {
    let pairs = pair.into_inner();
    let mut vec = Vec::with_capacity(pairs.len());

    for pair in pairs {
        let location = location.with_location(&pair);
        match pair.as_rule() {
            Rule::quoted_run => vec.push(StringType::text(pair.as_str(), location)),
            _ => vec.push(StringType::expression(
                parse_root(pair, &location)?,
                location,
            )),
        }
    }

    Ok(vec.into_boxed_slice().into())
}

#[inline(always)]
fn parse_multi_string<'a>(
    pair: Pair<'a, Rule>,
    location: &SourceLocation,
) -> Result<Arc<[Node<StringType<'a>>]>> {
    let pairs = pair.into_inner();
    let mut vec = Vec::with_capacity(pairs.len());

    for pair in pairs {
        let location = location.with_location(&pair);
        match pair.as_rule() {
            Rule::multi_run | Rule::multi_run_newline => {
                vec.push((StringType::Text(pair.as_str()), location))
            }
            _ => vec.push((
                StringType::Expression(parse_root(pair, &location)?),
                location,
            )),
        }
    }

    let mut start_of_line = true;
    let mut result = Vec::with_capacity(vec.len());
    let mut indent = ("", location.clone());
    for (s, loc) in vec.iter().rev() {
        match s {
            StringType::Text(s) => {
                if s.ends_with('\n') {
                    break;
                }
                indent = (s, loc.clone());
            }
            _ => {
                indent = ("", location.clone());
            }
        }
    }

    for (s, mut loc) in vec.into_iter() {
        if loc.col == 1 {
            loc.col += indent.0.len();
            loc.len -= indent.0.len();
        }
        match s {
            StringType::Text(v) => {
                if start_of_line {
                    if !v.starts_with(indent.0) {
                        // TODO: Error data
                        return Err(ParseError::BadMultiString);
                    }
                    let v = v.split_at(indent.0.len()).1;
                    if !v.is_empty() {
                        result.push(StringType::text(v, loc));
                    }
                } else if !v.is_empty() {
                    result.push(StringType::text(v, loc));
                }
                start_of_line = v.ends_with('\n');
            }
            StringType::Expression(e) => {
                if start_of_line && !indent.0.is_empty() {
                    // TODO: Error data
                    return Err(ParseError::BadMultiString);
                }
                result.push(StringType::expression(e, loc))
            }
        }
    }

    Ok(result.into_boxed_slice().into())
}

pub fn parse(value: &str, path: PathBuf) -> Result<Node<ExpressionType>> {
    let mut v = LangParser::parse(Rule::main, value)?;
    parse_root(v.next().unwrap(), &SourceLocation::new(path))
}

#[cfg(test)]
pub mod test {
    use std::{path::PathBuf, sync::Arc};

    use pest::Parser;

    use crate::parser::StringType;

    use super::{parse, ExpressionType, LangParser, SourceLocation};
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

    #[test]
    pub fn test_number() -> anyhow::Result<()> {
        assert_eq!(
            parse("1234", path())?,
            ExpressionType::number("1234", loc(1, 1, 4))
        );
        assert_eq!(
            parse("1234.4567", path())?,
            ExpressionType::number("1234.4567", loc(1, 1, 9))
        );

        Ok(())
    }

    #[test]
    pub fn test_string() -> anyhow::Result<()> {
        assert_eq!(
            parse(r#"" some value \( 1234 ) test ""#, path())?,
            ExpressionType::string(
                Arc::new([
                    StringType::text(" some value ", loc(1, 2, 12)),
                    StringType::expression(
                        ExpressionType::number("1234", loc(1, 17, 4)),
                        loc(1, 14, 9)
                    ),
                    StringType::text(" test ", loc(1, 23, 6)),
                ]),
                loc(1, 1, 29)
            )
        );

        Ok(())
    }

    #[test]
    pub fn test_multi_string() -> anyhow::Result<()> {
        assert_eq!(
            parse(
                r#"''
                some value
                foo \( 1234 ) bar
                 test 
                ''"#,
                path()
            )?,
            ExpressionType::string(
                Arc::new([
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
                ]),
                loc(1, 1, 105)
            )
        );

        Ok(())
    }
}
