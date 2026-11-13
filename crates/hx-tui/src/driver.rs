use std::path::{Path, PathBuf};
use std::sync::Arc;

use hx_agent::{Session, ToolRequest};
use hx_app::{Options, Prompter, Runtime};
use hx_kernel::BoxFuture;
use hx_session::{Block, Record, SessionId, TurnEndReason};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use tokio::sync::{Notify, mpsc, oneshot};

use crate::App;

/// Everything the event loop reacts to.
pub enum Msg {
    Terminal(TermEvent),
    /// A record appended by the running turn.
    Record(Record),
    /// The running turn ended; `Err` carries what to tell the user.
    TurnEnded(Result<(), String>),
    /// A tool call needs the user's decision.
    Approval(String, oneshot::Sender<Option<bool>>),
}

const HELP: &str = "/plan  toggle plan mode (read-only, proposes a plan)\n\
/rewind  undo the last turn, files included\n\
/new  start a new session\n\
/quit  leave (Ctrl-D)\n\
Esc cancels a running turn. PageUp and PageDown scroll.";

struct Asker(mpsc::UnboundedSender<Msg>);

impl Prompter for Asker {
    fn ask<'a>(
        &'a self,
        _request: &'a ToolRequest,
        question: String,
    ) -> BoxFuture<'a, Option<bool>> {
        Box::pin(async move {
            let (answer, answered) = oneshot::channel();
            self.0.send(Msg::Approval(question, answer)).ok()?;
            answered.await.ok().flatten()
        })
    }
}

/// The session being driven and the agent built for it.
struct Live {
    session: Session,
    runtime: Runtime,
}

struct Driver {
    home: PathBuf,
    cwd: PathBuf,
    options: Options,
    sender: mpsc::UnboundedSender<Msg>,
    live: Arc<tokio::sync::Mutex<Live>>,
    session_id: SessionId,
    cancel: Arc<Notify>,
    pending_approval: Option<oneshot::Sender<Option<bool>>>,
}

impl Driver {
    async fn open(
        home: &Path,
        cwd: &Path,
        options: &Options,
        sender: &mpsc::UnboundedSender<Msg>,
        existing: Option<&SessionId>,
    ) -> Result<Live, String> {
        let mut options = options.clone();
        options.prompter = Some(Arc::new(Asker(sender.clone())));
        let runtime = hx_app::build(home, cwd, &options).await?;
        let store = hx_app::session_store(home);
        let mut session = hx_app::open_session(&store, existing, cwd)?;
        let observer = sender.clone();
        session.observe(move |record| {
            let _ = observer.send(Msg::Record(record.clone()));
        });
        Ok(Live { session, runtime })
    }

    fn status(&self, live: &Live) -> String {
        let id = self.session_id.as_str();
        format!(
            "{} · {:?} · {}",
            &id[id.len().saturating_sub(8)..],
            live.runtime.policy.mode,
            self.cwd.display()
        )
    }

    /// Replaces the live session (after /new, /plan or /rewind) and redraws
    /// the transcript from it.
    async fn switch(&mut self, app: &mut App, existing: Option<&SessionId>) -> Result<(), String> {
        let fresh =
            Self::open(&self.home, &self.cwd, &self.options, &self.sender, existing).await?;
        self.session_id = fresh.session.id().clone();
        app.load(fresh.session.records());
        app.plan_mode = self.options.plan;
        app.status = self.status(&fresh);
        *self.live.lock().await = fresh;
        Ok(())
    }

    fn start_turn(&self, text: String) {
        let live = Arc::clone(&self.live);
        let cancel = Arc::clone(&self.cancel);
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let mut live = live.lock().await;
            let Live { session, runtime } = &mut *live;
            let outcome = tokio::select! {
                result = runtime.agent.run_turn(session, vec![Block::text(text)]) => Some(result),
                _ = cancel.notified() => None,
            };
            let result = match outcome {
                Some(result) => result.map_err(|e| e.to_string()),
                None => session
                    .close_open_turn(TurnEndReason::Cancelled)
                    .map_err(|e| e.to_string()),
            };
            let _ = sender.send(Msg::TurnEnded(result));
        });
    }

    /// Returns false when the user asked to leave.
    async fn command(&mut self, app: &mut App, line: &str) -> bool {
        let result = match line.trim() {
            "/quit" | "/exit" => return false,
            "/help" => {
                app.notice(HELP);
                Ok(())
            }
            "/new" => self.switch(app, None).await,
            "/plan" => {
                self.options.plan = !self.options.plan;
                let rebuilt = self.rebuild_agent(app).await;
                match &rebuilt {
                    Ok(()) => app.notice(if self.options.plan {
                        "plan mode: nothing will be changed"
                    } else {
                        "plan mode off"
                    }),
                    Err(_) => self.options.plan = !self.options.plan,
                }
                rebuilt
            }
            "/rewind" => {
                let store = hx_app::session_store(&self.home);
                match hx_app::rewind(&self.home, &store, &self.session_id, None).await {
                    Ok(rewound) => {
                        let switched = self.switch(app, Some(&rewound.session)).await;
                        if switched.is_ok() {
                            app.notice(format!(
                                "turn {} undone; {} file(s) removed, changed files restored",
                                rewound.turn,
                                rewound.restored.removed.len()
                            ));
                        }
                        switched
                    }
                    Err(e) => Err(e),
                }
            }
            other => Err(format!("unknown command {other}; try /help")),
        };
        if let Err(e) = result {
            app.notice(e);
        }
        true
    }

    /// Builds a new agent for the current options and keeps the session.
    async fn rebuild_agent(&mut self, app: &mut App) -> Result<(), String> {
        let mut options = self.options.clone();
        options.prompter = Some(Arc::new(Asker(self.sender.clone())));
        let runtime = hx_app::build(&self.home, &self.cwd, &options).await?;
        let mut live = self.live.lock().await;
        live.runtime = runtime;
        app.plan_mode = self.options.plan;
        app.status = self.status(&live);
        Ok(())
    }
}

/// Runs the interface until the user leaves.
pub async fn drive<B: Backend>(
    terminal: &mut Terminal<B>,
    home: PathBuf,
    cwd: PathBuf,
    options: Options,
    resume: Option<SessionId>,
    sender: mpsc::UnboundedSender<Msg>,
    mut inbox: mpsc::UnboundedReceiver<Msg>,
) -> Result<(), String>
where
    B::Error: std::fmt::Display,
{
    let live = Driver::open(&home, &cwd, &options, &sender, resume.as_ref()).await?;
    let mut app = App::new();
    app.load(live.session.records());
    app.plan_mode = options.plan;
    let mut driver = Driver {
        session_id: live.session.id().clone(),
        home,
        cwd,
        options,
        sender,
        live: Arc::new(tokio::sync::Mutex::new(Live {
            session: live.session,
            runtime: live.runtime,
        })),
        cancel: Arc::new(Notify::new()),
        pending_approval: None,
    };
    app.status = driver.status(&*driver.live.lock().await);
    if app.input.is_empty() && resume.is_none() {
        app.notice("hx — type a request, or /help");
    }

    loop {
        terminal
            .draw(|frame| app.render(frame))
            .map_err(|e| e.to_string())?;
        let Some(msg) = inbox.recv().await else {
            return Ok(());
        };
        match msg {
            Msg::Record(record) => app.apply(&record),
            Msg::TurnEnded(result) => {
                app.busy = false;
                if let Err(e) = result {
                    app.notice(format!("error: {e}"));
                }
            }
            Msg::Approval(question, answer) => {
                app.approval = Some(question);
                driver.pending_approval = Some(answer);
            }
            Msg::Terminal(TermEvent::Key(key)) if key.kind != KeyEventKind::Release => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                if app.approval.is_some() {
                    let decision = match key.code {
                        KeyCode::Char('y' | 'Y') => Some(true),
                        KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(false),
                        _ => None,
                    };
                    if let Some(decision) = decision {
                        app.approval = None;
                        if let Some(answer) = driver.pending_approval.take() {
                            let _ = answer.send(Some(decision));
                        }
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Char('d') if ctrl => return Ok(()),
                    KeyCode::Char('c') if ctrl => {
                        if app.busy {
                            driver.cancel.notify_waiters();
                        } else {
                            return Ok(());
                        }
                    }
                    KeyCode::Esc if app.busy => driver.cancel.notify_waiters(),
                    KeyCode::PageUp => app.scroll += 5,
                    KeyCode::PageDown => app.scroll = app.scroll.saturating_sub(5),
                    KeyCode::Backspace => {
                        app.input.pop();
                    }
                    KeyCode::Char(c) if !ctrl => app.input.push(c),
                    KeyCode::Enter if !app.busy && !app.input.trim().is_empty() => {
                        let line = std::mem::take(&mut app.input);
                        app.scroll = 0;
                        if line.starts_with('/') {
                            if !driver.command(&mut app, &line).await {
                                return Ok(());
                            }
                        } else {
                            app.busy = true;
                            driver.start_turn(line);
                        }
                    }
                    _ => {}
                }
            }
            Msg::Terminal(_) => {}
        }
    }
}

/// Runs the interface on the real terminal.
pub async fn run(
    home: PathBuf,
    cwd: PathBuf,
    options: Options,
    resume: Option<SessionId>,
) -> Result<(), String> {
    let (sender, inbox) = mpsc::unbounded_channel();
    let keys = sender.clone();
    // Terminal input is blocking; a plain thread forwards it. The thread
    // ends with the process.
    std::thread::spawn(move || {
        while let Ok(event) = ratatui::crossterm::event::read() {
            if keys.send(Msg::Terminal(event)).is_err() {
                break;
            }
        }
    });
    let mut terminal = ratatui::init();
    let result = drive(&mut terminal, home, cwd, options, resume, sender, inbox).await;
    ratatui::restore();
    result
}
