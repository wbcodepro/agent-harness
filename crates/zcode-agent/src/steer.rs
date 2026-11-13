//! Messages the user sends while a turn is running.

use std::sync::{Arc, Mutex};

use zcode_kernel::BoxFuture;
use zcode_session::{Block, Record};

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

    fn take(&self) -> Vec<String> {
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
                session.append(zcode_session::Event::UserMessage {
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
