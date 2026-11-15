//! What a notification says about a session's activity, and which
//! notifications are shown.
//!
//! The app notifies only while its window is hidden or not focused, when a
//! turn ends or a tool call waits for approval. A stopped turn was stopped
//! by the user and needs no notification.
//!
//! Each notification whose click is reported holds a thread of the
//! notification plugin until the user acts on it or it goes away, so a
//! session gets at most one notification until the user comes back to the
//! window, and what the app remembers about shown notifications is bounded.

use std::collections::HashMap;

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
        .map(one_line)
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
            body: format!("{name}: {}", shorten(&one_line(error), 120)),
        }),
        Activity::Approval {
            session,
            tool,
            question,
        } => {
            let question = one_line(question);
            let asked = if question.is_empty() {
                tool.clone()
            } else {
                shorten(&question, 120)
            };
            Some(Notice {
                session: session.clone(),
                title: "hame needs your approval".into(),
                body: format!("{name}: {asked}"),
            })
        }
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn shorten(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_string();
    }
    let cut: String = text.chars().take(most - 1).collect();
    format!("{}…", cut.trim_end())
}

/// How many shown notifications are remembered for their clicks.
const REMEMBERED: usize = 32;

/// The notifications shown: which session each is about, and which
/// sessions have one the user has not come back for yet.
#[derive(Debug, Default)]
pub struct Shown {
    next: i32,
    /// Notification id → session, oldest first.
    sessions: Vec<(i32, String)>,
    /// Sessions with a notification since the user last looked.
    unseen: HashMap<String, i32>,
}

impl Shown {
    /// The id for a new notification about `session`, or `None` when it
    /// already has one the user has not come back for.
    pub fn show(&mut self, session: &str) -> Option<i32> {
        if self.unseen.contains_key(session) {
            return None;
        }
        self.next = self.next.wrapping_add(1).max(1);
        let id = self.next;
        self.unseen.insert(session.to_string(), id);
        self.sessions.retain(|(_, known)| known != session);
        self.sessions.push((id, session.to_string()));
        if self.sessions.len() > REMEMBERED {
            self.sessions.remove(0);
        }
        Some(id)
    }

    /// The session notification `id` is about (when it was clicked).
    pub fn clicked(&mut self, id: i32) -> Option<String> {
        let session = self
            .sessions
            .iter()
            .find(|(known, _)| *known == id)
            .map(|(_, session)| session.clone())?;
        self.unseen.remove(&session);
        Some(session)
    }

    /// The user came back to the window: sessions may notify again.
    pub fn seen(&mut self) {
        self.unseen.clear();
    }
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
        let shown = notice(&ended(Some("the model\nis unavailable"), false), None).unwrap();
        assert_eq!(shown.title, "Turn stopped with an error");
        assert_eq!(shown.body, "Untitled session: the model is unavailable");
    }

    #[test]
    fn an_approval_says_what_is_asked() {
        let asked = Activity::Approval {
            session: "s2".into(),
            tool: "bash".into(),
            question: "Run `rm -rf build`?".into(),
        };
        let shown = notice(&asked, Some("Clean up")).unwrap();
        assert_eq!(shown.session, "s2");
        assert_eq!(shown.title, "hame needs your approval");
        assert_eq!(shown.body, "Clean up: Run `rm -rf build`?");
        let bare = Activity::Approval {
            session: "s2".into(),
            tool: "bash".into(),
            question: " ".into(),
        };
        assert_eq!(notice(&bare, Some("x")).unwrap().body, "x: bash");
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

    #[test]
    fn one_notification_per_session_until_the_user_looks() {
        let mut shown = Shown::default();
        let first = shown.show("a").unwrap();
        assert_eq!(shown.show("a"), None, "a already has one");
        let other = shown.show("b").unwrap();
        assert_ne!(first, other);
        assert_eq!(shown.clicked(first).as_deref(), Some("a"));
        let again = shown.show("a").unwrap();
        assert_eq!(shown.clicked(first), None, "replaced by the newer one");
        assert_eq!(shown.clicked(again).as_deref(), Some("a"));
        shown.seen();
        assert!(shown.show("b").is_some());
    }

    #[test]
    fn what_is_remembered_is_bounded() {
        let mut shown = Shown::default();
        for n in 0..100 {
            shown.show(&format!("s{n}"));
            shown.seen();
        }
        assert!(shown.sessions.len() <= REMEMBERED);
        assert!(shown.unseen.is_empty());
    }
}
