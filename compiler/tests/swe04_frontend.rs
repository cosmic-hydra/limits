use limits::{
    error::{Kind, Span},
    lexer::{self, Tok, Token},
    parser,
};

#[test]
fn missing_identifiers_report_the_actual_end_of_input() {
    for source in ["let", "fn", "fn f(", "fn f(a,", "for"] {
        let tokens = lexer::lex(source).unwrap();
        let end = tokens.last().unwrap().span;
        let error = parser::parse(tokens).unwrap_err();
        assert_eq!(error.kind, Kind::Parse);
        assert_eq!(error.msg, "expected identifier, found Eof");
        assert_eq!(error.span, end, "{source}");
    }
}

#[test]
fn malformed_public_token_streams_cannot_hide_after_an_early_eof() {
    let span = Span { line: 3, col: 7 };
    let mut tokens = vec![Token {
        tok: Tok::Eof,
        span,
    }];
    tokens.extend(lexer::lex("let x=1;").unwrap());
    let error = parser::parse(tokens).unwrap_err();
    assert_eq!(error.kind, Kind::Parse);
    assert_eq!(error.msg, "unexpected end-of-file token");
    assert_eq!(error.span, span);

    let tokens = vec![Token {
        tok: Tok::Eof,
        span,
    }];
    assert!(parser::parse(tokens).unwrap().is_empty());
}
