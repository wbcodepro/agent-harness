//! The agent loop.
//!
//! A turn takes one user input and runs steps until the model stops calling
//! tools. A step is one model request plus the tools it called. Every fact
//! is appended to the session log before the loop acts on it, and each
//! request is rebuilt from the log, so the log alone explains what the model
//! saw and did.

use std::sync::Arc;

use hx_kernel::BoxFuture;
use hx_kernel::Chain;
use hx_llm::{Llm, LlmError, Request, SinkError};
use hx_session::{
    Block, Delta, Event, History, Record, SessionError, SessionId, SessionWriter, StopReason,
    TurnEndReason, assemble, derive_history, plan_close,
};
use hx_tools::{Effect, ToolContext, ToolOutput, ToolRunner, ToolSet};
use serde_json::Value;

type Observer = Box<dyn FnMut(&Record) + Send>;

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

pub struct Agent {
    pub llm: Arc<dyn Llm>,
    pub tools: ToolSet,
    pub pipeline: ToolPipeline,
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
        let specs = self.tools.specs();
        if history.tools != specs {
            session.append(Event::ToolsDeclared { tools: specs })?;
        }
        session.append(Event::UserMessage { content: input })?;
        self.checkpoint(session).await?;

        let mut changed = false;
        for step in 1..=self.max_steps {
            session.append(Event::StepStart { turn, step })?;
            for hook in &self.hooks {
                hook.before_step(session).await?;
            }

            let mut request = Request::from(session.history());
            request.max_tokens = self.max_tokens;

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

            let mut called_tools = false;
            let mut may_have_changed = false;
            for block in content {
                let Block::ToolUse { id, name, input } = block else {
                    continue;
                };
                called_tools = true;
                if completion.stop == StopReason::Refusal {
                    session.append(Event::ToolResult {
                        call_id: id,
                        content: vec![Block::text("Not run: the provider declined this response.")],
                        is_error: true,
                    })?;
                    continue;
                }
                if completion.stop == StopReason::MaxTokens {
                    // The call may be cut short; running it would act on
                    // arguments the model never finished writing.
                    session.append(Event::ToolResult {
                        call_id: id,
                        content: vec![Block::text(
                            "Not run: the response hit its token limit before this tool call \
                             was complete. Send the call again.",
                        )],
                        is_error: true,
                    })?;
                    continue;
                }
                session.append(Event::ToolCall {
                    call_id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                })?;
                may_have_changed |= self
                    .tools
                    .get(&name)
                    .is_some_and(|tool| tool.effect() != Effect::ReadOnly);
                let output = self.execute(id.clone(), name, input).await;
                session.append(Event::ToolResult {
                    call_id: id,
                    content: vec![Block::text(output.text)],
                    is_error: output.is_error,
                })?;
            }

            if may_have_changed {
                changed = true;
                self.checkpoint(session).await?;
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
                let mut follow_up = None;
                for hook in &self.hooks {
                    follow_up = hook.before_stop(session.records(), changed).await;
                    if follow_up.is_some() {
                        break;
                    }
                }
                if let Some(content) = follow_up {
                    session.append(Event::UserMessage { content })?;
                    // What the hook objected to has been reported; only new
                    // changes count from here.
                    changed = false;
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
            Err(reason) => eprintln!("hx: checkpoint skipped: {reason}"),
        }
        Ok(())
    }

    async fn execute(&self, call_id: String, name: String, input: Value) -> ToolOutput {
        let Some(tool) = self.tools.get(&name) else {
            return ToolOutput::error(format!("There is no tool named {name:?}."));
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
                async move { self.runner.run(&tool, &request.input, context).await }
            })
            .await
    }
}
