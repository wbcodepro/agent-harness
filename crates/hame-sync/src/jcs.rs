//! Canonical JSON (RFC 8785, JSON Canonicalization Scheme): object members
//! sorted by the UTF-16 code units of their names, no whitespace, strings
//! escaped as ECMAScript's `JSON.stringify` does, numbers in ECMAScript's
//! shortest form. `packages/sync-js` implements the same; the approval
//! vectors check that both agree.

use serde_json::Value;

/// The canonical serialisation of `value`. Fails for numbers JSON cannot
/// express in ECMAScript (non-finite) or integers beyond 2^53 that would
/// not survive a round trip through a JavaScript number.
pub fn canonical(value: &Value) -> Result<String, String> {
    let mut out = String::new();
    write(value, &mut out)?;
    Ok(out)
}

fn write(value: &Value, out: &mut String) -> Result<(), String> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number(n)?),
        Value::String(s) => string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(item, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                string(key, out);
                out.push(':');
                write(&map[key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

const MAX_SAFE: u64 = (1 << 53) - 1;

fn number(n: &serde_json::Number) -> Result<String, String> {
    if let Some(i) = n.as_u64() {
        if i > MAX_SAFE {
            return Err(format!("{i} is not exact in JavaScript"));
        }
        return Ok(i.to_string());
    }
    if let Some(i) = n.as_i64() {
        if i.unsigned_abs() > MAX_SAFE {
            return Err(format!("{i} is not exact in JavaScript"));
        }
        return Ok(i.to_string());
    }
    let f = n.as_f64().ok_or("not a number")?;
    es_number(f)
}

/// ECMAScript Number::toString for a finite double.
fn es_number(f: f64) -> Result<String, String> {
    if !f.is_finite() {
        return Err("non-finite number".into());
    }
    if f == 0.0 {
        return Ok("0".into());
    }
    // Rust's `{:e}` gives the shortest round-tripping digits, like
    // ECMAScript: "d.ddde±x".
    let sci = format!("{:e}", f.abs());
    let (mantissa, exponent) = sci.split_once('e').expect("scientific form");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exponent.parse::<i32>().map_err(|e| e.to_string())? + 1;
    let sign = if f < 0.0 { "-" } else { "" };
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let e = if e >= 0 {
            format!("+{e}")
        } else {
            e.to_string()
        };
        if k == 1 {
            format!("{digits}e{e}")
        } else {
            format!("{}.{}e{e}", &digits[..1], &digits[1..])
        }
    };
    Ok(format!("{sign}{body}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_follow_ecmascript() {
        for (f, text) in [
            (1.0, "1"),
            (1.5, "1.5"),
            (-0.25, "-0.25"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (1e-7, "1e-7"),
            (1.25e-7, "1.25e-7"),
            (0.000001, "0.000001"),
            (123456.789, "123456.789"),
            (5e-324, "5e-324"),
        ] {
            assert_eq!(es_number(f).unwrap(), text, "{f}");
        }
    }

    #[test]
    fn objects_sort_and_strings_escape() {
        let value =
            json!({"b": [1, true, null], "a": "x\"\\\n\u{1}é", "€": 1, "\u{1d11e}": 2, "z": {}});
        // U+20AC (0x20ac) sorts before U+1D11E (surrogates 0xd834...) by UTF-16.
        assert_eq!(
            canonical(&value).unwrap(),
            "{\"a\":\"x\\\"\\\\\\n\\u0001é\",\"b\":[1,true,null],\"z\":{},\"€\":1,\"\u{1d11e}\":2}"
        );
        assert!(canonical(&json!(9007199254740993u64)).is_err());
    }
}
