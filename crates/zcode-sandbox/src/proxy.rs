use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::intercept::{Authority, Substitution, applying, rewrite_head};

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
    /// The certificate tools have to trust, when connections are read.
    authority_pem: Option<String>,
}

/// What the proxy does beyond allowing and refusing.
struct Rewriting {
    substitutions: Vec<Substitution>,
    authority: Option<Authority>,
}

impl EgressProxy {
    pub async fn start(allowlist: Allowlist) -> std::io::Result<Self> {
        Self::start_with(allowlist, Vec::new(), Vec::new()).await
    }

    /// Like [`start`](Self::start), and also puts secrets in place of
    /// their placeholders on the way to the hosts named for them. Those
    /// hosts' TLS connections are ended here; `extra_roots` are
    /// authorities to trust for them besides the system's.
    pub async fn start_with(
        allowlist: Allowlist,
        substitutions: Vec<Substitution>,
        extra_roots: Vec<rustls::pki_types::CertificateDer<'static>>,
    ) -> std::io::Result<Self> {
        let authority = if substitutions.is_empty() {
            None
        } else {
            Some(Authority::new(extra_roots).map_err(std::io::Error::other)?)
        };
        let authority_pem = authority.as_ref().map(Authority::pem);
        let rewriting = Arc::new(Rewriting {
            substitutions,
            authority,
        });
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let decisions = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&decisions);
        let allowlist = Arc::new(allowlist);
        let task = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let allowlist = Arc::clone(&allowlist);
                let log = Arc::clone(&log);
                let rewriting = Arc::clone(&rewriting);
                tokio::spawn(async move {
                    let _ = serve(client, &allowlist, &log, &rewriting).await;
                });
            }
        });
        Ok(Self {
            port,
            decisions,
            task,
            authority_pem,
        })
    }

    /// The certificate of the authority that signs for intercepted hosts,
    /// in PEM. `None` when nothing is intercepted.
    pub fn authority_pem(&self) -> Option<&str> {
        self.authority_pem.as_deref()
    }

    /// Everything a tool should trust when connections are read: the
    /// system's authorities and this proxy's, as one PEM text. `None` when
    /// nothing is intercepted or the system gives no authorities.
    pub fn trust_bundle_pem(&self) -> Option<String> {
        let own = self.authority_pem.as_deref()?;
        Some(format!("{}{own}", crate::intercept::system_roots_pem()?))
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
    rewriting: &Rewriting,
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
    let substitutions = applying(&rewriting.substitutions, host, port);
    if is_connect
        && !substitutions.is_empty()
        && let Some(authority) = &rewriting.authority
    {
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        return intercept(client, upstream, host, authority, &substitutions).await;
    }
    if is_connect {
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        upstream.write_all(&buffer[head_end..]).await?;
    } else if !substitutions.is_empty() {
        let rest: Vec<&str> = lines.collect();
        let head = format!("{method} {path} {version}\r\n{}", rest.join("\r\n"));
        upstream
            .write_all(rewrite_head(&head, &substitutions).as_bytes())
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

/// Ends the tool's TLS connection here, answering as `host`, puts the
/// secrets into the head of its request, and passes it on over a TLS
/// connection of the proxy's own to the real host.
async fn intercept(
    client: TcpStream,
    upstream: TcpStream,
    host: &str,
    authority: &Authority,
    substitutions: &[&Substitution],
) -> std::io::Result<()> {
    let answering = authority
        .answering_as(host)
        .map_err(std::io::Error::other)?;
    let mut client = tokio_rustls::TlsAcceptor::from(answering)
        .accept(client)
        .await?;
    let name =
        rustls::pki_types::ServerName::try_from(host.to_string()).map_err(std::io::Error::other)?;
    let connector = tokio_rustls::TlsConnector::from(authority.upstream());
    let mut upstream = connector.connect(name, upstream).await?;

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
            return Ok(());
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]);
    upstream
        .write_all(rewrite_head(&head, substitutions).as_bytes())
        .await?;
    upstream.write_all(&buffer[head_end..]).await?;
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

    /// An HTTPS server for `localhost` with a private authority. It
    /// answers every request with the head it received. Returns its port
    /// and its authority's certificate.
    async fn tls_origin() -> (u16, rustls::pki_types::CertificateDer<'static>) {
        use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let issuer = CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap();
        let key = KeyPair::generate().unwrap();
        let leaf = CertificateParams::new(vec!["localhost".to_string()])
            .unwrap()
            .signed_by(&key, &issuer)
            .unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf.der().clone()],
            rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
        )
        .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(socket).await else {
                        return;
                    };
                    let mut seen = Vec::new();
                    let mut chunk = [0u8; 4096];
                    while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                        match tls.read(&mut chunk).await {
                            Ok(n) if n > 0 => seen.extend_from_slice(&chunk[..n]),
                            _ => return,
                        }
                    }
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        seen.len()
                    );
                    let _ = tls.write_all(response.as_bytes()).await;
                    let _ = tls.write_all(&seen).await;
                    let _ = tls.shutdown().await;
                });
            }
        });
        (port, issuer.der().clone())
    }

    /// What a tool does: asks the proxy for a tunnel, speaks TLS through
    /// it trusting only `trusted`, and sends one request. Returns the
    /// response, or why TLS could not be set up.
    async fn through_tunnel(
        proxy_port: u16,
        origin_port: u16,
        trusted: rustls::pki_types::CertificateDer<'static>,
        request: &str,
    ) -> Result<String, String> {
        let mut stream = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
        stream
            .write_all(format!("CONNECT localhost:{origin_port} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut established = [0u8; 39];
        stream.read_exact(&mut established).await.unwrap();
        assert!(established.starts_with(b"HTTP/1.1 200"));

        let mut roots = rustls::RootCertStore::empty();
        roots.add(trusted).unwrap();
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let mut tls = tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(name, stream)
            .await
            .map_err(|e| e.to_string())?;
        tls.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        let _ = tls.read_to_end(&mut response).await;
        Ok(String::from_utf8_lossy(&response).into_owned())
    }

    fn pem_to_der(pem: &str) -> rustls::pki_types::CertificateDer<'static> {
        use base64::Engine;
        let body: String = pem
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        base64::engine::general_purpose::STANDARD
            .decode(body)
            .unwrap()
            .into()
    }

    #[tokio::test]
    async fn a_secret_reaches_its_host_without_ever_being_in_the_sandbox() {
        let (origin_port, origin_root) = tls_origin().await;
        let host = format!("localhost:{origin_port}");
        let request =
            "GET /repos HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer PLACEHOLDER-1\r\n\r\n";

        let proxy = EgressProxy::start_with(
            Allowlist::new([host.clone()]),
            vec![Substitution {
                placeholder: "PLACEHOLDER-1".into(),
                secret: "ghp_the_real_token".into(),
                hosts: vec![host.clone()],
            }],
            vec![origin_root.clone()],
        )
        .await
        .unwrap();
        let authority = pem_to_der(proxy.authority_pem().unwrap());

        // The tool trusts the proxy's authority and sends the placeholder;
        // the host receives the secret.
        let response = through_tunnel(proxy.port(), origin_port, authority.clone(), request)
            .await
            .unwrap();
        assert!(
            response.contains("Authorization: Bearer ghp_the_real_token"),
            "{response}"
        );
        assert!(!response.contains("PLACEHOLDER-1"), "{response}");
        assert!(response.contains("Connection: close"), "{response}");

        // A host with no secret of its own is tunnelled untouched: the
        // tool sees the host's own certificate and its placeholder stays
        // a placeholder.
        let plain = EgressProxy::start_with(
            Allowlist::new([host.clone()]),
            vec![Substitution {
                placeholder: "PLACEHOLDER-1".into(),
                secret: "ghp_the_real_token".into(),
                hosts: vec!["api.github.com".into()],
            }],
            vec![origin_root.clone()],
        )
        .await
        .unwrap();
        let untouched = through_tunnel(plain.port(), origin_port, origin_root.clone(), request)
            .await
            .unwrap();
        assert!(untouched.contains("Bearer PLACEHOLDER-1"), "{untouched}");
        assert!(
            through_tunnel(plain.port(), origin_port, authority, request)
                .await
                .is_err(),
            "the proxy's authority does not speak for hosts it does not intercept"
        );
        assert!(
            EgressProxy::start(Allowlist::default())
                .await
                .unwrap()
                .authority_pem()
                .is_none()
        );
    }
}
