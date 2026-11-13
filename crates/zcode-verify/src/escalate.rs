//! What to do when the model is stuck: tell it that it is repeating itself,
//! and, when a stronger model is configured, hand the rest of the turn to
//! it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use zcode_agent::{AgentError, Session, ToolRequest, TurnHook};
use zcode_kernel::{BoxFuture, Layer, Next};
use zcode_llm::{Completion, DeltaSink, Llm, LlmError, Request};
use zcode_session::{Block, Event, Record};
use zcode_tools::ToolOutput;

use crate::VERIFY_PREFIX;

/// Starts the note added to a tool result that keeps coming back the same.
pub const REPEAT_PREFIX: &str = "[zcode repeat]";

/// How many identical calls with identical results in a row draw the note.
const REPEATS: usize = 3;

/// A call that keeps failing draws the note at every multiple of this,
/// even when each failure reads differently.
const FAILURES: usize = 4;

/// Notices the model making the same call and getting the same result over
/// and over, and says so in the result.
///
/// Weaker models fall into this after a failed edit or a command that keeps
/// failing the same way; nothing in the conversation changes, so nothing
/// gets them out unless the harness points at it.
///
/// The other shape of being stuck is a check that is run again and again
/// between changes and never passes: every result differs a little, so
/// nothing repeats exactly, but nothing is getting closer either.
#[derive(Default)]
pub struct RepeatGuard {
    /// The last call, its result, and how many times in a row it happened.
    last: Mutex<Option<(String, String, usize)>>,
    /// How many times each call has failed since it last succeeded.
    failures: Mutex<HashMap<String, usize>>,
}

impl Layer<ToolRequest, ToolOutput> for RepeatGuard {
    fn call<'a>(
        &'a self,
        req: ToolRequest,
        next: Next<'a, ToolRequest, ToolOutput>,
    ) -> BoxFuture<'a, ToolOutput> {
        let call = format!("{} {}", req.name, req.input);
        Box::pin(async move {
            let mut output = next.run(req).await;
            let failed = {
                let mut failures = self.failures.lock().expect("repeat guard lock poisoned");
                if failures.len() > 256 {
                    failures.clear();
                }
                if output.is_error {
                    let count = failures.entry(call.clone()).or_insert(0);
                    *count += 1;
                    *count
                } else {
                    failures.remove(&call);
                    0
                }
            };
            let mut last = self.last.lock().expect("repeat guard lock poisoned");
            let count = match &*last {
                Some((seen, result, count)) if *seen == call && *result == output.text => count + 1,
                _ => 1,
            };
            *last = Some((call, output.text.clone(), count));
            if count < REPEATS && failed > 0 && failed % FAILURES == 0 {
                output.text.push_str(&format!(
                    "\n{REPEAT_PREFIX} This call has now failed {failed} times without once \
                     succeeding. What you have been changing between attempts is not fixing \
                     it. Stop and read the original request and the files involved again, \
                     and question your assumptions, including whether the thing you are \
                     running is itself right."
                ));
            }
            if count >= REPEATS {
                output.text.push_str(&format!(
                    "\n{REPEAT_PREFIX} You have made this exact call {count} times in a row \
                     and got the same result each time. Repeating it will not help. Look at \
                     the result again and do something different, or stop and say what is \
                     blocking you."
                ));
            }
            output
        })
    }
}

/// How many times this turn the model was told its work failed the
/// project's checks or that it is repeating itself.
pub fn trouble_this_turn(records: &[Record]) -> usize {
    let starts_with = |content: &[Block], prefix: &str| matches!(content.first(), Some(Block::Text { text }) if text.starts_with(prefix));
    records
        .iter()
        .rev()
        .take_while(|record| !matches!(record.event, Event::TurnStart { .. }))
        .filter(|record| match &record.event {
            Event::UserMessage { content } => starts_with(content, VERIFY_PREFIX),
            Event::ToolResult { content, .. } => content
                .iter()
                .any(|block| matches!(block, Block::Text { text } if text.contains(REPEAT_PREFIX))),
            _ => false,
        })
        .count()
}

/// Which of two models answers. Shared between the model wrapper and the
/// hook that decides.
#[derive(Clone, Default)]
pub struct Switch(Arc<AtomicBool>);

/// Sends requests to the usual model, or to the stronger one while the
/// switch is on.
pub struct Escalating {
    pub usual: Arc<dyn Llm>,
    pub stronger: Arc<dyn Llm>,
    pub switch: Switch,
}

impl Llm for Escalating {
    fn stream<'a>(
        &'a self,
        request: &'a Request,
        sink: &'a mut dyn DeltaSink,
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        if self.switch.0.load(Ordering::Relaxed) {
            self.stronger.stream(request, sink)
        } else {
            self.usual.stream(request, sink)
        }
    }
}

/// Hands the rest of a turn to the stronger model once the usual one has
/// been in trouble `after` times, and hands the next turn back.
///
/// The decision is read from the log each step, so a resumed session picks
/// the same model, and every change of model is logged: the log has to say
/// which model produced what, and reasoning signed by one model must not be
/// replayed to the other.
pub struct Escalation {
    pub usual: String,
    pub stronger: String,
    pub after: usize,
    pub switch: Switch,
}

impl TurnHook for Escalation {
    fn before_step<'a>(
        &'a self,
        session: &'a mut Session,
    ) -> BoxFuture<'a, Result<(), AgentError>> {
        Box::pin(async move {
            let records = session.records();
            let escalate = trouble_this_turn(records) >= self.after;
            let wanted = if escalate {
                &self.stronger
            } else {
                &self.usual
            };
            let current = records
                .iter()
                .rev()
                .find_map(|record| match &record.event {
                    Event::ModelSwitched { model } => Some(model),
                    _ => None,
                })
                .unwrap_or(&self.usual);
            if current != wanted {
                session.append(Event::ModelSwitched {
                    model: wanted.clone(),
                })?;
            }
            self.switch.0.store(escalate, Ordering::Relaxed);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use zcode_agent::ToolPipeline;
    use zcode_eval::{Recording, ScriptedLlm, ScriptedResponse};
    use zcode_session::{CreateOptions, SessionStore, StopReason, Usage};
    use zcode_tools::Effect;

    #[tokio::test]
    async fn the_third_identical_result_in_a_row_is_pointed_out() {
        let pipeline = ToolPipeline::new();
        pipeline.push(RepeatGuard::default());
        let run = |command: &'static str, result: &'static str| {
            let request = ToolRequest {
                call_id: "c".into(),
                name: "bash".into(),
                input: json!({ "command": command }),
                effect: Effect::Write,
            };
            pipeline.run(request, move |_| {
                Box::pin(async move { ToolOutput::error(result) })
            })
        };

        assert_eq!(run("make", "boom").await.text, "boom");
        assert_eq!(run("make", "boom").await.text, "boom");
        let third = run("make", "boom").await.text;
        assert!(third.starts_with("boom\n[zcode repeat]"), "{third}");
        assert!(run("make", "boom").await.text.contains("4 times"));
        // A different result, or a different call, starts over.
        assert_eq!(run("make", "other").await.text, "other");
        assert_eq!(run("ls", "other").await.text, "other");
        assert_eq!(run("make", "boom").await.text, "boom");
    }

    #[tokio::test]
    async fn a_check_that_keeps_failing_differently_is_pointed_out_too() {
        let pipeline = ToolPipeline::new();
        pipeline.push(RepeatGuard::default());
        let run = |result: String, failed: bool| {
            let request = ToolRequest {
                call_id: "c".into(),
                name: "bash".into(),
                input: json!({ "command": "make test" }),
                effect: Effect::Execute,
            };
            pipeline.run(request, move |_| {
                let result = result.clone();
                async move {
                    ToolOutput {
                        text: result,
                        is_error: failed,
                    }
                }
            })
        };
        for attempt in 1..=3 {
            assert!(
                !run(format!("error {attempt}"), true)
                    .await
                    .text
                    .contains(REPEAT_PREFIX)
            );
        }
        let fourth = run("error 4".into(), true).await.text;
        assert!(fourth.contains("failed 4 times"), "{fourth}");
        assert!(
            !run("error 5".into(), true)
                .await
                .text
                .contains(REPEAT_PREFIX)
        );
        // Passing once clears the count.
        assert_eq!(run("ok".into(), false).await.text, "ok");
        for attempt in 1..=3 {
            assert!(
                !run(format!("again {attempt}"), true)
                    .await
                    .text
                    .contains(REPEAT_PREFIX)
            );
        }
    }

    fn scripted(text: &str) -> Arc<dyn Llm> {
        Arc::new(ScriptedLlm::new(Recording {
            responses: (0..4).map(|_| ScriptedResponse::text(text)).collect(),
        }))
    }

    async fn answer(llm: &dyn Llm) -> String {
        let request = Request {
            system: None,
            tools: Vec::new(),
            messages: Vec::new(),
            max_tokens: None,
            cache_key: None,
        };
        let mut text = String::new();
        let mut sink = |delta: zcode_session::Delta| -> Result<(), zcode_llm::SinkError> {
            if let zcode_session::Delta::Text { text: piece } = delta {
                text.push_str(&piece);
            }
            Ok(())
        };
        llm.stream(&request, &mut sink).await.unwrap();
        text
    }

    #[tokio::test]
    async fn repeated_trouble_hands_the_turn_to_the_stronger_model_and_the_next_turn_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path());
        let writer = store
            .create(CreateOptions {
                id: None,
                cwd: "/work".into(),
            })
            .unwrap();
        let mut session = Session::new(store.read(writer.id()).unwrap(), writer);

        let switch = Switch::default();
        let llm = Escalating {
            usual: scripted("small"),
            stronger: scripted("big"),
            switch: switch.clone(),
        };
        let hook = Escalation {
            usual: "small-model".into(),
            stronger: "big-model".into(),
            after: 2,
            switch,
        };
        let failed_checks = || Event::UserMessage {
            content: vec![Block::text(format!("{VERIFY_PREFIX} The checks fail."))],
        };
        let switches = |session: &Session| -> Vec<String> {
            session
                .records()
                .iter()
                .filter_map(|record| match &record.event {
                    Event::ModelSwitched { model } => Some(model.clone()),
                    _ => None,
                })
                .collect()
        };

        session.append(Event::TurnStart { turn: 1 }).unwrap();
        session.append(failed_checks()).unwrap();
        hook.before_step(&mut session).await.unwrap();
        assert_eq!(answer(&llm).await, "small", "one failure is not enough");
        assert!(switches(&session).is_empty());

        session
            .append(Event::ToolResult {
                call_id: "c1".into(),
                content: vec![Block::text(format!("boom\n{REPEAT_PREFIX} You have made"))],
                is_error: true,
            })
            .unwrap();
        hook.before_step(&mut session).await.unwrap();
        assert_eq!(answer(&llm).await, "big");
        hook.before_step(&mut session).await.unwrap();
        assert_eq!(
            switches(&session),
            ["big-model"],
            "logged once, not every step"
        );

        // Reasoning signed by the small model is not replayed to the big one.
        session
            .append(Event::AssistantMessage {
                content: vec![Block::text("done")],
                stop: StopReason::EndTurn,
                usage: Usage::default(),
            })
            .unwrap();

        session.append(Event::TurnStart { turn: 2 }).unwrap();
        hook.before_step(&mut session).await.unwrap();
        assert_eq!(answer(&llm).await, "small");
        assert_eq!(switches(&session), ["big-model", "small-model"]);
    }
}
