use tokenizer::{tokenize, Token::*};

#[test]
fn numbers_identifiers_and_operators() {
    assert_eq!(
        tokenize("rate*(12.5 + x_1)/-3").unwrap(),
        vec![Ident("rate".into()), Op('*'), LParen, Number(12.5), Op('+'), Ident("x_1".into()),
             RParen, Op('/'), Op('-'), Number(3.0)]
    );
    assert_eq!(tokenize("  ").unwrap(), vec![]);
    assert_eq!(tokenize("7").unwrap(), vec![Number(7.0)]);
}

#[test]
fn errors_point_at_the_problem() {
    assert_eq!(tokenize("1 + $").unwrap_err(), "unexpected '$' at 4");
    assert_eq!(tokenize("1.2.3").unwrap_err(), "unexpected '.' at 3");
}
