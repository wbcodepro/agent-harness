//! What a notification says about a session's activity.
//!
//! The app notifies only while its window is hidden or not focused, when a
//! turn ends or a tool call waits for approval. A stopped turn was stopped
//! by the user and needs no notification.

use hame_web::Activity;

/// A notification to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub session: String,
    pub title: String,
    pub body: String,
}

/// The notification for `activity` in a session titled `title`, if any.
pub fn notice(activity: &Activity, title: Option<&str>) -> Option<Notice> {
    let name = title
        .map(|title| title.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|title| !title.is_empty())
        .map(|title| shorten(&title, 80))
        .unwrap_or_else(|| "Untitled session".into());
    match activity {
        Activity::TurnStarted { .. } => None,
        Activity::TurnEnded {
            cancelled: true, ..
        } => None,
        Activity::TurnEnded {
            session,
            error: None,
            ..
        } => Some(Notice {
            session: session.clone(),
            title: "Turn finished".into(),
            body: name,
        }),
        Activity::TurnEnded {
            session,
            error: Some(error),
            ..
        } => Some(Notice {
            session: session.clone(),
            title: "Turn stopped with an error".into(),
            body: format!("{name}: {}", shorten(error, 120)),
        }),
        Activity::Approval { session, tool, .. } => Some(Notice {
            session: session.clone(),
            title: "hame needs your approval".into(),
            body: format!("{name}: {tool}"),
        }),
    }
}

fn shorten(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_string();
    }
    let cut: String = text.chars().take(most - 1).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ended(error: Option<&str>, cancelled: bool) -> Activity {
        Activity::TurnEnded {
            session: "s1".into(),
            error: error.map(str::to_string),
            cancelled,
        }
    }

    #[test]
    fn a_finished_turn_names_its_session() {
        assert_eq!(
            notice(&ended(None, false), Some("Fix the build")),
            Some(Notice {
                session: "s1".into(),
                title: "Turn finished".into(),
                body: "Fix the build".into(),
            })
        );
    }

    #[test]
    fn a_failed_turn_says_why() {
        let shown = notice(&ended(Some("the model is unavailable"), false), None).unwrap();
        assert_eq!(shown.title, "Turn stopped with an error");
        assert_eq!(shown.body, "Untitled session: the model is unavailable");
    }

    #[test]
    fn an_approval_names_the_tool() {
        let asked = Activity::Approval {
            session: "s2".into(),
            tool: "bash".into(),
            question: "Run `rm -rf build`?".into(),
        };
        let shown = notice(&asked, Some("Clean up")).unwrap();
        assert_eq!(shown.session, "s2");
        assert_eq!(shown.title, "hame needs your approval");
        assert_eq!(shown.body, "Clean up: bash");
    }

    #[test]
    fn started_and_stopped_turns_are_quiet() {
        assert_eq!(
            notice(
                &Activity::TurnStarted {
                    session: "s".into()
                },
                None
            ),
            None
        );
        assert_eq!(notice(&ended(None, true), None), None);
    }

    #[test]
    fn long_titles_are_shortened() {
        let shown = notice(&ended(None, false), Some(&"x".repeat(200))).unwrap();
        assert_eq!(shown.body.chars().count(), 80);
        assert!(shown.body.ends_with('…'));
    }
}
