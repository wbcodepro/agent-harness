use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use hame_app::{Options, SandboxMode};
use hame_eval::{Recording, ScriptedLlm, ScriptedResponse};
use hame_session::{Delta, StopReason, Usage};
use hame_tools::InProcess;
use hame_tui::{Msg, drive};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use serde_json::json;
use tokio::sync::mpsc;

struct Ui {
    keys: mpsc::UnboundedSender<Msg>,
    done: tokio::task::JoinHandle<Result<(), String>>,
}

/// Starts the interface on an in-memory terminal, with a scripted model and
/// unconfined in-process tools so approval is what gates them.
fn start(home: &Path, work: &Path, responses: Vec<ScriptedResponse>, delay_ms: u64) -> Ui {
    let options = Options {
        sandbox: Some(SandboxMode::DangerFullAccess),
        max_steps: 10,
        llm: Some(Arc::new(
            ScriptedLlm::new(Recording { responses }).with_delay(Duration::from_millis(delay_ms)),
        )),
        runner: Some(Arc::new(InProcess)),
        ..Options::default()
    };
    let (keys, inbox) = mpsc::unbounded_channel();
    let (home, work, sender) = (home.to_path_buf(), work.to_path_buf(), keys.clone());
    let done = tokio::spawn(async move {
        let mut terminal = Terminal::new(TestBackend::new(70, 20)).unwrap();
        drive(&mut terminal, home, work, options, None, sender, inbox).await
    });
    Ui { keys, done }
}

impl Ui {
    fn key(&self, code: KeyCode) {
        self.keys
            .send(Msg::Terminal(Event::Key(KeyEvent::new(
                code,
                KeyModifiers::NONE,
            ))))
            .unwrap();
    }

    fn type_line(&self, text: &str) {
        for c in text.chars() {
            self.key(KeyCode::Char(c));
        }
        self.key(KeyCode::Enter);
    }
}

async fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..2000 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

fn write_call(path: &str, content: &str) -> ScriptedResponse {
    ScriptedResponse {
        deltas: vec![
            Delta::ToolUseStart {
                id: "c1".into(),
                name: "write".into(),
            },
            Delta::ToolInput {
                json: json!({"path": path, "content": content}).to_string(),
            },
        ],
        stop: StopReason::ToolUse,
        usage: Usage::default(),
    }
}

#[tokio::test]
async fn typing_a_request_runs_a_turn_with_approval_and_rewind_undoes_it() {
    let dir = tempfile::tempdir().unwrap();
    let (home, work) = (dir.path().join("home"), dir.path().join("work"));
    std::fs::create_dir_all(&work).unwrap();
    let work = work.canonicalize().unwrap();
    let ui = start(
        &home,
        &work,
        vec![
            write_call("a.txt", "hello"),
            ScriptedResponse::text("Wrote it."),
        ],
        0,
    );

    ui.type_line("make a.txt");
    // Unconfined tools ask first. Wait until the call is logged (the
    // question follows at once), and check nothing ran before the answer.
    let store = hame_app::session_store(&home);
    wait_for("the tool call to be logged", || {
        store.list().unwrap().first().is_some_and(|header| {
            store
                .read(&header.id)
                .unwrap()
                .iter()
                .any(|record| matches!(record.event, hame_session::Event::ToolCall { .. }))
        })
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!work.join("a.txt").exists());
    ui.key(KeyCode::Char('y'));
    wait_for("the approved write", || work.join("a.txt").exists()).await;

    // The session recorded a completed turn.
    wait_for("the turn to finish", || {
        store.list().unwrap().first().is_some_and(|header| {
            store.read(&header.id).unwrap().iter().any(|record| {
                matches!(
                    record.event,
                    hame_session::Event::TurnEnd {
                        reason: hame_session::TurnEndReason::Completed,
                        ..
                    }
                )
            })
        })
    })
    .await;

    ui.type_line("/rewind");
    wait_for("the rewind to remove the file", || {
        !work.join("a.txt").exists()
    })
    .await;
    wait_for("the rewind to continue in a fork", || {
        store.list().unwrap().len() == 2
    })
    .await;

    ui.type_line("/quit");
    assert_eq!(ui.done.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn escape_cancels_a_running_turn_and_plan_mode_removes_write_tools() {
    let dir = tempfile::tempdir().unwrap();
    let (home, work) = (dir.path().join("home"), dir.path().join("work"));
    std::fs::create_dir_all(&work).unwrap();
    let slow = ScriptedResponse {
        deltas: (0..2000)
            .map(|n| Delta::Text {
                text: format!("w{n} "),
            })
            .collect(),
        stop: StopReason::EndTurn,
        usage: Usage::default(),
    };
    let ui = start(
        &home,
        &work,
        vec![slow, ScriptedResponse::text("A plan.")],
        5,
    );
    let store = hame_app::session_store(&home);
    let events = || {
        store
            .list()
            .unwrap()
            .first()
            .map(|header| store.read(&header.id).unwrap())
            .unwrap_or_default()
    };

    ui.type_line("talk");
    wait_for("streaming to start", || {
        events()
            .iter()
            .any(|r| matches!(r.event, hame_session::Event::AssistantDelta { .. }))
    })
    .await;
    ui.key(KeyCode::Esc);
    wait_for("the cancelled turn to close", || {
        events().iter().any(|r| {
            matches!(
                r.event,
                hame_session::Event::TurnEnd {
                    reason: hame_session::TurnEndReason::Cancelled,
                    ..
                }
            )
        })
    })
    .await;

    ui.type_line("/plan");
    ui.type_line("plan it");
    wait_for("the planning turn", || {
        events()
            .iter()
            .filter(|r| matches!(r.event, hame_session::Event::TurnEnd { .. }))
            .count()
            == 2
    })
    .await;
    // In plan mode the model was offered no tool that writes files.
    let declared: Vec<Vec<String>> = events()
        .iter()
        .filter_map(|r| match &r.event {
            hame_session::Event::ToolsDeclared { tools } => {
                Some(tools.iter().map(|t| t.name.clone()).collect())
            }
            _ => None,
        })
        .collect();
    assert_eq!(declared.len(), 2);
    assert!(declared[0].contains(&"write".to_string()));
    assert!(
        !declared[1].contains(&"write".to_string()) && !declared[1].contains(&"edit".to_string())
    );
    assert!(declared[1].contains(&"read".to_string()));

    ui.keys
        .send(Msg::Terminal(Event::Key(KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
        ))))
        .unwrap();
    assert_eq!(ui.done.await.unwrap(), Ok(()));
}
