//! Per-operation latency of the `browser` tool against the test fixture,
//! for the backend comparison in `docs/release/EVIDENCE-browser.md`.
//!
//! cargo run --release -p hame-browser --example latency [-- REPS]

use std::io::BufRead as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use hame_browser::{BrowserConfig, BrowserTool, requirements};
use hame_session::contract::{Authorization, Capability, Channel, Grant};
use hame_tools::{Tool, ToolContext};
use serde_json::{Value, json};

fn find_ref(text: &str, head: &str) -> String {
    let line = text
        .lines()
        .find(|l| l.starts_with(head))
        .unwrap_or_else(|| panic!("no {head} in {text}"));
    line.split('[')
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap()
        .to_string()
}

fn stats(name: &str, mut ms: Vec<f64>) -> Value {
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = ms[ms.len() / 2];
    let p95 = ms[((ms.len() as f64 * 0.95).ceil() as usize).min(ms.len()) - 1];
    println!(
        "{name:10} n={:2} median={median:7.1} ms  p95={p95:7.1} ms  raw={ms:.1?}",
        ms.len()
    );
    json!({"op": name, "n": ms.len(), "median_ms": median, "p95_ms": p95, "raw_ms": ms})
}

#[tokio::main]
async fn main() {
    let reps: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(12);
    let dir = tempfile::tempdir().unwrap();
    let auth = Authorization {
        grants: vec![Grant {
            channel: Channel::Browser,
            app: None,
            origin: Some("http://127.0.0.1:*".into()),
            profile: None,
            capabilities: vec![
                Capability::Observe,
                Capability::Interact,
                Capability::Commit,
                Capability::FileWrite,
            ],
            folders: vec![],
        }],
    };
    let config = BrowserConfig::new(dir.path().join("profiles"), auth);
    requirements(&config).expect("browser requirements");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture/app.py");
    let mut fixture = Command::new(hame_kernel::python_program().unwrap())
        .arg("-I")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(fixture.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let base = format!(
        "http://127.0.0.1:{}",
        line.trim().trim_start_matches("PORT ")
    );
    let tool = BrowserTool::new(config);
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let ctx = ToolContext::new(&work);
    let call = |input: Value| {
        let tool = &tool;
        let ctx = &ctx;
        async move {
            let t = Instant::now();
            let out = tool.call(&input, ctx).await;
            assert!(!out.is_error, "{input}: {}", out.text);
            (t.elapsed().as_secs_f64() * 1000.0, out.text)
        }
    };

    let started = Instant::now();
    let (_, _) = call(json!({"action": "open", "url": format!("{base}/form")})).await;
    println!(
        "first open (helper + Chrome start): {:.0} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );

    let (mut nav, mut observe, mut fill, mut click, mut download) =
        (vec![], vec![], vec![], vec![], vec![]);
    for i in 0..reps {
        let (ms, _) = call(json!({"action": "navigate", "url": format!("{base}/form")})).await;
        nav.push(ms);
        let (ms, text) = call(json!({"action": "observe"})).await;
        observe.push(ms);
        let name = find_ref(&text, "- textbox \"Name\"");
        let express = find_ref(&text, "- checkbox \"Express delivery\"");
        let (ms, _) =
            call(json!({"action": "fill", "ref": name, "value": format!("Ana {i}")})).await;
        fill.push(ms);
        let (ms, _) = call(json!({"action": "click", "ref": express})).await;
        click.push(ms);
    }
    let (_, text) = call(json!({"action": "navigate", "url": format!("{base}/export")})).await;
    let from = find_ref(&text, "- textbox \"From\"");
    let to = find_ref(&text, "- textbox \"To\"");
    let button = find_ref(&text, "- button \"Export CSV\"");
    call(json!({"action": "fill", "ref": from, "value": "2026-01-10"})).await;
    call(json!({"action": "fill", "ref": to, "value": "2026-01-20"})).await;
    for i in 0..reps.min(10) {
        let (ms, _) =
            call(json!({"action": "download", "ref": button, "path": format!("dl/{i}.csv")})).await;
        download.push(ms);
    }
    let report = json!([
        stats("navigate+observe", nav),
        stats("observe", observe),
        stats("fill", fill),
        stats("click", click),
        stats("download", download),
    ]);
    std::fs::write(dir.path().join("latency.json"), report.to_string()).unwrap();
    println!("{report}");
    tool.shutdown().await;
    let _ = fixture.kill();
    let _ = fixture.wait();
}
