//! `hame://` links.
//!
//! - `hame://open` brings the window to the front;
//! - `hame://open?session=<id>` also opens that session;
//! - `hame://new` starts a new session.
//!
//! Anything else is ignored: a link is input from any web page or document
//! the user clicks, so it can only choose what to show, never act.

use url::Url;

/// What a link asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeepLink {
    Open { session: Option<String> },
    New,
}

/// Whether `id` can be a session id: the interface's own rule
/// (`#/s/<id>`), letters, digits, `-` and `_`, 1 to 64 of them.
pub fn is_session_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Reads a `hame://` link. `None` for another scheme, an unknown action or a
/// malformed session id.
pub fn parse(url: &Url) -> Option<DeepLink> {
    if url.scheme() != "hame" {
        return None;
    }
    // `hame://open?…` puts the action in the host; `hame:open?…` in the path.
    let action = match url.host_str() {
        Some(host) => host.to_string(),
        None => url.path().trim_matches('/').to_string(),
    };
    let rest = url.path().trim_matches('/');
    if url.host_str().is_some() && !rest.is_empty() {
        return None;
    }
    match action.to_ascii_lowercase().as_str() {
        "open" => {
            let mut session = None;
            for (key, value) in url.query_pairs() {
                if key == "session" {
                    if session.is_some() || !is_session_id(&value) {
                        return None;
                    }
                    session = Some(value.into_owned());
                }
            }
            Some(DeepLink::Open { session })
        }
        "new" => Some(DeepLink::New),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(raw: &str) -> Option<DeepLink> {
        parse(&Url::parse(raw).unwrap())
    }

    #[test]
    fn open_with_and_without_a_session() {
        assert_eq!(
            link("hame://open?session=0193abc-DEF_1"),
            Some(DeepLink::Open {
                session: Some("0193abc-DEF_1".into())
            })
        );
        assert_eq!(link("hame://open"), Some(DeepLink::Open { session: None }));
        assert_eq!(link("hame://open/"), Some(DeepLink::Open { session: None }));
        assert_eq!(
            link("hame://OPEN?session=a"),
            Some(DeepLink::Open {
                session: Some("a".into())
            })
        );
        assert_eq!(
            link("hame:open?session=a"),
            Some(DeepLink::Open {
                session: Some("a".into())
            })
        );
        // Other parameters are ignored.
        assert_eq!(
            link("hame://open?utm=x&session=a"),
            Some(DeepLink::Open {
                session: Some("a".into())
            })
        );
    }

    #[test]
    fn new_session() {
        assert_eq!(link("hame://new"), Some(DeepLink::New));
    }

    #[test]
    fn anything_else_is_ignored() {
        for raw in [
            "https://open?session=a",
            "hame://delete?session=a",
            "hame://open?session=",
            "hame://open?session=../../etc",
            "hame://open?session=a%20b",
            "hame://open?session=%3Cscript%3E",
            "hame://open?session=a&session=b",
            "hame://open/extra?session=a",
            &format!("hame://open?session={}", "a".repeat(65)),
        ] {
            assert_eq!(link(raw), None, "{raw}");
        }
    }

    #[test]
    fn session_ids() {
        assert!(is_session_id("019a4f2c-7e1b-7c3d-9a0b-1234567890ab"));
        assert!(!is_session_id(""));
        assert!(!is_session_id("a/b"));
        assert!(!is_session_id("a\"b"));
    }
}
