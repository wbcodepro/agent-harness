//! Hash-anchored edits: `read` shows a short hash of each line next to its
//! number, and `edit_lines` names the lines to replace by that pair.
//!
//! Quoting old text back is where weak models lose edits: one wrong space
//! and nothing matches. An anchor is a few characters to copy, and since
//! it is made from the line's content, an edit aimed at a file that has
//! changed since it was read is refused instead of landing on other lines.

use std::fs;

use hame_session::ToolSpec;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::files::{read_lines, read_schema, write_atomic};
use crate::{BoxFuture, Effect, Tool, ToolContext, ToolOutput, parse_input};

/// Lines shown on each side of a place in the file.
const CONTEXT: usize = 2;
/// A changed region longer than this is shown by its two ends only: the
/// model wrote those lines itself, and what it needs is where they sit.
const MAX_SHOWN: usize = 24;
const SHOWN_ENDS: usize = 8;
const MAX_SHOWN_CHARS: usize = 200;

/// Three hex digits from the line's text. Trailing whitespace is left
/// out, so the same line hashes alike with either line ending.
fn line_hash(line: &str) -> String {
    // FNV-1a, 32 bits.
    let mut hash: u32 = 0x811c_9dc5;
    for byte in line.trim_end().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    // Fold the high bits in, or short lines that differ late look alike.
    format!("{:03x}", (hash ^ (hash >> 12) ^ (hash >> 24)) & 0xfff)
}

/// A line shorter than this (blank, `}`, `end`) reads the same in many
/// places, so its own text cannot tell whether it is still the same line.
const SHORT_LINE: usize = 4;

/// The hash in the anchor of line `index`. A short line's hash takes in
/// the nearest fuller line above it: after lines are added higher up, an
/// old anchor for a blank line would otherwise match whichever blank line
/// now has that number, and the edit would land in the wrong place.
fn anchor_hash(lines: &[&str], index: usize) -> String {
    let line = lines[index];
    if line.trim().len() >= SHORT_LINE {
        return line_hash(line);
    }
    let above = lines[..index]
        .iter()
        .rev()
        .find(|above| above.trim().len() >= SHORT_LINE)
        .copied()
        .unwrap_or("");
    line_hash(&format!("{}\n{}", above.trim_end(), line.trim_end()))
}

/// What goes before line `index`: `12:a3f|`.
fn prefix(lines: &[&str], index: usize) -> String {
    format!("{}:{}|", index + 1, anchor_hash(lines, index))
}

/// Reads `12:a3f`. A model often copies the bar, or the whole line, along
/// with the anchor; what follows the bar is ignored.
fn parse_anchor(raw: &str) -> Option<(usize, String)> {
    let anchor = raw.split('|').next()?.trim();
    let (number, hash) = anchor.split_once(':')?;
    let number: usize = number.trim().parse().ok().filter(|number| *number > 0)?;
    let hash = hash.trim().to_ascii_lowercase();
    (hash.len() == 3 && hash.chars().all(|c| c.is_ascii_hexdigit())).then_some((number, hash))
}

/// A line of the file without its ending, and the ending.
fn split_ending(line: &str) -> (&str, &str) {
    if let Some(text) = line.strip_suffix("\r\n") {
        (text, "\r\n")
    } else if let Some(text) = line.strip_suffix('\n') {
        (text, "\n")
    } else {
        (line, "")
    }
}

/// The lines `new_text` stands for. One newline at the end is taken as
/// closing the last line, not as asking for an empty one after it.
fn new_lines(new_text: &str) -> Vec<&str> {
    if new_text.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = new_text
        .strip_suffix('\n')
        .unwrap_or(new_text)
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    // Lines copied from read's output with their anchors still on: the
    // anchors are not part of the file.
    let bare: Option<Vec<&str>> = lines
        .iter()
        .map(|line| {
            let (anchor, text) = line.split_once('|')?;
            let (number, hash) = anchor.split_once(':')?;
            let plain = !number.is_empty()
                && number.bytes().all(|b| b.is_ascii_digit())
                && hash.len() == 3
                && hash.bytes().all(|b| b.is_ascii_hexdigit());
            plain.then_some(text)
        })
        .collect();
    bare.unwrap_or(lines)
}

/// The anchored lines around each span of `lines`, a span being a 0-based
/// first line and a length. Spans must be in file order.
fn show(lines: &[&str], spans: &[(usize, usize)]) -> String {
    // Windows that touch are shown as one.
    let mut windows: Vec<(usize, usize)> = Vec::new();
    for &(start, len) in spans {
        let from = start.saturating_sub(CONTEXT).min(lines.len());
        let to = (start + len + CONTEXT).min(lines.len());
        match windows.last_mut() {
            Some(last) if from <= last.1 => last.1 = last.1.max(to),
            _ => windows.push((from, to)),
        }
    }
    let mut out = String::new();
    let line = |index: usize| {
        let text: String = lines[index].chars().take(MAX_SHOWN_CHARS).collect();
        format!("{}{text}\n", prefix(lines, index))
    };
    for (at, &(from, to)) in windows.iter().enumerate() {
        if at > 0 {
            out.push_str("...\n");
        }
        if to - from > MAX_SHOWN {
            (from..from + SHOWN_ENDS).for_each(|index| out.push_str(&line(index)));
            out.push_str(&format!(
                "[{} more lines; read them to get their anchors]\n",
                to - from - 2 * SHOWN_ENDS
            ));
            (to - SHOWN_ENDS..to).for_each(|index| out.push_str(&line(index)));
        } else {
            (from..to).for_each(|index| out.push_str(&line(index)));
        }
    }
    out
}

/// `read`, with each line's anchor in place of its bare number.
pub struct ReadAnchored;

impl Tool for ReadAnchored {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".into(),
            description: "Read a text file. Each line comes back as `number:hash|text`, for \
                          example `12:a3f|    let x = 1;`. The `12:a3f` part is the line's \
                          anchor, which edit_lines takes; the text starts right after the \
                          bar. Use offset and limit to page through files longer than 2000 \
                          lines."
                .into(),
            input_schema: read_schema(),
        }
    }

    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    fn worker_name(&self) -> String {
        "read#anchored".into()
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move { read_lines(input, ctx, prefix) })
    }
}

/// Replaces ranges of lines named by their anchors.
pub struct EditLines;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditLinesInput {
    path: String,
    edits: Vec<LineEdit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LineEdit {
    from: String,
    to: Option<String>,
    new_text: String,
}

/// An edit with its anchors checked: lines `from..=to`, 0-based.
struct Range<'a> {
    from: usize,
    to: usize,
    new: Vec<&'a str>,
}

impl Tool for EditLines {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit_lines".into(),
            description: "Change a file by replacing lines, named by the `number:hash` anchors \
                          that read shows before each line. Each edit replaces the lines from \
                          `from` to `to`, both included, with new_text. To delete lines, give \
                          an empty new_text. To insert, replace one line with itself plus the \
                          new lines. Several edits to one file go in one call; their ranges \
                          must not overlap. If an anchor no longer matches the file, nothing \
                          is changed: read the file again. The result shows the changed lines \
                          with their new anchors."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Absolute path, or relative to the working directory."},
                    "edits": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "from": {"type": "string", "description": "Anchor of the first line to replace, such as `12:a3f`."},
                                "to": {"type": "string", "description": "Anchor of the last line to replace. Leave out to replace only the `from` line."},
                                "new_text": {"type": "string", "description": "The lines to put there, without anchors. Empty deletes the lines."}
                            },
                            "required": ["from", "new_text"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["path", "edits"],
                "additionalProperties": false
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let input: EditLinesInput = match parse_input(input) {
                Ok(input) => input,
                Err(output) => return output,
            };
            if input.edits.is_empty() {
                return ToolOutput::error("edits must hold at least one edit.");
            }
            let path = ctx.resolve(&input.path);
            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(e) => return ToolOutput::error(format!("Cannot read {}: {e}", path.display())),
            };
            if text.is_empty() {
                return ToolOutput::error(format!(
                    "{} is empty, so it has no lines to replace. Use write to fill it.",
                    path.display()
                ));
            }
            let pieces: Vec<(&str, &str)> = text.split_inclusive('\n').map(split_ending).collect();
            let lines: Vec<&str> = pieces.iter().map(|(line, _)| *line).collect();

            // Every anchor is checked before anything is changed, so a
            // call either lands whole or not at all.
            let mut ranges: Vec<Range> = Vec::new();
            let mut stale: Vec<String> = Vec::new();
            let mut stale_at: Vec<(usize, usize)> = Vec::new();
            for edit in &input.edits {
                let mut ends = [0usize; 2];
                let anchors = [&edit.from, edit.to.as_ref().unwrap_or(&edit.from)];
                for (index, (end, raw)) in ends.iter_mut().zip(anchors).enumerate() {
                    let Some((number, hash)) = parse_anchor(raw) else {
                        return ToolOutput::error(format!(
                            "`{raw}` is not an anchor. An anchor is the `number:hash` that read \
                             shows before a line, such as `12:a3f`. Nothing was written."
                        ));
                    };
                    *end = number - 1;
                    // Without `to` the one anchor stands for both ends: check it,
                    // and report it, once.
                    if index == 1 && edit.to.is_none() {
                        continue;
                    }
                    match lines.get(number - 1) {
                        Some(_) if anchor_hash(&lines, number - 1) == hash => {}
                        Some(_) => {
                            stale.push(format!(
                                "`{number}:{hash}` is stale: line {number} is now `{number}:{}`.",
                                anchor_hash(&lines, number - 1)
                            ));
                            stale_at.push((number - 1, 1));
                        }
                        None => {
                            stale.push(format!(
                                "`{number}:{hash}` is stale: the file now has {} lines.",
                                lines.len()
                            ));
                            stale_at.push((lines.len() - 1, 1));
                        }
                    }
                }
                let [from, to] = ends;
                if from > to {
                    return ToolOutput::error(format!(
                        "`from` ({}) is after `to` ({}). Nothing was written.",
                        edit.from.trim(),
                        anchors[1].trim()
                    ));
                }
                ranges.push(Range {
                    from,
                    to,
                    new: new_lines(&edit.new_text),
                });
            }
            if !stale.is_empty() {
                stale_at.sort_unstable();
                return ToolOutput::error(format!(
                    "Nothing was written to {}: it has changed since you read it.\n{}\nThe \
                     lines there now:\n{}Read the file again and use the anchors it shows.",
                    path.display(),
                    stale.join("\n"),
                    show(&lines, &stale_at)
                ));
            }
            ranges.sort_by_key(|range| range.from);
            if let Some(pair) = ranges.windows(2).find(|pair| pair[1].from <= pair[0].to) {
                return ToolOutput::error(format!(
                    "Two edits overlap: lines {}-{} and lines {}-{}. Nothing was written. Give \
                     each line to one edit only.",
                    pair[0].from + 1,
                    pair[0].to + 1,
                    pair[1].from + 1,
                    pair[1].to + 1
                ));
            }

            // New lines end the way the file's lines do.
            let eol = match pieces[0].1 {
                "" => "\n",
                ending => ending,
            };
            let mut out: Vec<String> = pieces
                .iter()
                .map(|(line, ending)| format!("{line}{ending}"))
                .collect();
            // From the bottom up, so the line numbers of the edits still
            // to apply stay true.
            for range in ranges.iter().rev() {
                let last = range.new.len().saturating_sub(1);
                let new = range.new.iter().enumerate().map(|(index, line)| {
                    // The last one keeps the ending of the line it takes
                    // the place of, which at the end of the file may be none.
                    let ending = if index == last {
                        pieces[range.to].1
                    } else {
                        eol
                    };
                    format!("{line}{ending}")
                });
                out.splice(range.from..=range.to, new);
            }
            let mut updated = out.concat();
            // Deleting the last line of a file with no final newline must
            // not give it one.
            if !text.ends_with('\n') {
                let kept = updated.trim_end_matches(['\r', '\n']).len();
                updated.truncate(kept);
            }
            if updated == text {
                return ToolOutput::error(format!(
                    "These edits leave {} exactly as it is. Nothing was written.",
                    path.display()
                ));
            }

            // Where each edit's lines are in the new file.
            let mut spans: Vec<(usize, usize)> = Vec::new();
            let mut shift = 0isize;
            for range in &ranges {
                spans.push((range.from.wrapping_add_signed(shift), range.new.len()));
                shift += range.new.len() as isize - (range.to - range.from + 1) as isize;
            }
            let mut warning = crate::search::syntax_warning(&path, Some(&text), &updated);
            warning.push_str(&crate::search::uses_note(&path, &text, &updated, &ctx.cwd));
            if let Err(e) = write_atomic(&path, updated.as_bytes()) {
                return ToolOutput::error(format!("Cannot write {}: {e}", path.display()));
            }
            let now: Vec<&str> = updated.lines().collect();
            let shown = if now.is_empty() {
                "The file is now empty.".to_string()
            } else {
                format!(
                    "The lines there now, with their anchors:\n{}",
                    show(&now, &spans)
                )
            };
            ToolOutput::ok(format!(
                "Applied {} edit(s) to {}.{warning}\n{shown}",
                ranges.len(),
                path.display()
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolSet;

    /// The anchor `read` shows for the line whose text is `text`.
    async fn anchor(ctx: &ToolContext, path: &str, text: &str) -> String {
        let read = ReadAnchored.call(&json!({"path": path}), ctx).await;
        read.text
            .lines()
            .find_map(|line| {
                let (anchor, rest) = line.split_once('|')?;
                (rest == text).then(|| anchor.to_string())
            })
            .unwrap_or_else(|| panic!("no line `{text}` in:\n{}", read.text))
    }

    async fn edit(ctx: &ToolContext, path: &str, edits: Value) -> ToolOutput {
        EditLines
            .call(&json!({"path": path, "edits": edits}), ctx)
            .await
    }

    #[tokio::test]
    async fn anchors_from_read_are_what_edit_lines_takes() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let path = dir.path().join("a.rs");
        fs::write(&path, "fn main() {\n    let x = 1;   \n    go(x);\n}\n").unwrap();

        let read = ReadAnchored.call(&json!({"path": "a.rs"}), &ctx).await;
        let shown: Vec<&str> = read.text.lines().collect();
        assert_eq!(shown.len(), 4);
        assert!(shown[0].starts_with("1:") && shown[0].ends_with("|fn main() {"));
        // Number, colon, three hex digits, bar, then the line as it is.
        let (anchor, line) = shown[1].split_once('|').unwrap();
        assert_eq!(line, "    let x = 1;   ");
        assert_eq!(anchor, format!("2:{}", line_hash("    let x = 1;")));
        assert_eq!(parse_anchor(anchor).unwrap().1.len(), 3);
        // Paging keeps the file's line numbers.
        let page = ReadAnchored
            .call(&json!({"path": "a.rs", "offset": 3, "limit": 1}), &ctx)
            .await;
        assert!(page.text.starts_with(&format!("{}\n", shown[2])));

        let done = edit(
            &ctx,
            "a.rs",
            json!([{"from": anchor, "new_text": "    let x = 2;"}]),
        )
        .await;
        assert!(!done.is_error, "{}", done.text);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "fn main() {\n    let x = 2;\n    go(x);\n}\n"
        );
        // The result carries the new anchors, good for the next edit
        // without another read.
        let next = format!("2:{}", line_hash("    let x = 2;"));
        assert!(
            done.text.contains(&format!("{next}|    let x = 2;\n")),
            "{}",
            done.text
        );
        let chained = edit(
            &ctx,
            "a.rs",
            json!([{"from": next, "to": shown[2], "new_text": "    go(3);\n"}]),
        )
        .await;
        assert!(!chained.is_error, "{}", chained.text);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "fn main() {\n    go(3);\n}\n"
        );

        // The plain read is as it was.
        let plain = crate::Read.call(&json!({"path": "a.rs"}), &ctx).await;
        assert_eq!(plain.text, "1\tfn main() {\n2\t    go(3);\n3\t}\n");
        // A worker finds each of the two tools called `read`.
        let every = ToolSet::every();
        let found = every.for_worker(&ReadAnchored.worker_name()).unwrap();
        let anchored = found.call(&json!({"path": "a.rs"}), &ctx).await;
        assert!(anchored.text.ends_with("|}\n"), "{}", anchored.text);
        let found = every.for_worker("read").unwrap();
        assert_eq!(found.call(&json!({"path": "a.rs"}), &ctx).await, plain);
        assert!(every.for_worker("edit_lines").is_some());
    }

    #[tokio::test]
    async fn a_stale_anchor_writes_nothing_and_says_what_is_there_now() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let path = dir.path().join("a.py");
        fs::write(&path, "a = 1\nb = 2\nc = 3\nd = 4\ne = 5\n").unwrap();
        let (a, c) = (
            anchor(&ctx, "a.py", "a = 1").await,
            anchor(&ctx, "a.py", "c = 3").await,
        );
        // Someone else changes the file after the read.
        let changed = "a = 1\nb = 2\nc = 30\nd = 4\ne = 5\n";
        fs::write(&path, changed).unwrap();

        // One good edit and one stale one: neither is applied.
        let refused = edit(
            &ctx,
            "a.py",
            json!([{"from": a, "new_text": "a = 0"}, {"from": c, "new_text": "c = 4"}]),
        )
        .await;
        assert!(refused.is_error);
        assert_eq!(fs::read_to_string(&path).unwrap(), changed);
        let now = format!("3:{}", line_hash("c = 30"));
        for needle in [
            "Nothing was written".to_string(),
            format!("`{c}` is stale: line 3 is now `{now}`."),
            format!("{now}|c = 30\n"),
            format!("5:{}|e = 5\n", line_hash("e = 5")),
            "Read the file again".to_string(),
        ] {
            assert!(refused.text.contains(&needle), "{needle}: {}", refused.text);
        }
        assert_eq!(
            refused.text.matches("is stale").count(),
            1,
            "{}",
            refused.text
        );

        // A line past the end, and text that is no anchor at all.
        let past = edit(&ctx, "a.py", json!([{"from": "9:abc", "new_text": "x"}])).await;
        assert!(past.is_error && past.text.contains("now has 5 lines"));
        let odd = edit(&ctx, "a.py", json!([{"from": "line 3", "new_text": "x"}])).await;
        assert!(odd.is_error && odd.text.contains("is not an anchor"));
        assert_eq!(fs::read_to_string(&path).unwrap(), changed);
    }

    #[tokio::test]
    async fn several_edits_apply_together_and_overlapping_ones_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let path = dir.path().join("a.txt");
        let before = "one\ntwo\nthree\nfour\nfive\nsix";
        fs::write(&path, before).unwrap();
        let at = |text: &'static str| anchor(&ctx, "a.txt", text);
        let (one, two, three, four, six) = (
            at("one").await,
            at("two").await,
            at("three").await,
            at("four").await,
            at("six").await,
        );

        let overlap = edit(
            &ctx,
            "a.txt",
            json!([
                {"from": two, "to": four, "new_text": "x"},
                {"from": three, "new_text": "y"},
            ]),
        )
        .await;
        assert!(overlap.is_error);
        assert!(
            overlap.text.contains("lines 2-4 and lines 3-3"),
            "{}",
            overlap.text
        );
        let backwards = edit(
            &ctx,
            "a.txt",
            json!([{"from": four, "to": two, "new_text": "x"}]),
        )
        .await;
        assert!(backwards.is_error && backwards.text.contains("is after"));
        assert_eq!(fs::read_to_string(&path).unwrap(), before);

        // Given out of order: an insertion after the first line, a
        // deletion, a range replaced by one line, and the last line.
        let done = edit(
            &ctx,
            "a.txt",
            json!([
                {"from": six, "new_text": "SIX"},
                {"from": one, "new_text": "one\none and a half"},
                {"from": three, "to": four, "new_text": "3-4\n"},
                {"from": two, "new_text": ""},
            ]),
        )
        .await;
        assert!(!done.is_error, "{}", done.text);
        // No newline at the end before, none after.
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "one\none and a half\n3-4\nfive\nSIX"
        );
        assert!(done.text.starts_with("Applied 4 edit(s)"), "{}", done.text);
        assert!(
            done.text
                .ends_with(&format!("5:{}|SIX\n", anchor_hash(&["five", "SIX"], 1))),
            "{}",
            done.text
        );

        // Lines pasted back with their anchors on lose the anchors, and
        // deleting the last line does not leave a newline behind.
        let half = at("one and a half").await;
        let five = at("five").await;
        let last = at("SIX").await;
        let done = edit(
            &ctx,
            "a.txt",
            json!([
                {"from": half, "new_text": format!("{half}|one and a half\n{five}|1.75")},
                {"from": last, "new_text": ""},
            ]),
        )
        .await;
        assert!(!done.is_error, "{}", done.text);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "one\none and a half\n1.75\n3-4\nfive"
        );
    }

    #[tokio::test]
    async fn windows_line_endings_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let path = dir.path().join("a.txt");
        fs::write(&path, "one\r\ntwo\r\nthree\r\n").unwrap();
        let two = anchor(&ctx, "a.txt", "two").await;
        let three = anchor(&ctx, "a.txt", "three").await;
        let done = edit(
            &ctx,
            "a.txt",
            json!([
                {"from": two, "new_text": "2\n2.5"},
                {"from": three, "new_text": "3\r\n4\r\n"},
            ]),
        )
        .await;
        assert!(!done.is_error, "{}", done.text);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "one\r\n2\r\n2.5\r\n3\r\n4\r\n"
        );
        // The anchors shown do not depend on the line ending.
        assert!(
            done.text
                .contains(&format!("2:{}|2\n", anchor_hash(&["one", "2"], 1)))
        );

        // A short line's anchor depends on the fuller line above it, so
        // an old anchor does not fit another blank line that has taken
        // its number.
        let before = ["def a():", "    pass", "", "def b():", "    pass", ""];
        let after = ["import os", "", "def a():", "    pass", "", "def b():"];
        assert_ne!(anchor_hash(&before, 2), anchor_hash(&after, 1));
        assert_eq!(anchor_hash(&before, 2), anchor_hash(&after, 4));
    }

    // Python files are checked with python3, which the test machines
    // other than Unix ones may not have under that name.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_result_says_what_edit_would_about_syntax_and_uses() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        fs::write(
            dir.path().join("price.py"),
            "def total_price(items):\n    return sum(items)\n",
        )
        .unwrap();
        fs::write(dir.path().join("app.py"), "from price import total_price\n").unwrap();
        let first = anchor(&ctx, "price.py", "def total_price(items):").await;
        let signature = edit(
            &ctx,
            "price.py",
            json!([{"from": first, "new_text": "def total_price(items, tax):"}]),
        )
        .await;
        assert!(
            signature.text.contains("`total_price`") && signature.text.contains("app.py:1"),
            "{}",
            signature.text
        );
        let body = anchor(&ctx, "price.py", "    return sum(items)").await;
        let broken = edit(
            &ctx,
            "price.py",
            json!([{"from": body, "new_text": "    return sum(items"}]),
        )
        .await;
        assert!(!broken.is_error);
        assert!(broken.text.contains("no longer parses"), "{}", broken.text);
    }
}
