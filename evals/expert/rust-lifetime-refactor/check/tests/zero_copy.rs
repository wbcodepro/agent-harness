//! Checks the zero-copy API described in DESIGN.md.
use std::borrow::Cow;

use kvlog::{distinct_keys, parse, records, ErrorKind, Field, Index, ParseError, Record};

/// The byte offset of `part` inside `whole`, if `part` is a slice of it.
fn offset(whole: &str, part: &str) -> Option<usize> {
    let base = whole.as_ptr() as usize;
    let at = part.as_ptr() as usize;
    if at >= base && at + part.len() <= base + whole.len() {
        Some(at - base)
    } else {
        None
    }
}

/// Builds the input at run time so that nothing can point into a literal.
fn sample() -> String {
    let mut text = String::new();
    text.push_str("ts=1 level=info msg=\"service started\" host=a\n"); // line 1
    text.push_str("# a comment\n"); // line 2
    text.push_str("ts=2 level=warn msg=\"disk \\\"sda\\\" almost\\tfull\" host=b\n"); // line 3
    text.push_str("   \n"); // line 4
    text.push_str("  ts=3   level=info msg=bye host=a  "); // line 5
    text
}

fn borrowed<'a>(field: &Field<'a>) -> &'a str {
    match &field.value {
        Cow::Borrowed(s) => s,
        Cow::Owned(s) => panic!("value {:?} of key {:?} is owned, expected borrowed", s, field.key),
    }
}

#[test]
fn raw_and_keys_are_slices_of_the_input() {
    let input = sample();
    let recs = parse(&input).unwrap();
    assert_eq!(recs.len(), 3);
    assert_eq!(recs.iter().map(|r| r.line()).collect::<Vec<_>>(), vec![1, 3, 5]);
    let line3 = input.find("ts=2").unwrap();
    let line5 = input.find("  ts=3").unwrap();
    assert_eq!(offset(&input, recs[0].raw()), Some(0));
    assert_eq!(recs[0].raw(), "ts=1 level=info msg=\"service started\" host=a");
    assert_eq!(offset(&input, recs[1].raw()), Some(line3));
    assert_eq!(offset(&input, recs[2].raw()), Some(line5));
    assert_eq!(recs[2].raw(), "  ts=3   level=info msg=bye host=a  ");
    for rec in &recs {
        let start = offset(&input, rec.raw()).unwrap();
        for field in rec.fields() {
            let at = offset(&input, field.key).expect("key is not a slice of the input");
            assert_eq!(&input[at..at + field.key.len()], field.key);
            assert!(at >= start && at < start + rec.raw().len());
        }
    }
    let keys: Vec<&str> = recs[2].fields().iter().map(|f| f.key).collect();
    assert_eq!(keys, vec!["ts", "level", "msg", "host"]);
    assert_eq!(offset(&input, recs[2].fields()[1].key), Some(line5 + 9));
}

#[test]
fn bare_and_plain_quoted_values_are_borrowed() {
    let input = sample();
    let recs = parse(&input).unwrap();
    let f = recs[0].fields();
    assert_eq!(borrowed(&f[0]), "1");
    assert_eq!(offset(&input, borrowed(&f[0])), Some(3));
    assert_eq!(borrowed(&f[1]), "info");
    assert_eq!(offset(&input, borrowed(&f[1])), Some(11));
    assert_eq!(borrowed(&f[2]), "service started");
    assert_eq!(offset(&input, borrowed(&f[2])), Some(21));
    assert_eq!(borrowed(&f[3]), "a");
    assert_eq!(offset(&input, borrowed(&f[3])), Some(input.find("host=a").unwrap() + 5));
    let last = recs[2].fields();
    assert_eq!(offset(&input, borrowed(&last[2])), Some(input.find("bye").unwrap()));
}

#[test]
fn escaped_values_are_owned_and_unescaped() {
    let input = sample();
    let recs = parse(&input).unwrap();
    let msg = &recs[1].fields()[2];
    assert_eq!(msg.key, "msg");
    match &msg.value {
        Cow::Owned(s) => assert_eq!(s, "disk \"sda\" almost\tfull"),
        Cow::Borrowed(_) => panic!("a value with escapes cannot be borrowed"),
    }
    assert_eq!(recs[1].get("msg"), Some("disk \"sda\" almost\tfull"));
    assert_eq!(borrowed(&recs[1].fields()[3]), "b");

    let input = String::from("a=\"x\\\\y\" b=\"\\n\" c=\"tail\\\\\" d=\"\\\"\"");
    let recs = parse(&input).unwrap();
    let values: Vec<&str> = recs[0].fields().iter().map(|f| f.value.as_ref()).collect();
    assert_eq!(values, vec!["x\\y", "\n", "tail\\", "\""]);
    assert!(recs[0].fields().iter().all(|f| matches!(f.value, Cow::Owned(_))));
}

#[test]
fn odd_values() {
    let input = String::from("a= b=\"\" c=x\"y=z\\n d=\"p q\" a=second e==");
    let recs = parse(&input).unwrap();
    let f = recs[0].fields();
    assert_eq!(f.len(), 6);
    assert_eq!(borrowed(&f[0]), "");
    assert_eq!(borrowed(&f[1]), "");
    assert_eq!(borrowed(&f[2]), "x\"y=z\\n");
    assert_eq!(offset(&input, borrowed(&f[2])), Some(10));
    assert_eq!(borrowed(&f[3]), "p q");
    assert_eq!(borrowed(&f[5]), "=");
    assert_eq!(recs[0].get("a"), Some(""));
    assert_eq!(recs[0].get("missing"), None);
}

#[test]
fn multibyte_text() {
    let input = String::from("city=Zürich note=\"日本語 テキスト\" who=\"é\\té\" k=ß\nx=1");
    let recs = parse(&input).unwrap();
    let f = recs[0].fields();
    assert_eq!(borrowed(&f[0]), "Zürich");
    assert_eq!(offset(&input, borrowed(&f[0])), Some(5));
    assert_eq!(borrowed(&f[1]), "日本語 テキスト");
    assert_eq!(offset(&input, borrowed(&f[1])), Some(input.find('日').unwrap()));
    assert_eq!(f[2].value, Cow::<str>::Owned(String::from("é\té")));
    assert_eq!(borrowed(&f[3]), "ß");
    assert_eq!(offset(&input, f[3].key), Some(input.find("k=ß").unwrap()));
    assert_eq!(recs[1].line(), 2);
    assert_eq!(offset(&input, recs[1].raw()), Some(input.len() - 3));
}

fn err(text: &str) -> (usize, usize, ErrorKind) {
    let e: ParseError = parse(text).unwrap_err();
    (e.line, e.col, e.kind)
}

#[test]
fn errors_are_unchanged() {
    assert_eq!(err("a=1\nb=\"open"), (2, 3, ErrorKind::UnterminatedQuote));
    assert_eq!(err("a=1 =x"), (1, 5, ErrorKind::BadKey));
    assert_eq!(err("a=1 \"q\"=x"), (1, 5, ErrorKind::BadKey));
    assert_eq!(err("abc"), (1, 4, ErrorKind::MissingEquals));
    assert_eq!(err("\n\n  abc def=1"), (3, 6, ErrorKind::MissingEquals));
    assert_eq!(err("a=\"x\\qy\""), (1, 5, ErrorKind::BadEscape));
    assert_eq!(err("a=\"x\\"), (1, 5, ErrorKind::BadEscape));
    assert_eq!(err("a=\"x\"y"), (1, 6, ErrorKind::TrailingAfterQuote));
    assert_eq!(err("# c\na=\"x\"\"y\""), (2, 6, ErrorKind::TrailingAfterQuote));
    // the first error on the line wins
    assert_eq!(err("a=\"x\\q"), (1, 5, ErrorKind::BadEscape));
    assert_eq!(err("ok=1 a b=\"open"), (1, 7, ErrorKind::MissingEquals));
}

#[test]
fn columns_count_characters() {
    assert_eq!(err("a=\"é\"x"), (1, 6, ErrorKind::TrailingAfterQuote));
    assert_eq!(err("a=\"日本\\q\""), (1, 6, ErrorKind::BadEscape));
    assert_eq!(err("v=Zürich w=\"öö"), (1, 12, ErrorKind::UnterminatedQuote));
    assert_eq!(err("v=ß é=1"), (1, 5, ErrorKind::BadKey));
    assert_eq!(err("v=ßß key"), (1, 9, ErrorKind::MissingEquals));
    assert_eq!(err("naïve=1"), (1, 3, ErrorKind::MissingEquals));
}

#[test]
fn text_outlives_the_records() {
    let input = sample();
    let (raw, key, value, keys): (&str, &str, &str, Vec<&str>) = {
        let recs: Vec<Record> = parse(&input).unwrap();
        let field = &recs[0].fields()[2];
        let keys = distinct_keys(&recs);
        (recs[2].raw(), field.key, borrowed(field), keys)
    };
    assert_eq!(raw, "  ts=3   level=info msg=bye host=a  ");
    assert_eq!(offset(&input, key), Some(16));
    assert_eq!(value, "service started");
    assert_eq!(keys, vec!["host", "level", "msg", "ts"]);
    for k in keys {
        assert!(offset(&input, k).is_some(), "distinct_keys returned a copy");
    }
}

#[test]
fn record_new_and_traits() {
    let text = String::from("k v");
    let rec = Record::new(7, &text, vec![Field { key: &text[0..1], value: Cow::Borrowed(&text[2..3]) }]);
    assert_eq!(rec.line(), 7);
    assert_eq!(rec.get("k"), Some("v"));
    let copy = rec.clone();
    assert_eq!(copy, rec);
    assert_eq!(offset(&text, copy.raw()), Some(0));
    assert!(format!("{:?}", rec).contains("Record"));
}

#[test]
fn records_iterator_carries_on_after_errors() {
    let input = String::from("a=1\nbad line\n# skip\n\nb=\"x\ny=2 z=\"q\"\n=\n");
    let kept: Vec<Record> = {
        let mut iter = records(&input);
        let first = iter.next().unwrap().unwrap();
        assert_eq!(first.line(), 1);
        let second = iter.next().unwrap().unwrap_err();
        assert_eq!((second.line, second.col, second.kind), (2, 4, ErrorKind::MissingEquals));
        let third = iter.next().unwrap().unwrap_err();
        assert_eq!((third.line, third.col, third.kind), (5, 3, ErrorKind::UnterminatedQuote));
        let fourth = iter.next().unwrap().unwrap();
        assert_eq!(fourth.line(), 6);
        let fifth = iter.next().unwrap().unwrap_err();
        assert_eq!((fifth.line, fifth.col, fifth.kind), (7, 1, ErrorKind::BadKey));
        assert!(iter.next().is_none());
        vec![first, fourth]
    };
    assert_eq!(offset(&input, kept[1].raw()), Some(input.find("y=2").unwrap()));
    assert_eq!(kept[1].get("z"), Some("q"));
    let e = parse(&input).unwrap_err();
    assert_eq!((e.line, e.col, e.kind), (2, 4, ErrorKind::MissingEquals));
    assert_eq!(records("").count(), 0);
    assert_eq!(records("\n # not a comment=1").count(), 1);
    assert!(parse("\n\n#x\n").unwrap().is_empty());
}

#[test]
fn index_refers_to_the_same_records() {
    let input = sample();
    let recs = parse(&input).unwrap();
    let (hits, values, none): (Vec<&Record>, Vec<&str>, usize) = {
        let index = Index::build(&recs, "host");
        (index.get("a").to_vec(), index.values(), index.get("zzz").len())
    };
    assert_eq!(none, 0);
    assert_eq!(values, vec!["a", "b"]);
    assert_eq!(hits.len(), 2);
    assert!(std::ptr::eq(hits[0], &recs[0]), "Index::get returned a copy of the record");
    assert!(std::ptr::eq(hits[1], &recs[2]), "Index::get returned a copy of the record");
    for v in &values {
        assert!(offset(&input, v).is_some(), "Index::values returned a copy");
    }
    // the same strings stay valid as long as the input does
    let raw: &str = hits[1].raw();
    drop(hits);
    assert_eq!(offset(&input, raw), Some(input.find("  ts=3").unwrap()));
}

#[test]
fn index_on_owned_values_borrows_from_the_records() {
    let input = String::from("m=\"a\\tb\" n=1\nn=2\nm=\"a\\tb\" n=3\nm=plain n=4");
    let recs = parse(&input).unwrap();
    let index = Index::build(&recs, "m");
    let values = index.values();
    assert_eq!(values, vec!["a\tb", "plain"]);
    let first = recs[0].get("m").unwrap();
    let third = recs[2].get("m").unwrap();
    assert!(
        std::ptr::eq(values[0].as_ptr(), first.as_ptr()) || std::ptr::eq(values[0].as_ptr(), third.as_ptr()),
        "Index::values copied an owned value"
    );
    let group = index.get("a\tb");
    assert_eq!(group.len(), 2);
    assert!(std::ptr::eq(group[0], &recs[0]) && std::ptr::eq(group[1], &recs[2]));
    assert_eq!(index.get("plain")[0].get("n"), Some("4"));
}

#[test]
fn a_large_log_is_not_copied() {
    let mut input = String::new();
    for i in 0..20_000 {
        input.push_str(&format!("seq={} user=u{} note=\"entry number {}\" tag=t{}\n", i, i % 97, i, i % 5));
    }
    let recs = parse(&input).unwrap();
    assert_eq!(recs.len(), 20_000);
    let mut expect = 0;
    for rec in &recs {
        assert_eq!(offset(&input, rec.raw()), Some(expect));
        expect += rec.raw().len() + 1;
        for field in rec.fields() {
            assert!(offset(&input, field.key).is_some());
            assert!(offset(&input, borrowed(field)).is_some());
        }
    }
    let index = Index::build(&recs, "tag");
    assert_eq!(index.values(), vec!["t0", "t1", "t2", "t3", "t4"]);
    let group = index.get("t3");
    assert_eq!(group.len(), 4_000);
    assert!(std::ptr::eq(group[1], &recs[8]));
    assert_eq!(distinct_keys(&recs), vec!["note", "seq", "tag", "user"]);
}
