//! The agent loop.
//!
//! A turn takes one user input and runs steps until the model stops calling
//! tools. A step is one model request plus the tools it called. Every fact
//! is appended to the session log before the loop acts on it, and each
//! request is rebuilt from the log, so the log alone explains what the model
//! saw and did.

pub mod deferred;
mod steer;

pub use steer::Steering;

use std::sync::Arc;

use hame_kernel::BoxFuture;
use hame_kernel::Chain;
use hame_llm::{Llm, LlmError, Request, SinkError};
use hame_session::{
    Block, Delta, Event, History, Record, SessionError, SessionId, SessionWriter, StopReason,
    ToolSpec, TurnEndReason, assemble, derive_history, plan_close,
};
use hame_tools::{Effect, Tool, ToolContext, ToolOutput, ToolRunner, ToolSet};
use serde_json::Value;

type Observer = Box<dyn FnMut(&Record) + Send>;

/// Recovers tool arguments that are valid JSON wrapped in something else.
///
/// Arguments that did not parse are kept as the raw string. Some models
/// wrap them in a code fence or add text around the object; those can be
/// read without guessing. Anything that would need invented content, such
/// as closing an unfinished object, is left alone so the tool rejects it.
fn repair_input(input: Value) -> Value {
    let Value::String(raw) = &input else {
        return input;
    };
    let trimmed = raw.trim();
    let unfenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|rest| rest.strip_suffix("```"))
        .map(str::trim);
    let braced = match (trimmed.find('{'), trimmed.rfind('}')) {
        (Some(start), Some(end)) if start < end => Some(&trimmed[start..=end]),
        _ => None,
    };
    let repaired = [Some(trimmed), unfenced, braced]
        .into_iter()
        .flatten()
        .find_map(|candidate| {
            serde_json::from_str::<Value>(candidate)
                .ok()
                .filter(Value::is_object)
        });
    repaired.unwrap_or(input)
}

/// An open session: its records so far and the handle that appends more.
pub struct Session {
    records: Vec<Record>,
    writer: SessionWriter,
    observer: Option<Observer>,
}

impl Session {
    /// `records` must be everything already in the log `writer` appends to:
    /// what `SessionStore::open` or `fork` returned, or `SessionStore::read`
    /// right after `create`.
    pub fn new(records: Vec<Record>, writer: SessionWriter) -> Self {
        Self {
            records,
            writer,
            observer: None,
        }
    }

    pub fn id(&self) -> &SessionId {
        self.writer.id()
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    pub fn history(&self) -> History {
        derive_history(&self.records)
    }

    /// Calls `observer` with every record appended from now on.
    pub fn observe(&mut self, observer: impl FnMut(&Record) + Send + 'static) {
        self.observer = Some(Box::new(observer));
    }

    pub fn append(&mut self, event: Event) -> Result<(), SessionError> {
        let record = self.writer.append(event)?;
        if let Some(observer) = &mut self.observer {
            observer(&record);
        }
        self.records.push(record);
        Ok(())
    }

    /// Ends whatever turn is in progress for `reason`, settling a partial
    /// response and answering every outstanding tool call. Does nothing when
    /// no turn is open.
    pub fn close_open_turn(&mut self, reason: TurnEndReason) -> Result<(), SessionError> {
        for event in plan_close(&self.records, reason) {
            self.append(event)?;
        }
        Ok(())
    }
}

/// A tool call on its way to execution, as policy layers see it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolRequest {
    pub call_id: String,
    pub name: String,
    pub input: Value,
    pub effect: Effect,
}

/// Middleware around every tool execution. A layer can deny a call by
/// returning an error output without calling `next`, or rewrite the request
/// or the output.
pub type ToolPipeline = Chain<ToolRequest, ToolOutput>;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error("the turn was stopped after {0} steps without finishing")]
    StepLimit(u64),
    #[error("{0}")]
    Hook(String),
}

/// Saves the state of the workspace so a session can be rewound to it.
pub trait Checkpointer: Send + Sync {
    /// Returns an id for the saved state.
    fn snapshot(&self) -> BoxFuture<'_, Result<String, String>>;
}

/// Extension points inside a turn.
pub trait TurnHook: Send + Sync {
    /// Runs before each model request, with the session as the request will
    /// see it. May append events (e.g. a compaction).
    fn before_step<'a>(
        &'a self,
        _session: &'a mut Session,
    ) -> BoxFuture<'a, Result<(), AgentError>> {
        Box::pin(async { Ok(()) })
    }

    /// Runs right after a model response is logged, before its tool calls
    /// run. The response is the last record.
    fn after_response(&self, _records: &[Record]) {}

    /// Runs when the model has stopped calling tools and the turn is about
    /// to end, with the session's records. `changed` says whether any tool that can change the workspace
    /// ran this turn. Returning input keeps the turn going with that input
    /// as a new user message.
    fn before_stop<'a>(
        &'a self,
        _records: &'a [Record],
        _changed: bool,
    ) -> BoxFuture<'a, Option<Vec<Block>>> {
        Box::pin(async { None })
    }
}

/// Starts the message telling the model the turn is about to run out.
pub const STEPS_LEFT_PREFIX: &str = "[hame limit]";
const STEPS_LEFT_WARNING: u64 = 3;
/// The argument that asks for a command to be run after a change.
pub const THEN_RUN: &str = "then_run";
const SHELL_TOOL: &str = "bash";
/// The tools that take [`THEN_RUN`].
const FUSED_TOOLS: [&str; 4] = ["edit", "edit_lines", "write", "apply_patch"];
/// How often in one turn a response cut off by the output limit is asked
/// to carry on before the turn ends as it stands.
const MAX_CONTINUATIONS: u32 = 2;

pub struct Agent {
    pub llm: Arc<dyn Llm>,
    pub tools: ToolSet,
    /// Let a file change carry the command that checks it.
    pub fuse_actions: bool,
    /// Shared, so a tool that makes calls of its own can send them through
    /// the same layers.
    pub pipeline: Arc<ToolPipeline>,
    pub tool_context: ToolContext,
    /// Executes tool bodies, in process or in a confined worker.
    pub runner: Arc<dyn ToolRunner>,
    /// When set, the workspace is saved at the start of each turn and after
    /// every step that ran a tool able to change it.
    pub checkpointer: Option<Arc<dyn Checkpointer>>,
    pub hooks: Vec<Arc<dyn TurnHook>>,
    /// System prompt. Empty means none.
    pub system: String,
    /// Upper bound on model requests in one turn.
    pub max_steps: u64,
    pub max_tokens: Option<u32>,
    /// Most tool calls run from one model response; the rest are refused.
    /// Stops a model that is stuck emitting calls.
    pub max_tool_calls: usize,
    /// Tools that are not declared to the model but can be run through
    /// `tool_call`. See [`deferred`].
    pub deferred: ToolSet,
}

impl Agent {
    /// Runs one turn to completion.
    ///
    /// On error the turn is closed in the log as failed before returning, so
    /// the session is always left ready for another turn. If the returned
    /// future is dropped instead, call [`Session::close_open_turn`].
    pub async fn run_turn(
        &self,
        session: &mut Session,
        input: Vec<Block>,
    ) -> Result<(), AgentError> {
        let result = self.turn(session, input).await;
        if result.is_err() {
            // Best effort: if the log itself is failing, the original error
            // is the one worth reporting, and reopening the session repairs it.
            let _ = session.close_open_turn(TurnEndReason::Failed);
        }
        result
    }

    async fn turn(&self, session: &mut Session, input: Vec<Block>) -> Result<(), AgentError> {
        let turn = session
            .records()
            .iter()
            .filter_map(|record| match record.event {
                Event::TurnStart { turn } => Some(turn),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            + 1;
        session.append(Event::TurnStart { turn })?;

        let history = session.history();
        if history.system.as_deref().unwrap_or("") != self.system {
            session.append(Event::SystemMessage {
                text: self.system.clone(),
            })?;
        }
        let specs = self.specs();
        if history.tools != specs {
            session.append(Event::ToolsDeclared { tools: specs })?;
        }
        session.append(Event::UserMessage { content: input })?;
        self.checkpoint(session).await?;

        // One flag per hook: a hook that has spoken about a change has had
        // its say, but the hooks after it have not seen that change yet.
        let mut changed = vec![false; self.hooks.len()];
        let mut cut_short = 0;
        for step in 1..=self.max_steps {
            session.append(Event::StepStart { turn, step })?;
            for hook in &self.hooks {
                hook.before_step(session).await?;
            }

            let mut request = Request::from(session.history());
            request.max_tokens = self.max_tokens;
            request.cache_key = Some(session.id().to_string());

            let mut deltas: Vec<Delta> = Vec::new();
            let completion = {
                let mut journal = |delta: Delta| -> Result<(), SinkError> {
                    session.append(Event::AssistantDelta {
                        delta: delta.clone(),
                    })?;
                    deltas.push(delta);
                    Ok(())
                };
                self.llm.stream(&request, &mut journal).await?
            };

            let content = assemble(&deltas);
            session.append(Event::AssistantMessage {
                content: content.clone(),
                stop: completion.stop,
                usage: completion.usage,
            })?;

            for hook in &self.hooks {
                hook.after_response(session.records());
            }

            let calls: Vec<(String, String, Value)> = content
                .into_iter()
                .filter_map(|block| match block {
                    Block::ToolUse { id, name, input } => Some((id, name, repair_input(input))),
                    _ => None,
                })
                .collect();
            let called_tools = !calls.is_empty();
            let mut may_have_changed = false;

            let not_run = match completion.stop {
                StopReason::Refusal => Some("Not run: the provider declined this response."),
                // The call may be cut short; running it would act on
                // arguments the model never finished writing.
                StopReason::MaxTokens => Some(
                    "Not run: the response hit its token limit before this tool call was \
                     complete. Send the call again.",
                ),
                StopReason::EndTurn | StopReason::ToolUse => None,
            };
            if let Some(reason) = not_run {
                for (id, _, _) in calls {
                    session.append(Event::ToolResult {
                        call_id: id,
                        content: vec![Block::text(reason)],
                        is_error: true,
                    })?;
                }
            } else {
                // Neighbouring calls that are safe to overlap run together;
                // everything else runs alone, in the order the model gave.
                let mut calls = calls;
                for (id, _, _) in calls.drain(self.max_tool_calls.min(calls.len())..) {
                    session.append(Event::ToolResult {
                        call_id: id,
                        content: vec![Block::text(format!(
                            "Not run: more than {} tool calls in one response. Continue with \
                             fewer calls at a time.",
                            self.max_tool_calls
                        ))],
                        is_error: true,
                    })?;
                }
                let mut remaining = calls.into_iter().peekable();
                while let Some(first) = remaining.next() {
                    let overlaps = |name: &str, input: &Value| {
                        self.resolve(name, input)
                            .is_some_and(|(tool, _, _)| tool.concurrent())
                    };
                    let mut batch = vec![first];
                    if overlaps(&batch[0].1, &batch[0].2) {
                        while let Some(next) =
                            remaining.next_if(|(_, name, input)| overlaps(name, input))
                        {
                            batch.push(next);
                        }
                    }
                    for (id, name, input) in &batch {
                        session.append(Event::ToolCall {
                            call_id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        })?;
                        may_have_changed |= self
                            .resolve(name, input)
                            .is_some_and(|(tool, _, _)| tool.effect() != Effect::ReadOnly);
                    }
                    let ids: Vec<String> = batch.iter().map(|(id, _, _)| id.clone()).collect();
                    let outputs = futures_util::future::join_all(
                        batch
                            .into_iter()
                            .map(|(id, name, input)| self.execute(id, name, input)),
                    )
                    .await;
                    for (id, output) in ids.into_iter().zip(outputs) {
                        session.append(Event::ToolResult {
                            call_id: id,
                            content: vec![Block::text(output.text)],
                            is_error: output.is_error,
                        })?;
                    }
                }
            }

            if may_have_changed {
                changed.fill(true);
                self.checkpoint(session).await?;
            }
            // A turn cut off at the limit ends mid-thought with nothing
            // said. Warned a few requests ahead, the model can wrap up.
            if called_tools
                && self.max_steps > 2 * STEPS_LEFT_WARNING
                && step + STEPS_LEFT_WARNING == self.max_steps
            {
                session.append(Event::UserMessage {
                    content: vec![Block::text(format!(
                        "{STEPS_LEFT_PREFIX} This turn can make {STEPS_LEFT_WARNING} more \
                         requests. Finish what you can verify, then say plainly what is done \
                         and what is left."
                    ))],
                })?;
            }
            session.append(Event::StepEnd { turn, step })?;
            if completion.stop == StopReason::Refusal {
                // Asking again would get the same answer; hand the turn
                // back to the user with the refusal on record.
                session.append(Event::TurnEnd {
                    turn,
                    reason: TurnEndReason::Completed,
                })?;
                return Ok(());
            }
            if !called_tools {
                // An answer that ran into the output limit is not an
                // answer: the model may have spent it all on reasoning.
                if completion.stop == StopReason::MaxTokens && cut_short < MAX_CONTINUATIONS {
                    cut_short += 1;
                    session.append(Event::UserMessage {
                        content: vec![Block::text(
                            "[hame] Your last response hit the output limit before it was \
                             finished. Carry on from where it stopped: act with tools, or give \
                             the rest of the answer briefly.",
                        )],
                    })?;
                    continue;
                }
                let mut follow_up = None;
                for (hook, changed) in self.hooks.iter().zip(changed.iter_mut()) {
                    follow_up = hook.before_stop(session.records(), *changed).await;
                    // This hook has seen the change, whatever it said;
                    // for it, only new changes count from here.
                    *changed = false;
                    if follow_up.is_some() {
                        break;
                    }
                }
                if let Some(content) = follow_up {
                    session.append(Event::UserMessage { content })?;
                    continue;
                }
                session.append(Event::TurnEnd {
                    turn,
                    reason: TurnEndReason::Completed,
                })?;
                return Ok(());
            }
        }
        Err(AgentError::StepLimit(self.max_steps))
    }

    /// A checkpoint that cannot be taken is reported and skipped: losing the
    /// ability to rewind is not a reason to stop the work.
    async fn checkpoint(&self, session: &mut Session) -> Result<(), AgentError> {
        let Some(checkpointer) = &self.checkpointer else {
            return Ok(());
        };
        match checkpointer.snapshot().await {
            Ok(commit) => session.append(Event::Checkpoint { commit })?,
            Err(reason) => eprintln!("hame: checkpoint skipped: {reason}"),
        }
        Ok(())
    }

    /// The tool a call is really for, with its name and input: the named
    /// tool itself, or for `tool_call` the deferred tool it names.
    fn resolve(&self, name: &str, input: &Value) -> Option<(Arc<dyn Tool>, String, Value)> {
        if name == deferred::CALL_TOOL {
            // A name that resolves to nothing is an unknown tool, not a
            // call to `tool_call` for the policy layers to rule on.
            let inner = input["name"].as_str()?;
            let tool = self.deferred.get(inner)?;
            return Some((
                tool,
                inner.to_string(),
                repair_input(input["input"].clone()),
            ));
        }
        Some((self.tools.get(name)?, name.to_string(), input.clone()))
    }

    /// The tools as declared to the model. The tools that change files
    /// gain a `then_run` argument when there is a shell to run it.
    fn specs(&self) -> Vec<ToolSpec> {
        let mut specs = self.tools.specs();
        if self.fuse_actions && self.tools.get(SHELL_TOOL).is_some() {
            for spec in &mut specs {
                if !FUSED_TOOLS.contains(&spec.name.as_str()) {
                    continue;
                }
                if let Some(properties) = spec.input_schema["properties"].as_object_mut() {
                    properties.insert(
                        THEN_RUN.into(),
                        serde_json::json!({
                            "type": "string",
                            "description": "Optional. A shell command to run right after the \
                                            change is made, such as the test that covers it. \
                                            Its output comes back with this result, which \
                                            saves a separate call."
                        }),
                    );
                }
            }
        }
        specs
    }

    /// Runs a call, and for a change that names a follow-up command, that
    /// command too: an edit is nearly always followed by the check for it,
    /// and doing both in one call saves a request to the model.
    async fn execute(&self, call_id: String, name: String, mut input: Value) -> ToolOutput {
        let then_run = match input.as_object_mut() {
            Some(fields) if self.fuse_actions && FUSED_TOOLS.contains(&name.as_str()) => {
                match fields.remove(THEN_RUN) {
                    Some(Value::String(command)) if !command.trim().is_empty() => Some(command),
                    _ => None,
                }
            }
            _ => None,
        };
        let mut output = self.execute_one(call_id.clone(), name, input).await;
        let Some(command) = then_run else {
            return output;
        };
        if output.is_error {
            output.text.push_str(&format!(
                "\n[{THEN_RUN}] Not run, because the change was not made: `{command}`"
            ));
            return output;
        }
        // The command goes through the same layers as any shell call, so
        // rules, approval and the sandbox apply to it as they would.
        let ran = self
            .execute_one(
                format!("{call_id}-then"),
                SHELL_TOOL.into(),
                serde_json::json!({ "command": command }),
            )
            .await;
        output
            .text
            .push_str(&format!("\n\n[{THEN_RUN}] $ {command}\n{}", ran.text));
        if ran.is_error {
            output
                .text
                .push_str("\n[The change above was made; the command after it failed.]");
            output.is_error = true;
        }
        output
    }

    async fn execute_one(&self, call_id: String, name: String, input: Value) -> ToolOutput {
        let Some((tool, name, input)) = self.resolve(&name, &input) else {
            return ToolOutput::error(if name == deferred::CALL_TOOL {
                format!(
                    "There is no tool named {}. Find the exact name with {}.",
                    input["name"],
                    deferred::SEARCH_TOOL
                )
            } else {
                format!("There is no tool named {name:?}.")
            });
        };
        let request = ToolRequest {
            call_id,
            name,
            input,
            effect: tool.effect(),
        };
        let context = &self.tool_context;
        self.pipeline
            .run(request, |request: ToolRequest| {
                let tool = Arc::clone(&tool);
                async move {
                    if tool.host_side() {
                        tool.call(&request.input, context).await
                    } else {
                        self.runner.run(&tool, &request.input, context).await
                    }
                }
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wrapped_arguments_are_recovered_and_unfinished_ones_are_not() {
        let want = json!({"path": "a.rs"});
        for raw in [
            "{\"path\": \"a.rs\"}",
            "```json\n{\"path\": \"a.rs\"}\n```",
            "Here you go: {\"path\": \"a.rs\"} thanks",
        ] {
            assert_eq!(repair_input(json!(raw)), want, "{raw}");
        }
        // Would need invented content: left as it arrived.
        let cut = json!("{\"path\": \"a.r");
        assert_eq!(repair_input(cut.clone()), cut);
        assert_eq!(repair_input(want.clone()), want);
    }
}
