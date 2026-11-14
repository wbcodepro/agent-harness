/// Parses a duration such as `1h30m`, `45s`, `2d 4h` or `90` into
/// seconds.
///
/// Units are `d`, `h`, `m`, `s`, each at most once and in that order;
/// a bare number is seconds. Spaces between parts are allowed. Anything
/// else is an error naming the problem.
pub fn parse_duration(input: &str) -> Result<u64, String> {
    todo!()
}
