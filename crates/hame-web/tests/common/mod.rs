//! Shared helpers for the API tests: a server with a scripted model, and
//! a client that reads its event stream.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use hame_app::{Options, SandboxMode};
use hame_eval::{Recording, ScriptedLlm, ScriptedResponse};
use hame_session::{Delta, StopReason, Usage};
use hame_tools::InProcess;
use serde_json::{Value, json};

pub struct Page {
    pub http: reqwest::Client,
    pub base: String,
    pub token: String,
    pub dir: std::path::PathBuf,
}

impl Page {
    pub async fn post(&self, path: &str, body: Value) -> (u16, String) {
        let response = self
            .http
            .post(format!("{}/api{path}", self.base))
            .header("x-hame-token", &self.token)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .unwrap();
        (response.status().as_u16(), response.text().await.unwrap())
    }

    /// Any API call; the body is parsed as JSON when it is JSON.
    pub async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut request = self
            .http
            .request(method, format!("{}/api{path}", self.base))
            .header("x-hame-token", &self.token);
        if let Some(body) = body {
            request = request
                .header("content-type", "application/json")
                .body(body.to_string());
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    pub async fn get(&self, path: &str) -> (u16, Value) {
        self.call("GET", path, None).await
    }

    /// Creates a session and returns its id.
    pub async fn session(&self, body: Value) -> String {
        let (status, created) = self.call("POST", "/sessions", Some(body)).await;
        assert_eq!(status, 200, "{created}");
        created["id"].as_str().unwrap().to_string()
    }

    /// Opens the event stream and returns a reader of its JSON events.
    pub async fn events(&self, session: &str) -> Events {
        self.events_with(session, "", None).await
    }

    /// Opens the event stream with extra query text (such as `&after=3`)
    /// and, when given, a `Last-Event-ID` header.
    pub async fn events_with(&self, session: &str, query: &str, last_id: Option<u64>) -> Events {
        let mut request = self.http.get(format!(
            "{}/api/sessions/{session}/events?token={}{query}",
            self.base, self.token
        ));
        if let Some(id) = last_id {
            request = request.header("last-event-id", id.to_string());
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 200);
        Events {
            response,
            buffer: String::new(),
            ids: Vec::new(),
        }
    }
}

pub struct Events {
    response: reqwest::Response,
    buffer: String,
    /// The SSE id of each event read, `None` for events without one.
    pub ids: Vec<Option<u64>>,
}

impl Events {
    pub async fn next(&mut self) -> Value {
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let frame: String = self.buffer.drain(..end + 2).collect();
                if let Some(data) = frame.lines().find_map(|line| line.strip_prefix("data: ")) {
                    let id = frame
                        .lines()
                        .find_map(|line| line.strip_prefix("id: "))
                        .and_then(|id| id.parse().ok());
                    self.ids.push(id);
                    return serde_json::from_str(data).unwrap();
                }
                continue;
            }
            let chunk = tokio::time::timeout(Duration::from_secs(20), self.response.chunk())
                .await
                .expect("an event arrives in time")
                .unwrap()
                .expect("the stream stays open");
            self.buffer.push_str(&String::from_utf8_lossy(&chunk));
        }
    }

    pub async fn until(&mut self, what: &str, done: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut seen = Vec::new();
        for _ in 0..5000 {
            let event = self.next().await;
            let stop = done(&event);
            seen.push(event);
            if stop {
                return seen;
            }
        }
        panic!("never saw {what}");
    }
}

pub async fn start(dir: &std::path::Path, responses: Vec<ScriptedResponse>, delay_ms: u64) -> Page {
    start_with(dir, responses, delay_ms, |_| {}).await
}

/// Like `start`, with a chance to change the server's options.
pub async fn start_with(
    dir: &std::path::Path,
    responses: Vec<ScriptedResponse>,
    delay_ms: u64,
    change: impl FnOnce(&mut Options),
) -> Page {
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let mut options = Options {
        sandbox: Some(SandboxMode::DangerFullAccess),
        max_steps: 10,
        llm: Some(Arc::new(
            ScriptedLlm::new(Recording { responses }).with_delay(Duration::from_millis(delay_ms)),
        )),
        runner: Some(Arc::new(InProcess)),
        ..Options::default()
    };
    change(&mut options);
    let server = hame_web::start(dir.join("home"), work, options, 0)
        .await
        .unwrap();
    Page {
        http: reqwest::Client::builder().no_proxy().build().unwrap(),
        base: format!("http://{}", server.addr),
        token: server.token.clone(),
        dir: dir.to_path_buf(),
    }
}

pub fn write_call(path: &str) -> ScriptedResponse {
    ScriptedResponse {
        deltas: vec![
            Delta::ToolUseStart {
                id: "c1".into(),
                name: "write".into(),
            },
            Delta::ToolInput {
                json: json!({"path": path, "content": "from the web"}).to_string(),
            },
        ],
        stop: StopReason::ToolUse,
        usage: Usage::default(),
    }
}

/// A response that calls `name` with `input`.
pub fn tool_call(id: &str, name: &str, input: Value) -> ScriptedResponse {
    ScriptedResponse {
        deltas: vec![
            Delta::ToolUseStart {
                id: id.into(),
                name: name.into(),
            },
            Delta::ToolInput {
                json: input.to_string(),
            },
        ],
        stop: StopReason::ToolUse,
        usage: Usage::default(),
    }
}
