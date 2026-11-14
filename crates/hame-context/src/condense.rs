//! Leaves the lines of a test or build run that say nothing new out of
//! what the model is shown.
//!
//! A run of two hundred passing tests and one failure is paid for again on
//! every later request; only the failure decides the next step. Lines are
//! dropped by rule, never rewritten: every line that is kept is exactly as
//! the command printed it, and the caller keeps the whole output on disk.

use std::sync::OnceLock;

use regex::Regex;

/// Fewer dropped lines than this are not worth a note and a saved file.
const MIN_DROPPED: usize = 8;
/// A smaller saving than this is eaten by the note about the saved file.
const MIN_SAVED_BYTES: usize = 600;

/// A line that only reports that one test passed or one step of a build is
/// under way. Summary lines (`test result: ok`, `ok pkg 0.2s`, `5 passed`)
/// are not matched: they are the evidence that the run went well.
fn says_nothing_new(line: &str) -> bool {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN
        .get_or_init(|| {
            Regex::new(concat!(
                r"^(?:",
                // cargo test, and Rust doc-tests
                r"test \S.* \.\.\. ok",
                // cargo, pip, npm progress
                r"|\s*(?:Compiling|Checking|Downloading|Downloaded|Fresh|Documenting|Collecting|Installing|Building wheel for|Using cached|Requirement already satisfied)\b.*",
                // pytest -v, unittest -v
                r"|\S+::\S+ PASSED\s*(?:\[\s*\d+%\])?",
                r"|\S.* \.\.\. ok",
                // go test -v
                r"|\s*(?:=== RUN|=== PAUSE|=== CONT|--- PASS:)\s.*",
                // jest, vitest, mocha
                r"|\s*(?:✓|✔|√)\s.*",
                r")\s*$",
            ))
            .expect("valid pattern")
        })
        .is_match(line)
        && !carries_an_outcome(line)
}

/// A progress line that also reports how something came out
/// (`Building wheel for x: finished with status 'error'`) is kept.
fn carries_an_outcome(line: &str) -> bool {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN
        .get_or_init(|| {
            Regex::new(r"(?i)error|fail|warn|denied|not found|cannot|unable|missing|pending")
                .expect("valid pattern")
        })
        .is_match(line)
}

/// `text` without its uninformative lines, and how many were dropped.
/// `None` when too few would go for it to be worth doing.
pub fn condense(text: &str) -> Option<(String, usize)> {
    let dropped = text.lines().filter(|line| says_nothing_new(line)).count();
    if dropped < MIN_DROPPED {
        return None;
    }
    let mut out = String::with_capacity(text.len() / 2);
    let mut run = 0usize;
    let flush = |run: &mut usize, out: &mut String| {
        if *run > 0 {
            out.push_str(&format!(
                "[{run} lines omitted: passing tests or build progress]\n"
            ));
            *run = 0;
        }
    };
    for line in text.split_inclusive('\n') {
        if says_nothing_new(line.trim_end_matches(['\n', '\r'])) {
            run += 1;
        } else {
            flush(&mut run, &mut out);
            out.push_str(line);
        }
    }
    flush(&mut run, &mut out);
    // The note that says where the whole output is costs something too.
    if text.len() - out.len() < MIN_SAVED_BYTES {
        return None;
    }
    Some((out, dropped))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passing_lines_go_and_everything_else_stays_as_printed() {
        let mut log = String::from("running 12 tests\n");
        for n in 0..40 {
            log.push_str(&format!("test parser::case_{n} ... ok\n"));
        }
        log.push_str("test date::iso_week ... FAILED\n");
        log.push_str("test date::leap ... ok\n");
        log.push_str(
            "\nfailures:\n    date::iso_week\n\ntest result: FAILED. 41 passed; 1 failed\n",
        );

        let (shown, dropped) = condense(&log).unwrap();
        assert_eq!(dropped, 41);
        assert!(shown.contains("[40 lines omitted: passing tests or build progress]\n"));
        assert!(shown.contains("test date::iso_week ... FAILED\n"));
        assert!(shown.contains("test result: FAILED. 41 passed; 1 failed\n"));
        assert!(!shown.contains("case_3"));
        // Every kept line is a line of the original.
        for line in shown.lines().filter(|line| !line.starts_with('[')) {
            assert!(log.lines().any(|original| original == line), "{line}");
        }
    }

    #[test]
    fn ordinary_output_is_left_alone() {
        assert!(condense("a\nb\nc\n").is_none());
        let few = "test a ... ok\ntest b ... ok\nall good\n";
        assert!(condense(few).is_none());
        // Words like "passed" inside prose or code do not count.
        let prose: String = (0..20)
            .map(|n| format!("fn check_{n}() {{ assert!(passed); }}\n"))
            .collect();
        assert!(condense(&prose).is_none());
    }

    #[test]
    fn other_runners_are_recognised() {
        let mut log = String::new();
        for n in 0..9 {
            log.push_str(&format!("tests/test_a.py::test_{n} PASSED   [ {n}%]\n"));
            log.push_str(&format!(
                "=== RUN   TestThing{n}\n--- PASS: TestThing{n} (0.00s)\n"
            ));
            log.push_str(&format!("  ✓ handles case {n} 1ms\n"));
            log.push_str(&format!("   Compiling dep{n} v0.1.{n}\n"));
        }
        log.push_str("tests/test_a.py::test_x FAILED   [100%]\n--- FAIL: TestOther (0.01s)\n");
        let (shown, dropped) = condense(&log).unwrap();
        assert_eq!(dropped, 45);
        assert!(shown.contains("test_x FAILED") && shown.contains("--- FAIL: TestOther"));

        // A progress line that says something went wrong stays.
        let mut log: String = (0..60)
            .map(|n| format!("  Collecting package{n}==1.0.{n}\n"))
            .collect();
        log.push_str("  Building wheel for foo (pyproject.toml): finished with status 'error'\n");
        log.push_str("Checking migrations ... FAILED: 3 pending\n");
        let (shown, _) = condense(&log).unwrap();
        assert!(shown.contains("finished with status 'error'"));
        assert!(shown.contains("FAILED: 3 pending"));
    }
}
