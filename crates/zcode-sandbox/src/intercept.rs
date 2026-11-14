//! Keeping a secret out of the sandbox while still letting tools use it.
//!
//! A tool is given a placeholder in place of the secret. When it sends
//! the placeholder to one of the hosts the user named for that secret,
//! the proxy puts the real value in. For HTTPS the proxy has to read the
//! request to do that, so for those hosts only it ends the tool's TLS
//! connection itself, with a certificate from an authority made for this
//! run, and opens its own verified connection to the real host.
//!
//! Only the request head is rewritten (the request line and headers,
//! including a Basic `Authorization` header): changing a body would change
//! its length. Each connection carries one request, so every request's
//! head passes through here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, IsCa, KeyPair, KeyUsagePurpose,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::proxy::Allowlist;

/// A secret, what stands in for it inside the sandbox, and the hosts it
/// may be sent to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Substitution {
    pub placeholder: String,
    pub secret: String,
    pub hosts: Vec<String>,
}

/// The certificate authority of one proxy, and the TLS settings for both
/// sides of an intercepted connection.
pub(crate) struct Authority {
    issuer: CertifiedIssuer<'static, KeyPair>,
    upstream: Arc<rustls::ClientConfig>,
    /// One certificate per host, made the first time the host is seen.
    leaves: Mutex<HashMap<String, Arc<rustls::ServerConfig>>>,
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

impl Authority {
    /// `extra_roots` are trusted in addition to the system's, for hosts
    /// with a private authority.
    pub(crate) fn new(extra_roots: Vec<CertificateDer<'static>>) -> Result<Self, String> {
        let problem = |e: &dyn std::fmt::Display| format!("cannot set up TLS interception: {e}");
        let mut params = CertificateParams::new(Vec::<String>::new()).map_err(|e| problem(&e))?;
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params
            .distinguished_name
            .push(DnType::CommonName, "zcode sandbox (this run only)");
        let key = KeyPair::generate().map_err(|e| problem(&e))?;
        let issuer = CertifiedIssuer::self_signed(params, key).map_err(|e| problem(&e))?;

        let mut roots = rustls::RootCertStore::empty();
        for certificate in rustls_native_certs::load_native_certs().certs {
            // A system store can hold certificates rustls will not take.
            let _ = roots.add(certificate);
        }
        for certificate in extra_roots {
            roots.add(certificate).map_err(|e| problem(&e))?;
        }
        let mut upstream = rustls::ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| problem(&e))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        // One request per connection, read as text: HTTP/1.1 only.
        upstream.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Self {
            issuer,
            upstream: Arc::new(upstream),
            leaves: Mutex::new(HashMap::new()),
        })
    }

    /// The authority's certificate, for tools to trust.
    pub(crate) fn pem(&self) -> String {
        self.issuer.pem()
    }

    pub(crate) fn upstream(&self) -> Arc<rustls::ClientConfig> {
        Arc::clone(&self.upstream)
    }

    /// TLS settings for answering as `host`.
    pub(crate) fn answering_as(&self, host: &str) -> Result<Arc<rustls::ServerConfig>, String> {
        if let Some(known) = self.leaves.lock().expect("leaf cache poisoned").get(host) {
            return Ok(Arc::clone(known));
        }
        let problem =
            |e: &dyn std::fmt::Display| format!("cannot make a certificate for {host}: {e}");
        let mut params = CertificateParams::new(vec![host.to_string()]).map_err(|e| problem(&e))?;
        params.distinguished_name.push(DnType::CommonName, host);
        let key = KeyPair::generate().map_err(|e| problem(&e))?;
        let leaf = params
            .signed_by(&key, &self.issuer)
            .map_err(|e| problem(&e))?;
        let mut config = rustls::ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| problem(&e))?
            .with_no_client_auth()
            .with_single_cert(
                vec![leaf.der().clone(), self.issuer.der().clone()],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            )
            .map_err(|e| problem(&e))?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let config = Arc::new(config);
        self.leaves
            .lock()
            .expect("leaf cache poisoned")
            .insert(host.to_string(), Arc::clone(&config));
        Ok(config)
    }
}

/// The system's trusted authorities as one PEM text, or `None` if the
/// system gives none.
pub(crate) fn system_roots_pem() -> Option<String> {
    let certificates = rustls_native_certs::load_native_certs().certs;
    if certificates.is_empty() {
        return None;
    }
    let mut pem = String::new();
    for certificate in certificates {
        pem.push_str("-----BEGIN CERTIFICATE-----\n");
        let encoded = STANDARD.encode(&certificate);
        for line in encoded.as_bytes().chunks(64) {
            pem.push_str(&String::from_utf8_lossy(line));
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");
    }
    Some(pem)
}

/// The substitutions that apply to a connection to `host:port`.
pub(crate) fn applying<'a>(
    substitutions: &'a [Substitution],
    host: &str,
    port: u16,
) -> Vec<&'a Substitution> {
    substitutions
        .iter()
        .filter(|substitution| Allowlist::new(substitution.hosts.clone()).allows(host, port))
        .collect()
}

/// A request head with every placeholder replaced by its secret, and
/// marked to be the only request on its connection.
///
/// Credentials sent as `Authorization: Basic` are base64, so a
/// placeholder inside them is looked for after decoding.
pub(crate) fn rewrite_head(head: &str, substitutions: &[&Substitution]) -> String {
    let replace = |text: &str| {
        substitutions
            .iter()
            .fold(text.to_string(), |text, substitution| {
                text.replace(&substitution.placeholder, &substitution.secret)
            })
    };
    let mut out = String::new();
    for line in head.split("\r\n") {
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("connection:") || lower.starts_with("proxy-") {
            continue;
        }
        let basic = lower
            .strip_prefix("authorization:")
            .map(str::trim_start)
            .filter(|value| value.starts_with("basic "))
            .and_then(|_| line.split_once(':'))
            .and_then(|(_, value)| value.trim().split_once(' '))
            .and_then(|(_, encoded)| STANDARD.decode(encoded.trim()).ok())
            .and_then(|decoded| String::from_utf8(decoded).ok());
        match basic {
            Some(credentials) => out.push_str(&format!(
                "Authorization: Basic {}",
                STANDARD.encode(replace(&credentials))
            )),
            None => out.push_str(&replace(line)),
        }
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_replaced_in_the_head_only_for_their_hosts() {
        let substitutions = [
            Substitution {
                placeholder: "PLACEHOLDER-A".into(),
                secret: "real-a".into(),
                hosts: vec!["api.example.com".into()],
            },
            Substitution {
                placeholder: "PLACEHOLDER-B".into(),
                secret: "real-b".into(),
                hosts: vec!["*.internal.test:8443".into()],
            },
        ];
        let names = |host: &str, port: u16| -> Vec<&str> {
            applying(&substitutions, host, port)
                .iter()
                .map(|substitution| substitution.secret.as_str())
                .collect()
        };
        assert_eq!(names("api.example.com", 443), ["real-a"]);
        assert_eq!(names("git.internal.test", 8443), ["real-b"]);
        assert!(names("evil.example.net", 443).is_empty());
        assert!(names("git.internal.test", 443).is_empty());

        let basic = STANDARD.encode("user:PLACEHOLDER-A");
        let head = "GET /v1?key=PLACEHOLDER-A HTTP/1.1\r\nHost: api.example.com\r\n\
                    Authorization: Bearer PLACEHOLDER-A\r\nConnection: keep-alive\r\n\
                    Proxy-Authorization: x\r\nX-Other: PLACEHOLDER-B\r\n\r\n";
        let rewritten = rewrite_head(head, &applying(&substitutions, "api.example.com", 443));
        assert_eq!(
            rewritten,
            "GET /v1?key=real-a HTTP/1.1\r\nHost: api.example.com\r\n\
             Authorization: Bearer real-a\r\nX-Other: PLACEHOLDER-B\r\nConnection: close\r\n\r\n"
        );
        let with_basic = format!("GET / HTTP/1.1\r\nAuthorization: Basic {basic}\r\n\r\n");
        let rewritten = rewrite_head(
            &with_basic,
            &applying(&substitutions, "api.example.com", 443),
        );
        assert!(rewritten.contains(&format!(
            "Authorization: Basic {}",
            STANDARD.encode("user:real-a")
        )));
    }
}
