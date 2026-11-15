//! Messages the user sends while a turn is running.

use std::sync::{Arc, Mutex};

use hame_kernel::BoxFuture;
use hame_session::{Block, Record};

use crate::{AgentError, Session, TurnHook};

/// A queue of messages for the running turn. The interface pushes; the
/// agent delivers each one to the model as a user message at the next
/// point where the conversation can take one: before its next request, or
/// when it was about to stop.
///
/// A message pushed while no turn is running is delivered at the start of
/// the next one.
#[derive(Clone, Default)]
pub struct Steering(Arc<Mutex<Vec<String>>>);

impl Steering {
    pub fn push(&self, text: impl Into<String>) {
        self.0
            .lock()
            .expect("steering lock poisoned")
            .push(text.into());
    }

    /// The messages waiting for the turn, oldest first.
    pub fn pending(&self) -> Vec<String> {
        self.0.lock().expect("steering lock poisoned").clone()
    }

    /// Withdraws the waiting message at `index` (as `pending` listed them),
    /// if it is still waiting and is still `text`. Returns whether it was
    /// withdrawn: a message the turn has already taken cannot be.
    pub fn withdraw(&self, index: usize, text: &str) -> bool {
        let mut waiting = self.0.lock().expect("steering lock poisoned");
        if waiting.get(index).is_some_and(|queued| queued == text) {
            waiting.remove(index);
            true
        } else {
            false
        }
    }

    /// Takes every waiting message, leaving the queue empty.
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().expect("steering lock poisoned"))
    }
}

impl TurnHook for Steering {
    fn before_step<'a>(
        &'a self,
        session: &'a mut Session,
    ) -> BoxFuture<'a, Result<(), AgentError>> {
        Box::pin(async move {
            for text in self.take() {
                session.append(hame_session::Event::UserMessage {
                    content: vec![Block::text(text)],
                })?;
            }
            Ok(())
        })
    }

    fn before_stop<'a>(
        &'a self,
        _records: &'a [Record],
        _changed: bool,
    ) -> BoxFuture<'a, Option<Vec<Block>>> {
        Box::pin(async move {
            let waiting = self.take();
            (!waiting.is_empty()).then(|| vec![Block::text(waiting.join("\n\n"))])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_waiting_message_can_be_listed_and_withdrawn() {
        let steering = Steering::default();
        steering.push("first");
        steering.push("second");
        assert_eq!(steering.pending(), vec!["first", "second"]);
        // The index must still name the same text.
        assert!(!steering.withdraw(0, "second"));
        assert!(steering.withdraw(1, "second"));
        assert_eq!(steering.pending(), vec!["first"]);
        assert_eq!(steering.take(), vec!["first"]);
        assert!(!steering.withdraw(0, "first"));
    }
}
