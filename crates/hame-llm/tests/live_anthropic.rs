//! Live tests against the real Anthropic Messages API.
//!
//! Ignored by default; they spend money. Run them one at a time with a key
//! in the environment:
//!
//! ```sh
//! HAME_LIVE_ANTHROPIC=1 CLAUDE_API_KEY=... \
//!     cargo test -p hame-llm --test live_anthropic -- --ignored --nocapture --test-threads=1
//! ```
//!
//! `HAME_LIVE_MODEL` picks the model (default `claude-haiku-5-5`, the
//! cheapest vision model). Each test prints one `live:` line with the
//! model, tokens, bytes and latency of its call.

#![allow(clippy::await_holding_lock)]

use std::sync::{Arc, Mutex};
use std::time::Instant;

use hame_llm::{Anthropic, Completion, Llm, Request};
use hame_session::{
    ArtifactRef, ArtifactSource, ArtifactStore, Block, Delta, Message, Role, StopReason, ToolSpec,
};
use serde_json::json;

/// One request at a time, even if the harness runs tests in parallel.
/// Each test has its own runtime on its own thread, so holding this lock
/// across an await blocks only other tests, which is the point.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn live() -> Option<Anthropic> {
    if std::env::var("HAME_LIVE_ANTHROPIC").as_deref() != Ok("1") {
        eprintln!("skipped: set HAME_LIVE_ANTHROPIC=1 to run live tests");
        return None;
    }
    let key = std::env::var("CLAUDE_API_KEY")
        .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
        .expect("CLAUDE_API_KEY or ANTHROPIC_API_KEY");
    let model = std::env::var("HAME_LIVE_MODEL").unwrap_or_else(|_| "claude-haiku-5-5".into());
    Some(Anthropic::new("https://api.anthropic.com", key, model).with_effort(Some("low".into())))
}

/// A PNG of `size` x `size` pixels, the left half `left` and the right half
/// `right`, stored uncompressed.
fn png(size: u32, left: [u8; 3], right: [u8; 3]) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    let row: Vec<u8> = std::iter::once(0)
        .chain((0..size).flat_map(|x| if x < size / 2 { left } else { right }))
        .collect();
    let raw = row.repeat(size as usize);
    let mut zlib = vec![0x78, 0x01, 1];
    let len = raw.len() as u16;
    zlib.extend_from_slice(&len.to_le_bytes());
    zlib.extend_from_slice(&(!len).to_le_bytes());
    zlib.extend_from_slice(&raw);
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = size.to_be_bytes().to_vec();
    header.extend_from_slice(&size.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn store() -> (tempfile::TempDir, ArtifactStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(dir.path().join("artifacts"));
    (dir, store)
}

fn source(store: &ArtifactStore) -> Option<ArtifactSource> {
    Some(ArtifactSource(Arc::new(store.clone())))
}

/// Runs one call and prints its measurements. Returns the answer's text.
async fn call(name: &str, llm: &Anthropic, request: &Request) -> (String, Completion) {
    let mut text = String::new();
    let mut sink = |delta: Delta| {
        if let Delta::Text { text: part } = delta {
            text.push_str(&part);
        }
        Ok(())
    };
    let started = Instant::now();
    let result = llm.stream(request, &mut sink).await;
    let latency = started.elapsed().as_millis();
    let completion = match result {
        Ok(completion) => completion,
        Err(e) => panic!("{name}: the call failed: {e}"),
    };
    let (usage, meta) = (&completion.usage, &completion.meta);
    println!(
        "live: test={name} model={} input={} cache_read={} cache_write={} output={} \
         request_bytes={} response_bytes={} images={} image_bytes={} ttft_ms={} latency_ms={latency} \
         stop={:?} raw_usage={}",
        meta.model.as_deref().unwrap_or("?"),
        usage.input_tokens,
        usage.cache_read_tokens,
        usage.cache_write_tokens,
        usage.output_tokens,
        meta.request_bytes.unwrap_or(0),
        meta.response_bytes.unwrap_or(0),
        meta.images.unwrap_or(0),
        meta.image_bytes.unwrap_or(0),
        meta.ttft_ms.map_or("-".into(), |ms| ms.to_string()),
        completion.stop,
        meta.raw_usage.clone().unwrap_or_default(),
    );
    println!("live: test={name} answer={text:?}");
    (text, completion)
}

fn image(artifact: &ArtifactRef) -> Block {
    Block::Image {
        artifact: artifact.clone(),
    }
}

#[tokio::test]
#[ignore = "calls the real Anthropic API; set HAME_LIVE_ANTHROPIC=1"]
async fn live_an_image_in_a_tool_result_is_seen() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(llm) = live() else { return };
    let (_dir, store) = store();
    let red = store
        .put(&png(64, [220, 20, 20], [220, 20, 20]), "image/png")
        .unwrap();
    let request = Request {
        system: Some("You are a terse assistant.".into()),
        tools: vec![ToolSpec {
            name: "screenshot".into(),
            description: "Take a screenshot of the screen.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        }],
        messages: vec![
            Message {
                role: Role::User,
                content: vec![Block::text("Take a screenshot.")],
            },
            Message {
                role: Role::Assistant,
                content: vec![Block::ToolUse {
                    id: "toolu_live_1".into(),
                    name: "screenshot".into(),
                    input: json!({}),
                }],
            },
            Message {
                role: Role::Tool,
                content: vec![Block::ToolResult {
                    call_id: "toolu_live_1".into(),
                    content: vec![Block::text("Screenshot taken."), image(&red)],
                    is_error: false,
                }],
            },
            Message {
                role: Role::User,
                content: vec![Block::text(
                    "What single colour fills the screenshot? Answer with one word.",
                )],
            },
        ],
        max_tokens: Some(1024),
        cache_key: None,
        artifacts: source(&store),
    };
    let (text, completion) = call("tool_result_image", &llm, &request).await;
    assert_eq!(completion.meta.images, Some(1));
    assert_eq!(completion.meta.image_bytes, Some(red.bytes));
    assert!(text.to_lowercase().contains("red"), "answer: {text:?}");
}

#[tokio::test]
#[ignore = "calls the real Anthropic API; set HAME_LIVE_ANTHROPIC=1"]
async fn live_an_image_in_a_user_message_is_seen() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(llm) = live() else { return };
    let (_dir, store) = store();
    let halves = store
        .put(&png(64, [20, 40, 230], [20, 200, 40]), "image/png")
        .unwrap();
    let request = Request {
        messages: vec![Message {
            role: Role::User,
            content: vec![
                image(&halves),
                Block::text(
                    "This image has two halves. Name the colour of the left half and of the \
                     right half, as `left: <colour>, right: <colour>`.",
                ),
            ],
        }],
        max_tokens: Some(1024),
        artifacts: source(&store),
        ..Request::default()
    };
    let (text, completion) = call("user_image", &llm, &request).await;
    assert_eq!(completion.meta.images, Some(1));
    let answer = text.to_lowercase();
    let left = answer.find("blue");
    let right = answer.find("green");
    assert!(
        matches!((left, right), (Some(l), Some(r)) if l < r),
        "answer: {text:?}"
    );
}

#[tokio::test]
#[ignore = "calls the real Anthropic API; set HAME_LIVE_ANTHROPIC=1"]
async fn live_an_image_missing_from_the_store_becomes_a_note() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(llm) = live() else { return };
    let (_dir, store) = store();
    let mut gone = store
        .put(&png(16, [1, 2, 3], [1, 2, 3]), "image/png")
        .unwrap();
    // A reference whose bytes were never stored.
    gone.sha256 = "e3".repeat(32);
    let request = Request {
        messages: vec![Message {
            role: Role::User,
            content: vec![
                Block::text("I attached a picture."),
                image(&gone),
                Block::text(
                    "If you received the picture, describe it in five words. If instead you \
                     see a note saying it was not sent, reply with the reason the note gives.",
                ),
            ],
        }],
        max_tokens: Some(1024),
        artifacts: source(&store),
        ..Request::default()
    };
    let (text, completion) = call("missing_image", &llm, &request).await;
    assert_eq!(completion.meta.images, Some(0));
    assert_eq!(completion.stop, StopReason::EndTurn);
    assert!(text.to_lowercase().contains("missing"), "answer: {text:?}");
}

/// Two identical calls with a long system prompt: the first writes the
/// prompt cache, the second reads it. Shows the normalized usage keeps
/// uncached input, cache reads and cache writes apart.
#[tokio::test]
#[ignore = "calls the real Anthropic API; set HAME_LIVE_ANTHROPIC=1"]
async fn live_cache_reads_and_writes_are_reported_apart() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(llm) = live() else { return };
    let rules: String = (1..=400)
        .map(|n| format!("Rule {n}: answer politely and keep replies under ten words.\n"))
        .collect();
    let request = Request {
        system: Some(format!("You follow these rules.\n{rules}")),
        messages: vec![Message {
            role: Role::User,
            content: vec![Block::text("Say hello.")],
        }],
        max_tokens: Some(512),
        ..Request::default()
    };
    let (_, first) = call("cache_first", &llm, &request).await;
    let (_, second) = call("cache_second", &llm, &request).await;
    let written = first.usage.cache_write_tokens + first.usage.cache_read_tokens;
    assert!(written > 0, "nothing cached: {:?}", first.usage);
    assert!(
        second.usage.cache_read_tokens > 0,
        "no cache read: {:?}",
        second.usage
    );
    // The same prompt is counted once, whichever bucket it lands in.
    let total = |c: &Completion| {
        c.usage.input_tokens + c.usage.cache_read_tokens + c.usage.cache_write_tokens
    };
    assert_eq!(total(&first), total(&second));
}
