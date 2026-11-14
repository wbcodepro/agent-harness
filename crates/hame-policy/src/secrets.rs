use std::path::Path;

/// Named secret values the harness holds: provider keys and anything else
/// loaded from the environment or an env file.
///
/// The model never needs the values. Tools get only the ones the user
/// grants, and every tool output is passed through [`Secrets::redact`]
/// before it is logged or shown to the model.
#[derive(Clone, Default)]
pub struct Secrets {
    entries: Vec<(String, String)>,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.entries.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("Secrets").field("names", &names).finish()
    }
}

/// Values shorter than this are not redacted: replacing every occurrence of
/// a short string would corrupt ordinary output.
const MIN_REDACTED_LEN: usize = 8;

fn looks_secret(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    ["KEY", "TOKEN", "SECRET", "PASSWORD", "PASSWD", "CREDENTIAL"]
        .iter()
        .any(|marker| name.contains(marker))
}

impl Secrets {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces a secret. Empty values are ignored.
    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let (name, value) = (name.into(), value.into());
        if value.is_empty() {
            return;
        }
        self.entries.retain(|(existing, _)| *existing != name);
        self.entries.push((name, value));
    }

    /// Adds every process environment variable whose name looks like a
    /// credential.
    pub fn load_process_env(&mut self) {
        for (name, value) in std::env::vars() {
            if looks_secret(&name) {
                self.insert(name, value);
            }
        }
    }

    /// Adds every `NAME=value` line of an env file. A missing file is fine.
    /// Returns how many entries were read.
    pub fn load_env_file(&mut self, path: &Path) -> std::io::Result<usize> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        let mut count = 0;
        for line in text.lines() {
            let line = line.trim();
            let line = line.strip_prefix("export ").unwrap_or(line);
            if line.starts_with('#') {
                continue;
            }
            let Some((name, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            self.insert(name.trim(), value);
            count += 1;
        }
        Ok(count)
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(existing, _)| existing == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(name, _)| name.as_str())
    }

    /// Replaces every occurrence of a secret value with `[secret:NAME]`.
    pub fn redact(&self, text: &str) -> String {
        let mut entries: Vec<&(String, String)> = self
            .entries
            .iter()
            .filter(|(_, value)| value.len() >= MIN_REDACTED_LEN)
            .collect();
        // Longest first, so a secret containing another is replaced whole.
        entries.sort_by_key(|(_, value)| std::cmp::Reverse(value.len()));
        let mut out = text.to_string();
        for (name, value) in entries {
            if out.contains(value.as_str()) {
                out = out.replace(value.as_str(), &format!("[secret:{name}]"));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_files_are_parsed_and_values_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(
            &path,
            "# comment\nAPI_KEY=sk-abcdef123456\nexport TOKEN=\"tok en-987654\"\nSHORT=abc\nEMPTY=\n\nnot a line\n",
        )
        .unwrap();
        let mut secrets = Secrets::new();
        assert_eq!(secrets.load_env_file(&path).unwrap(), 4);
        assert_eq!(
            secrets.load_env_file(&dir.path().join("missing")).unwrap(),
            0
        );
        assert_eq!(secrets.get("TOKEN"), Some("tok en-987654"));
        assert_eq!(secrets.get("EMPTY"), None);

        let redacted =
            secrets.redact("curl -H 'x: sk-abcdef123456' ; echo tok en-987654 abc sk-abcdef123456");
        assert_eq!(
            redacted,
            "curl -H 'x: [secret:API_KEY]' ; echo [secret:TOKEN] abc [secret:API_KEY]"
        );
        assert!(!format!("{secrets:?}").contains("sk-abcdef"));
    }

    #[test]
    fn a_secret_containing_another_is_replaced_whole() {
        let mut secrets = Secrets::new();
        secrets.insert("INNER", "abcdefgh");
        secrets.insert("OUTER", "xx-abcdefgh-yy");
        assert_eq!(secrets.redact("v=xx-abcdefgh-yy"), "v=[secret:OUTER]");
    }
}
