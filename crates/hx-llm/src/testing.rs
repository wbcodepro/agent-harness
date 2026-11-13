//! A canned HTTP server for adapter tests.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serves one canned HTTP response per connection, in order, and records
/// each request body.
pub(crate) async fn serve(responses: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bodies);
    tokio::spawn(async move {
        for (status, body) in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut raw = Vec::new();
            let request_body = loop {
                let mut chunk = [0u8; 4096];
                let n = socket.read(&mut chunk).await.unwrap();
                raw.extend_from_slice(&chunk[..n]);
                let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let head = String::from_utf8_lossy(&raw[..split]).to_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .map(|v| v.trim().parse().unwrap())
                    .unwrap_or(0);
                if raw.len() >= split + 4 + length {
                    break raw[split + 4..split + 4 + length].to_vec();
                }
            };
            seen.lock()
                .unwrap()
                .push(serde_json::from_slice(&request_body).unwrap());
            let head = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: text/event-stream\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            // Dribble the body out so SSE lines straddle read chunks.
            for piece in body.as_bytes().chunks(7) {
                socket.write_all(piece).await.unwrap();
                socket.flush().await.unwrap();
            }
            socket.shutdown().await.ok();
        }
    });
    (base_url, bodies)
}
