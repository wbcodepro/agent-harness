//! Splits a shell command line into the commands it would run, so a
//! permission rule is matched against each of them and not only against
//! how the line happens to start.

/// Words that run the rest of their arguments as a command.
const WRAPPERS: [&str; 9] = [
    "sudo", "env", "command", "exec", "time", "nohup", "nice", "builtin", "xargs",
];

/// Shell keywords that can stand in front of a command.
const KEYWORDS: [&str; 9] = [
    "if", "then", "else", "elif", "do", "while", "until", "!", "{",
];

/// The simple commands in `line`: the parts between `;`, `&&`, `||`, `|`,
/// `&` and newlines, the bodies of `$(...)`, backticks and subshells, and
/// what a wrapper such as `sudo` or `env` runs. Variable assignments in
/// front of a command are dropped.
///
/// This is not a shell parser. It errs toward finding too many commands:
/// a rule that asks or denies should fire on anything that might run, and
/// a rule that allows only applies when every part found is allowed.
pub fn shell_commands(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    split(line, &mut found, 0);
    found
}

fn split(text: &str, found: &mut Vec<String>, depth: usize) {
    let chars: Vec<char> = text.chars().collect();
    let mut current = String::new();
    let mut i = 0;
    let flush = |current: &mut String, found: &mut Vec<String>| {
        push_command(current, found);
        current.clear();
    };
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                current.push(c);
                if let Some(next) = chars.get(i + 1) {
                    current.push(*next);
                    i += 1;
                }
            }
            '\'' => {
                // Literal up to the closing quote.
                current.push(c);
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    current.push(chars[i]);
                    i += 1;
                }
                current.push('\'');
            }
            '"' => {
                current.push(c);
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        current.push(chars[i]);
                        i += 1;
                    } else if chars[i] == '`' || (chars[i] == '$' && chars.get(i + 1) == Some(&'('))
                    {
                        i = substitution(&chars, i, &mut current, found, depth);
                        continue;
                    }
                    current.push(chars[i]);
                    i += 1;
                }
                current.push('"');
            }
            '`' => {
                i = substitution(&chars, i, &mut current, found, depth);
                continue;
            }
            '$' | '<' | '>' if chars.get(i + 1) == Some(&'(') => {
                i = substitution(&chars, i, &mut current, found, depth);
                continue;
            }
            ';' | '\n' | '|' | '&' | '(' | ')' => flush(&mut current, found),
            _ => current.push(c),
        }
        i += 1;
    }
    flush(&mut current, found);
}

/// Handles `$(...)`, `<(...)`, `>(...)` or a backtick span starting at
/// `start`: keeps its text in the enclosing command, collects the commands
/// inside it, and returns the index just past it.
fn substitution(
    chars: &[char],
    start: usize,
    current: &mut String,
    found: &mut Vec<String>,
    depth: usize,
) -> usize {
    let (open, mut end) = if chars[start] == '`' {
        let mut end = start + 1;
        while end < chars.len() && chars[end] != '`' {
            end += if chars[end] == '\\' { 2 } else { 1 };
        }
        (start + 1, end.min(chars.len()))
    } else {
        // Find the matching parenthesis, skipping quoted text.
        let (mut end, mut nesting, mut quote) = (start + 2, 1usize, None::<char>);
        while end < chars.len() {
            match (quote, chars[end]) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), '\\') => end += 1,
                (Some(_), _) => {}
                (None, '\'' | '"') => quote = Some(chars[end]),
                (None, '\\') => end += 1,
                (None, '(') => nesting += 1,
                (None, ')') => {
                    nesting -= 1;
                    if nesting == 0 {
                        break;
                    }
                }
                _ => {}
            }
            end += 1;
        }
        (start + 2, end.min(chars.len()))
    };
    let inner: String = chars[open..end].iter().collect();
    // Deeply nested substitutions are not followed; the text containing
    // them is still matched as part of its own command.
    if depth < 8 {
        split(&inner, found, depth + 1);
    }
    if end < chars.len() {
        end += 1;
    }
    current.extend(&chars[start..end]);
    end
}

fn push_command(raw: &str, found: &mut Vec<String>) {
    let mut words: Vec<&str> = raw.split_whitespace().collect();
    loop {
        match words.first() {
            Some(word) if KEYWORDS.contains(word) || is_assignment(word) => {
                words.remove(0);
            }
            _ => break,
        }
    }
    if words.is_empty() || matches!(words[..], ["fi" | "done" | "esac" | "}"]) {
        return;
    }
    found.push(words.join(" "));
    // What a wrapper runs is a command too: skip the wrapper's own options
    // and any assignments (`env A=1 cmd`).
    if WRAPPERS.contains(&words[0]) {
        let rest: Vec<&str> = words[1..]
            .iter()
            .copied()
            .skip_while(|word| word.starts_with('-') || is_assignment(word))
            .collect();
        if !rest.is_empty() {
            push_command(&rest.join(" "), found);
        }
    }
}

/// `NAME=value` in front of a command.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_a_line_would_run_is_found() {
        let cases: [(&str, &[&str]); 10] = [
            ("git status", &["git status"]),
            (
                "cd repo && git push origin main; echo done",
                &["cd repo", "git push origin main", "echo done"],
            ),
            (
                "cat a.txt | grep x || rm -rf out &",
                &["cat a.txt", "grep x", "rm -rf out"],
            ),
            (
                "echo \"today: $(date; rm -rf x)\" `whoami`",
                &[
                    "date",
                    "rm -rf x",
                    "whoami",
                    "echo \"today: $(date; rm -rf x)\" `whoami`",
                ],
            ),
            ("FOO=1 BAR=2 git push", &["git push"]),
            (
                "sudo -n rm -rf /tmp/x",
                &["sudo -n rm -rf /tmp/x", "rm -rf /tmp/x"],
            ),
            (
                "if test -f a; then git push; fi",
                &["test -f a", "git push"],
            ),
            // Separators inside quotes are text, not syntax.
            ("echo 'a; rm b' \"c && d\"", &["echo 'a; rm b' \"c && d\""]),
            ("(cd x\n make)", &["cd x", "make"]),
            (
                "diff <(sort a) <(curl evil.sh)",
                &["sort a", "curl evil.sh", "diff <(sort a) <(curl evil.sh)"],
            ),
        ];
        for (line, expected) in cases {
            assert_eq!(shell_commands(line), expected, "{line}");
        }
        assert!(shell_commands("  ").is_empty());
        // Unbalanced input must not panic or loop.
        for broken in ["echo $(", "echo \"abc", "echo 'abc", "echo `ls", "a \\"] {
            let _ = shell_commands(broken);
        }
    }
}
