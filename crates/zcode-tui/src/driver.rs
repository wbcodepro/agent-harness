use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use zcode_app::Options;
use zcode_session::{Record, SessionId};

use crate::App;

/// Everything the event loop reacts to.
pub enum Msg {
    Terminal(TermEvent),
    /// A JSON-RPC message from the agent.
    Agent(Value),
}

const HELP: &str = "/plan  toggle plan mode (read-only, proposes a plan)\n\
/rewind  undo the last turn, files included\n\
/new  start a new session\n\
/quit  leave (Ctrl-D)\n\
Esc cancels a running turn; Enter while one runs sends your message to it. PageUp and PageDown scroll.";

/// What a request we sent was for, so its response can be acted on.
enum Sent {
    Initialize,
    /// `session/new`, or `session/load` of the given session.
    Open(Option<String>),
    Prompt,
    /// `session/set_mode`; the flag is whether plan mode was requested.
    Mode(bool),
    Rewind,
}

/// The interface's side of the Agent Client Protocol connection.
///
/// The interface never touches a session or an agent directly: like any
/// editor, it sends requests and renders what comes back. It asks for whole
/// session records (`_zcode/record`) so it can show everything the log holds.
struct Client {
    to_agent: mpsc::UnboundedSender<String>,
    next_id: u64,
    sent: HashMap<u64, Sent>,
}

impl Client {
    fn request(&mut self, method: &str, params: Value, purpose: Sent) {
        self.next_id += 1;
        self.sent.insert(self.next_id, purpose);
        let _ = self.to_agent.send(
            json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
                .to_string(),
        );
    }

    fn notify(&self, method: &str, params: Value) {
        let _ = self
            .to_agent
            .send(json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string());
    }

    fn respond(&self, id: &Value, result: Value) {
        let _ = self
            .to_agent
            .send(json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string());
    }
}

struct Driver {
    client: Client,
    cwd: PathBuf,
    /// The session prompts go to; empty until one is open.
    session: String,
    /// The session the transcript currently shows.
    shown: String,
    sandbox: String,
    /// Permission requests awaiting an answer, oldest first: the request id
    /// to answer and the question to show.
    approvals: VecDeque<(Value, String)>,
}

impl Driver {
    fn open(&mut self, existing: Option<&str>) {
        match existing {
            Some(id) => self.client.request(
                "session/load",
                json!({"sessionId": id, "cwd": self.cwd, "mcpServers": []}),
                Sent::Open(Some(id.to_string())),
            ),
            None => self.client.request(
                "session/new",
                json!({"cwd": self.cwd, "mcpServers": []}),
                Sent::Open(None),
            ),
        }
    }

    fn show(&mut self, app: &mut App, session: &str) {
        if self.shown != session {
            // Nothing to clear before the first session; keep the greeting.
            if !self.shown.is_empty() {
                app.reset();
            }
            self.shown = session.to_string();
        }
    }

    fn refresh_status(&self, app: &mut App) {
        let id = &self.session;
        app.status = format!(
            "{} · {} · {}",
            &id[id.len().saturating_sub(8)..],
            self.sandbox,
            self.cwd.display()
        );
    }

    fn next_approval(&self, app: &mut App) {
        app.approval = self.approvals.front().map(|(_, question)| question.clone());
    }

    /// Handles a message from the agent.
    fn receive(&mut self, app: &mut App, message: Value, start_in_plan: &mut bool) {
        match (message["method"].as_str(), message.get("id")) {
            (Some("_zcode/record"), None) => {
                let params = &message["params"];
                let Ok(record) = serde_json::from_value::<Record>(params["record"].clone()) else {
                    return;
                };
                self.show(app, params["sessionId"].as_str().unwrap_or(""));
                if params["replay"] == true {
                    app.apply_replayed(&record);
                } else {
                    app.apply(&record);
                }
            }
            (Some("session/request_permission"), Some(id)) => {
                let question = message["params"]["toolCall"]["title"]
                    .as_str()
                    .unwrap_or("Allow this tool call?")
                    .to_string();
                self.approvals.push_back((id.clone(), question));
                self.next_approval(app);
            }
            // A request we do not serve.
            (Some(_), Some(id)) => {
                let _ = self.client.to_agent.send(
                    json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32601, "message": "not supported"}})
                    .to_string(),
                );
            }
            (Some(_), None) => {}
            (None, Some(id)) => {
                let Some(purpose) = id.as_u64().and_then(|id| self.client.sent.remove(&id)) else {
                    return;
                };
                let error = message["error"]["message"].as_str().map(String::from);
                let result = &message["result"];
                match purpose {
                    Sent::Initialize => {}
                    Sent::Open(loaded) => match error {
                        Some(e) => app.notice(format!("error: {e}")),
                        None => {
                            let id = loaded
                                .or_else(|| result["sessionId"].as_str().map(String::from))
                                .unwrap_or_default();
                            self.show(app, &id);
                            self.session = id;
                            self.sandbox = result["_meta"]["zcode"]["sandbox"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();
                            app.plan_mode = false;
                            self.refresh_status(app);
                            if std::mem::take(start_in_plan) {
                                self.set_plan(true);
                            }
                        }
                    },
                    Sent::Prompt => {
                        app.busy = false;
                        // Questions from a turn that is over have no one
                        // left to hear the answer.
                        self.approvals.clear();
                        self.next_approval(app);
                        if let Some(e) = error {
                            app.notice(format!("error: {e}"));
                        }
                    }
                    Sent::Mode(plan) => match error {
                        Some(e) => app.notice(format!("error: {e}")),
                        None => {
                            app.plan_mode = plan;
                            app.notice(if plan {
                                "plan mode: nothing will be changed"
                            } else {
                                "plan mode off"
                            });
                        }
                    },
                    Sent::Rewind => match error {
                        Some(e) => app.notice(e),
                        None => {
                            let id = result["sessionId"].as_str().unwrap_or("").to_string();
                            self.show(app, &id);
                            self.session = id;
                            app.plan_mode = false;
                            self.refresh_status(app);
                            app.notice(format!(
                                "turn {} undone; {} file(s) removed, changed files restored",
                                result["turn"],
                                result["removed"].as_array().map_or(0, Vec::len)
                            ));
                        }
                    },
                }
            }
            (None, None) => {}
        }
    }

    fn set_plan(&mut self, plan: bool) {
        self.client.request(
            "session/set_mode",
            json!({"sessionId": self.session, "modeId": if plan { "plan" } else { "act" }}),
            Sent::Mode(plan),
        );
    }

    /// Returns false when the user asked to leave.
    fn command(&mut self, app: &mut App, line: &str) -> bool {
        match line.trim() {
            "/quit" | "/exit" => return false,
            "/help" => app.notice(HELP),
            "/new" => self.open(None),
            "/plan" => self.set_plan(!app.plan_mode),
            "/rewind" => self.client.request(
                "_zcode/rewind",
                json!({"sessionId": self.session}),
                Sent::Rewind,
            ),
            other => app.notice(format!("unknown command {other}; try /help")),
        }
        true
    }
}

/// Runs the interface until the user leaves.
///
/// The agent runs behind an in-memory Agent Client Protocol connection, the
/// same protocol an editor would use over stdio.
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
    let mut start_in_plan = options.plan;
    let mut agent_options = options;
    agent_options.plan = false;

    let (ours, theirs) = tokio::io::duplex(1 << 20);
    let (agent_read, agent_write) = tokio::io::split(theirs);
    let agent = tokio::spawn(zcode_acp::serve(
        home,
        agent_options,
        agent_read,
        agent_write,
    ));
    let (from_agent, mut to_agent_stream) = tokio::io::split(ours);
    let (to_agent, mut outgoing) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(line) = outgoing.recv().await {
            if to_agent_stream.write_all(line.as_bytes()).await.is_err()
                || to_agent_stream.write_all(b"\n").await.is_err()
            {
                break;
            }
        }
        // Tell the agent we are gone; dropping a split half does not.
        let _ = to_agent_stream.shutdown().await;
    });
    tokio::spawn(async move {
        let mut lines = BufReader::new(from_agent).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(message) = serde_json::from_str::<Value>(&line)
                && sender.send(Msg::Agent(message)).is_err()
            {
                break;
            }
        }
    });

    let mut driver = Driver {
        client: Client {
            to_agent,
            next_id: 0,
            sent: HashMap::new(),
        },
        cwd,
        session: String::new(),
        shown: String::new(),
        sandbox: String::new(),
        approvals: VecDeque::new(),
    };
    let mut app = App::new();
    driver.client.request(
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {"_meta": {"zcode": {"records": true}}},
            "clientInfo": {"name": "zcode-tui", "version": env!("CARGO_PKG_VERSION")}}),
        Sent::Initialize,
    );
    driver.open(resume.as_ref().map(SessionId::as_str));
    if resume.is_none() {
        app.notice("zcode — type a request, or /help");
    }

    // Keys typed before the session is open are kept and handled once it
    // is, rather than lost.
    let mut early: VecDeque<Msg> = VecDeque::new();
    while !driver.client.sent.is_empty() {
        match inbox.recv().await {
            Some(Msg::Agent(message)) => driver.receive(&mut app, message, &mut start_in_plan),
            Some(other) => early.push_back(other),
            None => return Ok(()),
        }
    }

    let result = loop {
        if let Err(e) = terminal.draw(|frame| app.render(frame)) {
            break Err(e.to_string());
        }
        let next = match early.pop_front() {
            Some(msg) => Some(msg),
            None => inbox.recv().await,
        };
        let Some(msg) = next else {
            break Ok(());
        };
        match msg {
            Msg::Agent(message) => driver.receive(&mut app, message, &mut start_in_plan),
            Msg::Terminal(TermEvent::Key(key)) if key.kind != KeyEventKind::Release => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                if app.approval.is_some() {
                    let option = match key.code {
                        KeyCode::Char('y' | 'Y') => Some("allow-once"),
                        KeyCode::Char('n' | 'N') | KeyCode::Esc => Some("reject-once"),
                        _ => None,
                    };
                    if let Some(option) = option
                        && let Some((id, _)) = driver.approvals.pop_front()
                    {
                        driver.client.respond(
                            &id,
                            json!({"outcome": {"outcome": "selected", "optionId": option}}),
                        );
                        driver.next_approval(&mut app);
                    }
                    continue;
                }
                let cancel = |driver: &Driver| {
                    driver
                        .client
                        .notify("session/cancel", json!({"sessionId": driver.session}));
                };
                match key.code {
                    KeyCode::Char('d') if ctrl => break Ok(()),
                    KeyCode::Char('c') if ctrl => {
                        if app.busy {
                            cancel(&driver);
                        } else {
                            break Ok(());
                        }
                    }
                    KeyCode::Esc if app.busy => cancel(&driver),
                    KeyCode::PageUp => app.scroll += 5,
                    KeyCode::PageDown => app.scroll = app.scroll.saturating_sub(5),
                    KeyCode::Backspace => {
                        app.input.pop();
                    }
                    KeyCode::Char(c) if !ctrl => app.input.push(c),
                    // While a turn runs, what is typed goes to that turn.
                    KeyCode::Enter
                        if app.busy
                            && !app.input.trim().is_empty()
                            && !app.input.starts_with('/')
                            && !driver.session.is_empty() =>
                    {
                        let line = std::mem::take(&mut app.input);
                        driver.client.notify(
                            "_zcode/steer",
                            json!({"sessionId": driver.session, "text": line}),
                        );
                    }
                    KeyCode::Enter
                        if !app.busy
                            && !app.input.trim().is_empty()
                            && !driver.session.is_empty() =>
                    {
                        let line = std::mem::take(&mut app.input);
                        app.scroll = 0;
                        if line.starts_with('/') {
                            if !driver.command(&mut app, &line) {
                                break Ok(());
                            }
                        } else {
                            app.busy = true;
                            driver.client.request(
                                "session/prompt",
                                json!({"sessionId": driver.session,
                                    "prompt": [{"type": "text", "text": line}]}),
                                Sent::Prompt,
                            );
                        }
                    }
                    _ => {}
                }
            }
            Msg::Terminal(_) => {}
        }
    };
    // Closing our end tells the agent to stop its turns and release its
    // sessions; wait so a session is not left locked behind us.
    drop(driver);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), agent).await;
    result
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
