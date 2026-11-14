use kvlog::{distinct_keys, parse, ErrorKind, Index};

const LOG: &str = "ts=1 level=info msg=\"service started\" host=a\n\
# a comment\n\
ts=2 level=warn msg=\"disk \\\"sda\\\" almost full\" host=b\n\
\n\
ts=3 level=info msg=bye host=a";

#[test]
fn parses_fields() {
    let records = parse(LOG).unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].line(), 1);
    assert_eq!(records[1].line(), 3);
    assert_eq!(records[0].get("msg").as_deref(), Some("service started"));
    assert_eq!(records[1].get("msg").as_deref(), Some("disk \"sda\" almost full"));
    assert_eq!(records[2].raw(), "ts=3 level=info msg=bye host=a");
    assert_eq!(records[2].get("nope"), None);
}

#[test]
fn reports_errors() {
    let err = parse("a=1\nb=\"open").unwrap_err();
    assert_eq!((err.line, err.col, err.kind), (2, 3, ErrorKind::UnterminatedQuote));
}

#[test]
fn groups_and_keys() {
    let records = parse(LOG).unwrap();
    let index = Index::build(&records, "host");
    assert_eq!(index.values(), vec!["a", "b"]);
    assert_eq!(index.get("a").len(), 2);
    assert_eq!(index.get("zzz").len(), 0);
    assert_eq!(distinct_keys(&records), vec!["host", "level", "msg", "ts"]);
}
