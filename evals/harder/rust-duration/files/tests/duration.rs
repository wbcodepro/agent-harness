use duration::parse_duration;

#[test]
fn parses() {
    assert_eq!(parse_duration("90"), Ok(90));
    assert_eq!(parse_duration("45s"), Ok(45));
    assert_eq!(parse_duration("1h30m"), Ok(5400));
    assert_eq!(parse_duration("2d 4h"), Ok(187200));
    assert_eq!(parse_duration(" 1d1h1m1s "), Ok(90061));
    assert_eq!(parse_duration("0m"), Ok(0));
}

#[test]
fn rejects() {
    for bad in ["", "h", "1x", "1m1h", "1h1h", "1.5h", "-5s", "1h 30", "5 m"] {
        assert!(parse_duration(bad).is_err(), "{bad:?} should be an error");
    }
    assert!(parse_duration("99999999999999999999d").is_err());
}
