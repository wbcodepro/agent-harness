//! What happens in the sessions of a server, for an app that embeds it.
//!
//! The desktop app notifies from these and asks before quitting while
//! turns run; see [`crate::WebServer::activity`] and
//! [`crate::WebServer::running`].

use tokio::sync::broadcast;

/// A turn starting or ending, or an approval question, in one session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Activity {
    /// A run of turns started in `session`.
    TurnStarted { session: String },
    /// The turns of `session` are over (a message queued while one ran
    /// starts the next before this is sent); `error` says why the last one
    /// failed, `cancelled` that it was stopped.
    TurnEnded {
        session: String,
        error: Option<String>,
        cancelled: bool,
    },
    /// A tool call in `session` waits for the user's approval; `question`
    /// is what the page shows.
    Approval {
        session: String,
        tool: String,
        question: String,
    },
}

impl Activity {
    /// The session it happened in.
    pub fn session(&self) -> &str {
        match self {
            Activity::TurnStarted { session }
            | Activity::TurnEnded { session, .. }
            | Activity::Approval { session, .. } => session,
        }
    }
}

/// A session with a turn running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningTurn {
    pub session: String,
    /// The session's title: the one given, else from its first message.
    pub title: Option<String>,
    /// An approval question waits for an answer.
    pub needs_you: bool,
}

/// Sends one session's activity to whoever embeds the server.
#[derive(Clone)]
pub struct Reporter {
    session: String,
    sender: broadcast::Sender<Activity>,
}

impl Reporter {
    pub fn new(session: String, sender: broadcast::Sender<Activity>) -> Self {
        Self { session, sender }
    }

    pub fn report(&self, activity: impl FnOnce(String) -> Activity) {
        // Nobody listening is fine.
        let _ = self.sender.send(activity(self.session.clone()));
    }
}
