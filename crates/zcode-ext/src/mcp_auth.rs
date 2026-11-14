//! Signing in to an MCP server that wants OAuth.
//!
//! The server says where its authorization server is; the client registers
//! itself there, sends the user to it in a browser, receives the answer on
//! a local port, and exchanges it for tokens. This is the authorization
//! code flow with PKCE, for a public client: no secret is kept, and the
//! code is useless to anyone who did not start the flow.
//!
//! Tokens are stored in the data directory, one file per server, and
//! refreshed when a session starts. They are not refreshed in the middle
//! of a session.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

/// What is kept between sessions for one server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Seconds since the epoch after which the access token is no good.
    pub expires_at: Option<u64>,
    pub token_endpoint: String,
    pub client_id: String,
    /// The MCP server the tokens are for.
    pub resource: String,
}

/// How long the user has to finish in the browser.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Where the tokens for `server` are kept.
pub fn token_path(home: &Path, server: &str) -> PathBuf {
    let name: String = server
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    home.join("mcp-auth").join(format!("{name}.json"))
}

pub fn load(path: &Path) -> Option<Tokens> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Writes the tokens so that only the user can read them.
pub fn save(path: &Path, tokens: &Tokens) -> Result<(), String> {
    let problem = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(problem)?;
    }
    let text = serde_json::to_string_pretty(tokens).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(problem)?;
        file.write_all(text.as_bytes()).map_err(problem)
    }
    #[cfg(not(unix))]
    std::fs::write(path, text).map_err(problem)
}

async fn get_json(http: &reqwest::Client, url: &str) -> Option<Value> {
    let response = http.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    serde_json::from_slice(&response.bytes().await.ok()?).ok()
}

/// Posts a form and reads the answer as JSON.
async fn post_form(
    http: &reqwest::Client,
    url: &str,
    fields: &[(&str, &str)],
) -> Result<Value, String> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    let response = http
        .post(url)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("cannot reach {url}: {e}"))?;
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| format!("the answer from {url} is unreadable: {e}"))
}

/// A random string fit for a URL, with about 240 bits in it.
fn random() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// The value of `key` in a `WWW-Authenticate` header.
fn challenge_param(header: &str, key: &str) -> Option<String> {
    let after = header.split(&format!("{key}=")).nth(1)?;
    let value = match after.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next()?,
        None => after.split([',', ' ']).next()?,
    };
    Some(value.to_string())
}

/// What signing in to a server needs to know about it.
struct Discovered {
    /// The authorization server's metadata.
    metadata: Value,
    /// The scopes the server says it needs.
    scopes: Vec<String>,
    /// The name the server gives itself, which tokens are asked for.
    resource: String,
}

/// Finds the authorization server for the MCP server at `server_url`.
async fn discover(http: &reqwest::Client, server_url: &Url) -> Result<Discovered, String> {
    let origin = server_url.origin().ascii_serialization();
    // The server points at its own description when asked without a token.
    let unauthenticated = http
        .post(server_url.clone())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(json!({"jsonrpc": "2.0", "id": 0, "method": "ping"}).to_string())
        .send()
        .await
        .map_err(|e| format!("cannot reach {server_url}: {e}"))?;
    let challenge = unauthenticated
        .headers()
        .get("www-authenticate")
        .and_then(|header| header.to_str().ok())
        .unwrap_or("")
        .to_string();
    let pointed = challenge_param(&challenge, "resource_metadata");
    let mut candidates: Vec<String> = pointed.into_iter().collect();
    candidates.push(format!(
        "{origin}/.well-known/oauth-protected-resource{}",
        server_url.path().trim_end_matches('/')
    ));
    candidates.push(format!("{origin}/.well-known/oauth-protected-resource"));
    let mut resource = None;
    for candidate in &candidates {
        if let Some(found) = get_json(http, candidate).await {
            resource = Some(found);
            break;
        }
    }
    // The scopes asked for in the refusal itself are the ones this request
    // needs; the description lists everything the server knows.
    let scopes: Vec<String> = match challenge_param(&challenge, "scope") {
        Some(asked) => asked.split_whitespace().map(String::from).collect(),
        None => resource
            .as_ref()
            .and_then(|resource| resource["scopes_supported"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|scope| scope.as_str().map(String::from))
            .collect(),
    };
    let named = resource
        .as_ref()
        .and_then(|resource| resource["resource"].as_str())
        .unwrap_or(server_url.as_str())
        .to_string();
    // No description: the server is assumed to be its own authorization
    // server, as the earlier revisions of the protocol had it.
    let issuer = resource
        .as_ref()
        .and_then(|resource| resource["authorization_servers"][0].as_str())
        .unwrap_or(&origin)
        .trim_end_matches('/')
        .to_string();
    let issuer_url =
        Url::parse(&issuer).map_err(|e| format!("bad authorization server {issuer}: {e}"))?;
    let (issuer_origin, issuer_path) = (
        issuer_url.origin().ascii_serialization(),
        issuer_url.path().trim_end_matches('/').to_string(),
    );
    for candidate in [
        format!("{issuer_origin}/.well-known/oauth-authorization-server{issuer_path}"),
        format!("{issuer_origin}/.well-known/openid-configuration{issuer_path}"),
        format!("{issuer}/.well-known/openid-configuration"),
    ] {
        if let Some(found) = get_json(http, &candidate).await
            && found["authorization_endpoint"].is_string()
            && found["token_endpoint"].is_string()
        {
            return Ok(Discovered {
                metadata: found,
                scopes,
                resource: named,
            });
        }
    }
    Err(format!(
        "{server_url} does not describe an OAuth authorization server; if it takes a fixed \
         token, put it in the server's `headers` instead"
    ))
}

/// Waits for the browser to come back to the local port, answers it, and
/// returns the query of the address it came back to.
async fn callback(listener: tokio::net::TcpListener) -> Result<Vec<(String, String)>, String> {
    loop {
        let (mut socket, _) = listener.accept().await.map_err(|e| e.to_string())?;
        let mut buffer = vec![0u8; 8192];
        let read = socket.read(&mut buffer).await.unwrap_or(0);
        let request = String::from_utf8_lossy(&buffer[..read]).to_string();
        let target = request.split_whitespace().nth(1).unwrap_or("");
        let is_callback = target.starts_with("/callback");
        let body = if is_callback {
            "You are signed in. You can close this window and go back to zcode."
        } else {
            "Not found."
        };
        let status = if is_callback {
            "200 OK"
        } else {
            "404 Not Found"
        };
        let _ = socket
            .write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await;
        let _ = socket.shutdown().await;
        // Browsers also ask for icons and the like; only the callback counts.
        if is_callback {
            let url =
                Url::parse(&format!("http://127.0.0.1{target}")).map_err(|e| e.to_string())?;
            return Ok(url
                .query_pairs()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect());
        }
    }
}

fn tokens_from(
    answer: &Value,
    token_endpoint: &str,
    client_id: &str,
    resource: &str,
    previous: Option<&Tokens>,
) -> Result<Tokens, String> {
    let access_token = answer["access_token"]
        .as_str()
        .ok_or_else(|| {
            format!(
                "the authorization server gave no access token: {}",
                answer["error_description"]
                    .as_str()
                    .or(answer["error"].as_str())
                    .unwrap_or("no reason given")
            )
        })?
        .to_string();
    Ok(Tokens {
        access_token,
        // A refresh that returns no new refresh token keeps the old one.
        refresh_token: answer["refresh_token"]
            .as_str()
            .map(String::from)
            .or_else(|| previous.and_then(|tokens| tokens.refresh_token.clone())),
        expires_at: answer["expires_in"].as_u64().map(|seconds| now() + seconds),
        token_endpoint: token_endpoint.to_string(),
        client_id: client_id.to_string(),
        resource: resource.to_string(),
    })
}

/// Signs the user in to the MCP server at `server_url`.
///
/// `show` is handed the address the user has to open; it should open a
/// browser, or at least print it.
///
/// `client` is for authorization servers where clients cannot register
/// themselves: a client id obtained from the provider beforehand, and the
/// local port its redirect address was registered with.
pub async fn login(
    server_url: &str,
    client: Option<(&str, Option<u16>)>,
    show: impl FnOnce(&str),
) -> Result<Tokens, String> {
    let http = reqwest::Client::new();
    let server =
        Url::parse(server_url).map_err(|e| format!("bad server address {server_url}: {e}"))?;
    let Discovered {
        metadata,
        scopes,
        resource,
    } = discover(&http, &server).await?;
    let resource = resource.as_str();
    let endpoint = |name: &str| metadata[name].as_str().unwrap_or("").to_string();
    let supports_s256 = metadata["code_challenge_methods_supported"]
        .as_array()
        .is_none_or(|methods| methods.iter().any(|method| method == "S256"));
    if !supports_s256 {
        return Err(
            "the authorization server does not support PKCE with S256, which this \
                    client requires"
                .into(),
        );
    }

    let wanted_port = client.and_then(|(_, port)| port).unwrap_or(0);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", wanted_port))
        .await
        .map_err(|e| format!("cannot listen for the sign-in to come back: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}/callback");

    let client_id = match client {
        Some((id, _)) => id.to_string(),
        None => {
            // The client introduces itself to the authorization server.
            let registration = endpoint("registration_endpoint");
            if registration.is_empty() {
                return Err(
                    "the authorization server does not let clients register themselves; \
                     register zcode with the provider and set `oauth_client_id` (and \
                     `oauth_redirect_port`) for this server"
                        .into(),
                );
            }
            let request = json!({
                "client_name": "zcode",
                "redirect_uris": [redirect],
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
            });
            let response = http
                .post(&registration)
                .header("content-type", "application/json")
                .body(request.to_string())
                .send()
                .await
                .map_err(|e| format!("cannot register with the authorization server: {e}"))?;
            let bytes = response.bytes().await.map_err(|e| e.to_string())?;
            let registered: Value = serde_json::from_slice(&bytes).map_err(|e| {
                format!("the authorization server's registration answer is unreadable: {e}")
            })?;
            registered["client_id"]
                .as_str()
                .ok_or("the authorization server did not register this client")?
                .to_string()
        }
    };

    let (verifier, state) = (random(), random());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut authorize = Url::parse(&endpoint("authorization_endpoint"))
        .map_err(|e| format!("bad authorization endpoint: {e}"))?;
    {
        let mut query = authorize.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &client_id)
            .append_pair("redirect_uri", &redirect)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("resource", resource);
        if !scopes.is_empty() {
            query.append_pair("scope", &scopes.join(" "));
        }
    }
    show(authorize.as_str());

    let came_back = tokio::time::timeout(SIGN_IN_TIMEOUT, callback(listener))
        .await
        .map_err(|_| "nobody finished signing in within five minutes".to_string())??;
    let param = |name: &str| {
        came_back
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    // An answer to a flow this process did not start is not ours to use.
    if param("state") != Some(state.as_str()) {
        return Err("the sign-in answer does not belong to this request; nothing was saved".into());
    }
    let code = param("code").ok_or_else(|| {
        format!(
            "sign-in was refused: {}",
            param("error_description")
                .or(param("error"))
                .unwrap_or("no reason given")
        )
    })?;

    let token_endpoint = endpoint("token_endpoint");
    let answer = post_form(
        &http,
        &token_endpoint,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &redirect),
            ("client_id", &client_id),
            ("code_verifier", &verifier),
            ("resource", resource),
        ],
    )
    .await?;
    tokens_from(&answer, &token_endpoint, &client_id, resource, None)
}

/// The access token stored at `path`, refreshed first if it is about to
/// run out. `None` when there is none, or it has expired and cannot be
/// renewed: the user then has to sign in again.
pub async fn access_token(path: &Path) -> Option<String> {
    let tokens = load(path)?;
    let fresh_enough = tokens.expires_at.is_none_or(|at| at > now() + 60);
    if fresh_enough {
        return Some(tokens.access_token);
    }
    let refresh = tokens.refresh_token.clone()?;
    let answer = post_form(
        &reqwest::Client::new(),
        &tokens.token_endpoint,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh.as_str()),
            ("client_id", tokens.client_id.as_str()),
            ("resource", tokens.resource.as_str()),
        ],
    )
    .await
    .ok()?;
    let renewed = tokens_from(
        &answer,
        &tokens.token_endpoint,
        &tokens.client_id,
        &tokens.resource,
        Some(&tokens),
    )
    .ok()?;
    let _ = save(path, &renewed);
    Some(renewed.access_token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// One server playing the MCP server and its authorization server.
    /// It remembers the PKCE challenge it was shown, and gives out a token
    /// that has already expired, so the first use has to refresh it.
    async fn serve(challenge: Arc<Mutex<String>>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let origin = base.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let (origin, challenge) = (origin.clone(), challenge.clone());
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let (head, body) = loop {
                        let n = socket.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        raw.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&raw).to_string();
                        if let Some((head, body)) = text.split_once("\r\n\r\n") {
                            let length: usize = head
                                .lines()
                                .find_map(|line| {
                                    line.to_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse().unwrap())
                                })
                                .unwrap_or(0);
                            if body.len() >= length {
                                break (head.to_string(), body.to_string());
                            }
                        }
                    };
                    let target = head.split_whitespace().nth(1).unwrap_or("").to_string();
                    let form: Vec<(String, String)> = url::form_urlencoded::parse(body.as_bytes())
                        .into_owned()
                        .collect();
                    let field = |name: &str| {
                        form.iter()
                            .find(|(key, _)| key == name)
                            .map(|(_, value)| value.clone())
                            .unwrap_or_default()
                    };
                    let (status, extra, payload) = match target.as_str() {
                        "/mcp" => (
                            "401 Unauthorized",
                            format!(
                                "WWW-Authenticate: Bearer resource_metadata=\"{origin}/described\"\r\n"
                            ),
                            json!({}),
                        ),
                        "/described" => (
                            "200 OK",
                            String::new(),
                            json!({"authorization_servers": [origin], "scopes_supported": ["tools"]}),
                        ),
                        "/.well-known/oauth-authorization-server" => (
                            "200 OK",
                            String::new(),
                            json!({
                                "authorization_endpoint": format!("{origin}/authorize"),
                                "token_endpoint": format!("{origin}/token"),
                                "registration_endpoint": format!("{origin}/register"),
                                "code_challenge_methods_supported": ["S256"],
                            }),
                        ),
                        "/register" => ("200 OK", String::new(), json!({"client_id": "c-1"})),
                        "/token" if field("grant_type") == "authorization_code" => {
                            let proved = URL_SAFE_NO_PAD
                                .encode(Sha256::digest(field("code_verifier").as_bytes()));
                            let good = field("code") == "code-9"
                                && proved == *challenge.lock().unwrap()
                                && field("client_id") == "c-1"
                                && field("resource").ends_with("/mcp");
                            if good {
                                (
                                    "200 OK",
                                    String::new(),
                                    json!({"access_token": "at-1", "refresh_token": "rt-1", "expires_in": 0}),
                                )
                            } else {
                                (
                                    "400 Bad Request",
                                    String::new(),
                                    json!({"error": "invalid_grant"}),
                                )
                            }
                        }
                        "/token" if field("refresh_token") == "rt-1" => (
                            "200 OK",
                            String::new(),
                            json!({"access_token": "at-2", "expires_in": 3600}),
                        ),
                        _ => (
                            "404 Not Found",
                            String::new(),
                            json!({"error": "not found"}),
                        ),
                    };
                    let payload = payload.to_string();
                    let _ = socket
                        .write_all(
                            format!(
                                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra}\
                                 Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                                payload.len()
                            )
                            .as_bytes(),
                        )
                        .await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        base
    }

    /// Stands in for the user's browser: looks at the address it was sent
    /// to and comes back to the local port with `code` and `state`.
    fn browser(
        challenge: Arc<Mutex<String>>,
        seen: Arc<Mutex<Vec<(String, String)>>>,
        code: &'static str,
        forge_state: bool,
    ) -> impl FnOnce(&str) {
        move |address: &str| {
            let url = Url::parse(address).unwrap();
            let query: Vec<(String, String)> = url.query_pairs().into_owned().collect();
            let get = |name: &str| {
                query
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.clone())
                    .unwrap_or_default()
            };
            *challenge.lock().unwrap() = get("code_challenge");
            let state = if forge_state {
                "forged".to_string()
            } else {
                get("state")
            };
            let back = format!("{}?code={code}&state={state}", get("redirect_uri"));
            *seen.lock().unwrap() = query;
            tokio::spawn(async move {
                let _ = reqwest::Client::new().get(back).send().await;
            });
        }
    }

    #[tokio::test]
    async fn signing_in_proves_itself_with_pkce_and_tokens_renew_themselves() {
        let dir = tempfile::tempdir().unwrap();
        let challenge = Arc::new(Mutex::new(String::new()));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = serve(challenge.clone()).await;
        let server = format!("{base}/mcp");

        let tokens = login(
            &server,
            None,
            browser(challenge.clone(), seen.clone(), "code-9", false),
        )
        .await
        .unwrap();
        assert_eq!(tokens.access_token, "at-1");
        assert_eq!(tokens.refresh_token.as_deref(), Some("rt-1"));
        assert_eq!(tokens.client_id, "c-1");
        // What the user was sent to: a public client asking for a code,
        // bound to this server, with the scopes the server named.
        let asked = seen.lock().unwrap().clone();
        let param = |name: &str| {
            asked
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
                .unwrap_or("")
                .to_string()
        };
        assert_eq!(param("response_type"), "code");
        assert_eq!(param("code_challenge_method"), "S256");
        assert_eq!(param("resource"), server);
        assert_eq!(param("scope"), "tools");
        assert!(param("redirect_uri").starts_with("http://127.0.0.1:"));

        // Stored for the user alone, and renewed when it has run out.
        let path = token_path(dir.path(), "docs server/1");
        assert!(path.ends_with("mcp-auth/docs_server_1.json"));
        save(&path, &tokens).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(access_token(&path).await.as_deref(), Some("at-2"));
        let renewed = load(&path).unwrap();
        assert_eq!(
            renewed.refresh_token.as_deref(),
            Some("rt-1"),
            "kept when not replaced"
        );
        assert_eq!(
            access_token(&path).await.as_deref(),
            Some("at-2"),
            "still fresh: no request"
        );
        assert_eq!(access_token(&dir.path().join("none.json")).await, None);
    }

    #[tokio::test]
    async fn an_answer_that_is_not_to_our_request_or_a_wrong_code_gives_no_tokens() {
        let challenge = Arc::new(Mutex::new(String::new()));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let server = format!("{}/mcp", serve(challenge.clone()).await);

        let forged = login(
            &server,
            None,
            browser(challenge.clone(), seen.clone(), "code-9", true),
        )
        .await
        .unwrap_err();
        assert!(
            forged.contains("does not belong to this request"),
            "{forged}"
        );

        let refused = login(
            &server,
            None,
            browser(challenge.clone(), seen.clone(), "stolen", false),
        )
        .await
        .unwrap_err();
        assert!(refused.contains("gave no access token"), "{refused}");
    }

    #[tokio::test]
    async fn a_client_id_from_the_provider_skips_registration() {
        let challenge = Arc::new(Mutex::new(String::new()));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let server = format!("{}/mcp", serve(challenge.clone()).await);
        // The fake server's token endpoint only accepts client c-1, which
        // here is given, not registered; the redirect uses the fixed port.
        let port = {
            let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            probe.local_addr().unwrap().port()
        };
        let tokens = login(
            &server,
            Some(("c-1", Some(port))),
            browser(challenge.clone(), seen.clone(), "code-9", false),
        )
        .await
        .unwrap();
        assert_eq!(tokens.client_id, "c-1");
        let asked = seen.lock().unwrap().clone();
        let redirect = asked
            .iter()
            .find(|(key, _)| key == "redirect_uri")
            .unwrap()
            .1
            .clone();
        assert_eq!(redirect, format!("http://127.0.0.1:{port}/callback"));
    }

    #[test]
    fn the_challenge_header_is_read_quoted_or_not() {
        let header = r#"Bearer realm="x", resource_metadata="https://h.example/meta", scope="a b""#;
        assert_eq!(
            challenge_param(header, "resource_metadata").unwrap(),
            "https://h.example/meta"
        );
        assert_eq!(
            challenge_param(
                "Bearer resource_metadata=https://h/m, x=1",
                "resource_metadata"
            )
            .unwrap(),
            "https://h/m"
        );
        assert_eq!(challenge_param("Bearer", "resource_metadata"), None);
        assert_eq!(challenge_param(header, "scope").unwrap(), "a b");
    }

    /// Reads the public metadata of real servers; needs the network, so it
    /// only runs when asked for (`cargo test -- --ignored`). It registers
    /// nothing and signs in to nothing.
    #[tokio::test]
    #[ignore]
    async fn real_servers_describe_themselves_the_way_discovery_expects() {
        let http = reqwest::Client::new();
        for server in ["https://mcp.linear.app/mcp", "https://mcp.notion.com/mcp"] {
            let found = discover(&http, &Url::parse(server).unwrap()).await.unwrap();
            assert!(
                found.metadata["authorization_endpoint"].is_string(),
                "{server}"
            );
            assert!(
                found.metadata["registration_endpoint"].is_string(),
                "{server}"
            );
            assert!(!found.scopes.is_empty(), "{server}");
            assert!(found.resource.starts_with("https://"), "{server}");
        }
    }
}
