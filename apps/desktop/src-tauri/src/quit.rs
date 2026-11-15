//! What quitting asks when turns are running.
//!
//! Closing the window only hides it, so turns keep running; quitting stops
//! them. When any is running the app asks first, naming them.

use hame_web::RunningTurn;

/// How many turns the question names before "and N more".
const NAMED: usize = 5;

/// The question to ask before quitting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirmation {
    pub title: String,
    pub message: String,
    /// The label of the button that quits.
    pub quit: String,
    /// The label of the button that keeps hame running.
    pub keep: String,
}

/// `None` when nothing is running and the app can quit at once.
pub fn confirmation(running: &[RunningTurn]) -> Option<Confirmation> {
    if running.is_empty() {
        return None;
    }
    let count = running.len();
    let mut lines: Vec<String> = running
        .iter()
        .take(NAMED)
        .map(|turn| {
            let name = name(turn);
            if turn.needs_you {
                format!("• {name} (waiting for your approval)")
            } else {
                format!("• {name}")
            }
        })
        .collect();
    if count > NAMED {
        lines.push(format!("…and {} more", count - NAMED));
    }
    let (title, stop) = if count == 1 {
        ("A turn is still running".to_string(), "Quitting stops it.")
    } else {
        (
            format!("{count} turns are still running"),
            "Quitting stops them.",
        )
    };
    Some(Confirmation {
        title,
        message: format!(
            "{}\n\n{stop} Close the window instead to keep hame working in the background.",
            lines.join("\n")
        ),
        quit: "Quit".into(),
        keep: "Keep Running".into(),
    })
}

/// A turn's session as the user knows it: its title, shortened, or
/// "Untitled session".
fn name(turn: &RunningTurn) -> String {
    let title = turn
        .title
        .as_deref()
        .map(|title| title.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|title| !title.is_empty());
    match title {
        Some(title) if title.chars().count() > 60 => {
            let cut: String = title.chars().take(59).collect();
            format!("“{}…”", cut.trim_end())
        }
        Some(title) => format!("“{title}”"),
        None => "Untitled session".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(title: Option<&str>, needs_you: bool) -> RunningTurn {
        RunningTurn {
            session: "s".into(),
            title: title.map(str::to_string),
            needs_you,
        }
    }

    #[test]
    fn nothing_running_quits_without_asking() {
        assert_eq!(confirmation(&[]), None);
    }

    #[test]
    fn one_turn_is_named() {
        let asked = confirmation(&[turn(Some("Fix the failing test"), false)]).unwrap();
        assert_eq!(asked.title, "A turn is still running");
        assert!(
            asked
                .message
                .starts_with("• “Fix the failing test”\n\nQuitting stops it.")
        );
        assert_eq!(asked.quit, "Quit");
    }

    #[test]
    fn several_turns_are_named_and_a_waiting_one_says_so() {
        let asked =
            confirmation(&[turn(Some("Refactor the parser"), false), turn(None, true)]).unwrap();
        assert_eq!(asked.title, "2 turns are still running");
        assert!(asked.message.contains("• “Refactor the parser”\n"));
        assert!(
            asked
                .message
                .contains("• Untitled session (waiting for your approval)")
        );
        assert!(asked.message.contains("Quitting stops them."));
    }

    #[test]
    fn long_lists_and_long_titles_are_shortened() {
        let many: Vec<RunningTurn> = (0..8)
            .map(|n| turn(Some(&format!("task {n}")), false))
            .collect();
        let asked = confirmation(&many).unwrap();
        assert!(asked.message.contains("task 4"));
        assert!(!asked.message.contains("task 5"));
        assert!(asked.message.contains("…and 3 more"));

        let long = "word ".repeat(40);
        let asked = confirmation(&[turn(Some(&long), false)]).unwrap();
        let first = asked.message.lines().next().unwrap();
        assert!(first.chars().count() <= 2 + 1 + 59 + 2, "{first}");
        assert!(first.ends_with("…”"));
    }

    #[test]
    fn titles_lose_their_line_breaks() {
        let asked = confirmation(&[turn(Some("two\nlines\t here"), false)]).unwrap();
        assert!(asked.message.starts_with("• “two lines here”"));
        let asked = confirmation(&[turn(Some("  \n "), false)]).unwrap();
        assert!(asked.message.starts_with("• Untitled session"));
    }
}
