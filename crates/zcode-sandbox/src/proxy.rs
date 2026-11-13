use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Hosts the sandbox may reach. A pattern is a host name, or `*.suffix` for
/// every subdomain of `suffix`; either may end in `:port`. Without a port,
/// only 80 and 443 are allowed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Allowlist {
    patterns: Vec<String>,
}

impl Allowlist {
    pub fn new(patterns: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            patterns: patterns
                .into_iter()
                .map(|pattern| pattern.into().to_ascii_lowercase())
                .collect(),
        }
    }

    pub fn allows(&self, host: &str, port: u16) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        self.patterns.iter().any(|pattern| {
            let (pattern_host, pattern_port) = match pattern.rsplit_once(':') {
                Some((h, p)) if p.parse::<u16>().is_ok() => (h, p.parse::<u16>().ok()),
                _ => (pattern.as_str(), None),
            };
            let port_ok = match pattern_port {
                Some(allowed) => allowed == port,
                None => port == 80 || port == 443,
            };
            let host_ok = match pattern_host.strip_prefix("*.") {
                Some(suffix) => host
                    .strip_suffix(suffix)
                    .is_some_and(|rest| rest.ends_with('.') && rest.len() > 1),
                None => host == pattern_host,
            };
            port_ok && host_ok
        })
    }
}

/// One connection attempt the proxy saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub host: String,
    pub port: u16,
    pub allowed: bool,
}

/// A forward proxy on localhost that only connects to allowlisted hosts.
/// Confined tools reach the network through it or not at all. Stops when
/// dropped.
pub struct EgressProxy {
    port: u16,
    decisions: Arc<Mutex<Vec<Decision>>>,
    task: tokio::task::JoinHandle<()>,
}

impl EgressProxy {
    pub async fn start(allowlist: Allowlist) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let decisions = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&decisions);
        let allowlist = Arc::new(allowlist);
        let task = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let allowlist = Arc::clone(&allowlist);
                let log = Arc::clone(&log);
                tokio::spawn(async move {
                    let _ = serve(client, &allowlist, &log).await;
                });
            }
        });
        Ok(Self {
            port,
            decisions,
            task,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Every connection attempt so far, in order.
    pub fn decisions(&self) -> Vec<Decision> {
        self.decisions.lock().expect("proxy log poisoned").clone()
    }
}

impl Drop for EgressProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(
    mut client: TcpStream,
    allowlist: &Allowlist,
    log: &Mutex<Vec<Decision>>,
) -> std::io::Result<()> {
    // Read the request head.
    let mut buffer = Vec::new();
    let head_end = loop {
        let mut chunk = [0u8; 4096];
        let n = client.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..n]);
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
        if buffer.len() > 64 * 1024 {
            return refuse(&mut client, 431, "request head too large").await;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return refuse(&mut client, 400, "malformed request").await;
    };

    let is_connect = method.eq_ignore_ascii_case("CONNECT");
    let (authority, path) = if is_connect {
        (target, "")
    } else if let Some(rest) = target.strip_prefix("http://") {
        match rest.find('/') {
            Some(slash) => (&rest[..slash], &rest[slash..]),
            None => (rest, "/"),
        }
    } else {
        return refuse(
            &mut client,
            400,
            "this is a proxy; send an absolute http:// URL or CONNECT",
        )
        .await;
    };
    let default_port = if is_connect { 443 } else { 80 };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => match port.parse::<u16>() {
            Ok(port) => (host, port),
            Err(_) => return refuse(&mut client, 400, "invalid port").await,
        },
        _ => (authority, default_port),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');

    let allowed = allowlist.allows(host, port);
    log.lock().expect("proxy log poisoned").push(Decision {
        host: host.to_string(),
        port,
        allowed,
    });
    if !allowed {
        return refuse(
            &mut client,
            403,
            &format!("zcode sandbox: {host}:{port} is not on the network allowlist"),
        )
        .await;
    }

    let mut upstream = match TcpStream::connect((host, port)).await {
        Ok(upstream) => upstream,
        Err(e) => {
            return refuse(
                &mut client,
                502,
                &format!("cannot connect to {host}:{port}: {e}"),
            )
            .await;
        }
    };
    if is_connect {
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        upstream.write_all(&buffer[head_end..]).await?;
    } else {
        // Re-issue the request in origin form, without hop-by-hop proxy
        // headers, and close after one exchange.
        let mut forwarded = format!("{method} {path} {version}\r\n");
        for line in lines {
            let lower = line.to_ascii_lowercase();
            if line.is_empty() || lower.starts_with("proxy-") || lower.starts_with("connection:") {
                continue;
            }
            forwarded.push_str(line);
            forwarded.push_str("\r\n");
        }
        forwarded.push_str("Connection: close\r\n\r\n");
        upstream.write_all(forwarded.as_bytes()).await?;
        upstream.write_all(&buffer[head_end..]).await?;
    }
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

async fn refuse(client: &mut TcpStream, status: u16, reason: &str) -> std::io::Result<()> {
    let body = format!("{reason}\n");
    let response = format!(
        "HTTP/1.1 {status} Refused\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    client.write_all(response.as_bytes()).await?;
    client.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_matches_hosts_subdomains_and_ports() {
        let list = Allowlist::new(["github.com", "*.crates.io", "localhost:8080"]);
        assert!(list.allows("github.com", 443));
        assert!(list.allows("GitHub.com.", 80));
        assert!(!list.allows("github.com", 22));
        assert!(!list.allows("evilgithub.com", 443));
        assert!(!list.allows("github.com.evil.io", 443));
        assert!(list.allows("static.crates.io", 443));
        assert!(!list.allows("crates.io", 443));
        assert!(!list.allows("xcrates.io", 443));
        assert!(list.allows("localhost", 8080));
        assert!(!list.allows("localhost", 80));
        assert!(!Allowlist::default().allows("github.com", 443));
    }

    /// A tiny origin server answering every request with `body`.
    async fn origin(body: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut seen = Vec::new();
                let mut chunk = [0u8; 1024];
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    seen.extend_from_slice(&chunk[..n]);
                }
                let head = String::from_utf8_lossy(&seen);
                let line = head.lines().next().unwrap_or("").to_string();
                let reply = format!("{body} [{line}]");
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        port
    }

    async fn exchange(proxy_port: u16, request: String) -> String {
        let mut socket = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        socket.read_to_end(&mut response).await.unwrap();
        String::from_utf8_lossy(&response).into_owned()
    }

    #[tokio::test]
    async fn forwards_allowed_requests_and_refuses_the_rest() {
        let origin_port = origin("hello").await;
        let proxy = EgressProxy::start(Allowlist::new([format!("localhost:{origin_port}")]))
            .await
            .unwrap();

        let plain = exchange(
            proxy.port(),
            format!(
                "GET http://localhost:{origin_port}/a?b=1 HTTP/1.1\r\nHost: localhost\r\nProxy-Connection: keep-alive\r\n\r\n"
            ),
        )
        .await;
        assert!(plain.ends_with("hello [GET /a?b=1 HTTP/1.1]"), "{plain}");

        let denied = exchange(
            proxy.port(),
            "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n".into(),
        )
        .await;
        assert!(denied.starts_with("HTTP/1.1 403"), "{denied}");
        assert!(denied.contains("not on the network allowlist"));

        // CONNECT tunnels raw bytes once established.
        let mut tunnel = TcpStream::connect(("127.0.0.1", proxy.port()))
            .await
            .unwrap();
        tunnel
            .write_all(format!("CONNECT localhost:{origin_port} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut established = [0u8; 39];
        tunnel.read_exact(&mut established).await.unwrap();
        assert!(established.starts_with(b"HTTP/1.1 200"));
        tunnel.write_all(b"GET /t HTTP/1.1\r\n\r\n").await.unwrap();
        let mut through = Vec::new();
        tunnel.read_to_end(&mut through).await.unwrap();
        assert!(String::from_utf8_lossy(&through).ends_with("hello [GET /t HTTP/1.1]"));

        let blocked = exchange(proxy.port(), "CONNECT 10.0.0.1:443 HTTP/1.1\r\n\r\n".into()).await;
        assert!(blocked.starts_with("HTTP/1.1 403"));

        let seen: Vec<(String, bool)> = proxy
            .decisions()
            .into_iter()
            .map(|d| (d.host, d.allowed))
            .collect();
        assert_eq!(
            seen,
            [
                ("localhost".to_string(), true),
                ("example.com".to_string(), false),
                ("localhost".to_string(), true),
                ("10.0.0.1".to_string(), false),
            ]
        );
    }
}
