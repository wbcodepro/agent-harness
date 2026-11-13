use std::sync::Arc;
use std::time::Duration;

use hx_app::{Options, SandboxMode};
use hx_eval::{Recording, ScriptedLlm, ScriptedResponse};
use hx_session::{Delta, StopReason, Usage};
use hx_tools::InProcess;
use serde_json::{Value, json};

struct Page {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Page {
    async fn post(&self, path: &str, body: Value) -> (u16, String) {
        let response = self
            .http
            .post(format!("{}/api{path}", self.base))
            .header("x-hx-token", &self.token)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .unwrap();
        (response.status().as_u16(), response.text().await.unwrap())
    }

    /// Opens the event stream and returns a reader of its JSON events.
    async fn events(&self, session: &str) -> Events {
        let response = self
            .http
            .get(format!(
                "{}/api/sessions/{session}/events?token={}",
                self.base, self.token
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        Events {
            response,
            buffer: String::new(),
        }
    }
}

struct Events {
    response: reqwest::Response,
    buffer: String,
}

impl Events {
    async fn next(&mut self) -> Value {
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let frame: String = self.buffer.drain(..end + 2).collect();
                if let Some(data) = frame.lines().find_map(|line| line.strip_prefix("data: ")) {
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

    async fn until(&mut self, what: &str, done: impl Fn(&Value) -> bool) -> Vec<Value> {
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

async fn start(dir: &std::path::Path, responses: Vec<ScriptedResponse>, delay_ms: u64) -> Page {
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let options = Options {
        sandbox: Some(SandboxMode::DangerFullAccess),
        max_steps: 10,
        llm: Some(Arc::new(
            ScriptedLlm::new(Recording { responses }).with_delay(Duration::from_millis(delay_ms)),
        )),
        runner: Some(Arc::new(InProcess)),
        ..Options::default()
    };
    let server = hx_web::start(dir.join("home"), work, options, 0)
        .await
        .unwrap();
    Page {
        http: reqwest::Client::builder().no_proxy().build().unwrap(),
        base: format!("http://{}", server.addr),
        token: server.token.clone(),
    }
}

fn write_call(path: &str) -> ScriptedResponse {
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

#[tokio::test]
async fn a_page_runs_a_turn_answers_an_approval_and_can_reload() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(
        dir.path(),
        vec![write_call("a.txt"), ScriptedResponse::text("Written.")],
        0,
    )
    .await;

    let (status, body) = page.post("/sessions", json!({})).await;
    assert_eq!(status, 200, "{body}");
    let session = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut events = page.events(&session).await;

    assert_eq!(
        page.post(
            &format!("/sessions/{session}/prompt"),
            json!({"text": "write a.txt"})
        )
        .await
        .0,
        200
    );
    let before = events
        .until("the approval question", |e| e["kind"] == "approval")
        .await;
    let question = before.last().unwrap();
    assert!(
        question["question"]
            .as_str()
            .unwrap()
            .starts_with("Allow write")
    );
    assert!(
        !dir.path().join("work/a.txt").exists(),
        "nothing runs before the answer"
    );
    // A second prompt while the turn runs is refused.
    assert_eq!(
        page.post(
            &format!("/sessions/{session}/prompt"),
            json!({"text": "again"})
        )
        .await
        .0,
        409
    );

    let answer = json!({"id": question["id"], "allow": true});
    assert_eq!(
        page.post(&format!("/sessions/{session}/approval"), answer.clone())
            .await
            .0,
        200
    );
    assert_eq!(
        page.post(&format!("/sessions/{session}/approval"), answer)
            .await
            .0,
        400
    );
    let rest = events
        .until("the end of the turn", |e| {
            e["kind"] == "turn" && e["state"] == "ended"
        })
        .await;
    assert_eq!(rest.last().unwrap()["error"], Value::Null);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("work/a.txt")).unwrap(),
        "from the web"
    );
    let streamed: Vec<&str> = before
        .iter()
        .chain(&rest)
        .filter_map(|e| e["record"]["type"].as_str())
        .collect();
    assert!(streamed.contains(&"tool/call") && streamed.contains(&"tool/result"));
    assert_eq!(streamed.last(), Some(&"turn/end"));

    // A reloaded page gets the whole conversation again, in order.
    let mut reloaded = page.events(&session).await;
    let replayed = reloaded
        .until("the replayed end of turn", |e| {
            e["record"]["type"] == "turn/end"
        })
        .await;
    let seqs: Vec<u64> = replayed
        .iter()
        .filter_map(|e| e["record"]["seq"].as_u64())
        .collect();
    assert_eq!(seqs, (0..seqs.len() as u64).collect::<Vec<_>>());
}

#[tokio::test]
async fn the_api_needs_the_token_and_a_turn_can_be_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let slow = ScriptedResponse {
        deltas: (0..2000)
            .map(|n| Delta::Text {
                text: format!("w{n} "),
            })
            .collect(),
        stop: StopReason::EndTurn,
        usage: Usage::default(),
    };
    let page = start(dir.path(), vec![slow], 5).await;

    // The page itself is public; the API is not.
    let home = page
        .http
        .get(format!("{}/", page.base))
        .send()
        .await
        .unwrap();
    assert!(home.text().await.unwrap().contains("<title>hx</title>"));
    for request in [
        page.http.get(format!("{}/api/sessions", page.base)),
        page.http
            .get(format!("{}/api/sessions", page.base))
            .header("x-hx-token", "wrong"),
        // Right token, but addressed to another host name (DNS rebinding).
        page.http
            .get(format!("{}/api/sessions", page.base))
            .header("x-hx-token", &page.token)
            .header("host", "evil.example:80"),
    ] {
        assert_eq!(request.send().await.unwrap().status(), 403);
    }

    let (_, body) = page.post("/sessions", json!({})).await;
    let session = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut events = page.events(&session).await;
    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "talk"}),
    )
    .await;
    events
        .until("some streamed text", |e| {
            e["record"]["type"] == "assistant/delta"
        })
        .await;
    assert_eq!(
        page.post(&format!("/sessions/{session}/cancel"), json!({}))
            .await
            .0,
        200
    );
    let rest = events
        .until("the turn to end", |e| {
            e["kind"] == "turn" && e["state"] == "ended"
        })
        .await;
    assert!(
        rest.iter()
            .any(|e| e["record"]["data"]["reason"] == "cancelled")
    );

    assert_eq!(page.post("/sessions/..%2Fx/cancel", json!({})).await.0, 400);
}
