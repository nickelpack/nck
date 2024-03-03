#![feature(allocator_api)]

mod parser;

#[macro_export]
macro_rules! test_lexer {
    ($name: ident, $type: ident, $src: literal, $end: literal, | $bump: ident |[$(($range: expr, $line: literal, $col: literal, $tok: expr)),+], | $err_bump: ident | [$(($err_range: expr, $err_line: literal, $err_col: literal, $err: expr)),+]) => {
        #[test]
        fn $name() {
            use $crate::parser::lexer::TokenLexer;
            let $bump = bumpalo::Bump::new();
            let $bump = &$bump;
            let path = std::path::PathBuf::from("test.cue");
            let mut lexer = $crate::parser::lexer::Lexer::new($src, &path, $bump);
            lexer
                .lex::<$type>()
                .map(|r| {
                    r.accept(&mut lexer);
                })
                .expect("successfully lexes");
            ::pretty_assertions::assert_eq!(
                lexer.scanner.offset,
                $end,
                "the lexer end position must match",
            );
            ::pretty_assertions::assert_eq!(
                lexer.tokens,
                vec![$(
                    $crate::parser::lexer::Token {
                        kind: $tok,
                        location: $crate::parser::Location::new_in($range, $line, $col, &path, $bump),
                    }
                ),+],
                "the tokens match",
            );
            let $err_bump = $bump;
            ::pretty_assertions::assert_eq!(
                lexer.errors,
                vec![$(
                    $crate::parser::lexer::Error {
                        kind: $err,
                        location: $crate::parser::Location::new_in($err_range, $err_line, $err_col, &path, $err_bump),
                    }
                ),+],
                "the errors match",
            );
        }
    };

    ($name: ident, $type: ident, $src: literal, $end: literal, | $bump: ident | [$(($range: expr, $line: literal, $col: literal, $tok: expr)),+]) => {
        #[test]
        fn $name() {
            use $crate::parser::lexer::TokenLexer;
            let $bump = bumpalo::Bump::new();
            let $bump = &$bump;
            let path = std::path::PathBuf::from("test.cue");
            let mut lexer = $crate::parser::lexer::Lexer::new($src, &path, $bump);
            lexer
                .lex::<$type>()
                .map(|r| {
                    r.accept(&mut lexer);
                })
                .expect("successfully lexes");
            ::pretty_assertions::assert_eq!(
                lexer.scanner.offset,
                $end,
                "the lexer end position must match",
            );
            ::pretty_assertions::assert_eq!(
                lexer.tokens,
                vec![$(
                    $crate::parser::lexer::Token {
                        kind: $tok,
                        location: $crate::parser::Location::new_in($range, $line, $col, &path, $bump),
                    }
                ),+],
                "the tokens match",
            );
        }
    };

    ($name: ident, $type: ident, $src: literal) => {
        #[test]
        fn $name() {
            use $crate::parser::lexer::TokenLexer;
            let bump = bumpalo::Bump::new();
            let bump = &bump;
            let path = std::path::PathBuf::from("test.cue");
            let mut lexer = $crate::parser::lexer::Lexer::new($src, &path, bump);
            let v = lexer
                .lex::<$type>()
                .map(|r| {
                    r.accept(&mut lexer);
                });
            ::pretty_assertions::assert_eq!(v, None, "should not lex");
        }
    };
}
