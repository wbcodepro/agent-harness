//! Plugins packaged for Claude Code: a directory bundling skills, hooks
//! and MCP servers. The parts that have an equivalent here are loaded;
//! commands, subagent definitions and the rest are left alone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::mcp::McpServerConfig;

/// The placeholder a plugin uses for its own directory.
const ROOT_VARIABLE: &str = "${CLAUDE_PLUGIN_ROOT}";

/// A plugin directory that has been found and named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudePlugin {
    pub name: String,
    pub root: PathBuf,
}

impl ClaudePlugin {
    /// Opens the plugin at `root`. Its name comes from
    /// `.claude-plugin/plugin.json`, or from the directory when that file
    /// gives none.
    pub fn open(root: &Path) -> Result<Self, String> {
        if !root.is_dir() {
            return Err(format!("plugin {} is not a directory", root.display()));
        }
        let manifest = root.join(".claude-plugin/plugin.json");
        let named = match std::fs::read_to_string(&manifest) {
            Ok(text) => serde_json::from_str::<Value>(&text)
                .map_err(|e| format!("{}: {e}", manifest.display()))?["name"]
                .as_str()
                .map(String::from),
            Err(_) => None,
        };
        let name = named
            .or_else(|| Some(root.file_name()?.to_string_lossy().into_owned()))
            .ok_or_else(|| format!("plugin {} has no name", root.display()))?;
        Ok(Self {
            name,
            root: root.to_path_buf(),
        })
    }

    /// Where the plugin keeps its skills (`<name>/SKILL.md` under it).
    pub fn skills_root(&self) -> PathBuf {
        self.root.join("skills")
    }

    /// The plugin's hooks, in the same format as a settings file.
    pub fn hooks_file(&self) -> PathBuf {
        self.root.join("hooks/hooks.json")
    }

    fn expand(&self, text: &str) -> String {
        text.replace(ROOT_VARIABLE, &self.root.to_string_lossy())
    }

    /// The MCP servers declared in the plugin's `.mcp.json`, each named
    /// `<plugin>_<server>`. A server of a kind that cannot be reached from
    /// here is an error naming it.
    pub fn mcp_servers(&self) -> Result<Vec<(String, McpServerConfig)>, String> {
        let file = self.root.join(".mcp.json");
        let Ok(text) = std::fs::read_to_string(&file) else {
            return Ok(Vec::new());
        };
        let parsed: Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
        // The servers are either the whole object or under `mcpServers`.
        let servers = match parsed.get("mcpServers") {
            Some(servers) => servers,
            None => &parsed,
        };
        let strings = |value: &Value| -> BTreeMap<String, String> {
            value
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(key, value)| Some((key.clone(), self.expand(value.as_str()?))))
                .collect()
        };
        let mut found = Vec::new();
        for (server, spec) in servers.as_object().into_iter().flatten() {
            let name = format!("{}_{server}", self.name);
            let config = match (
                spec["command"].as_str(),
                spec["url"].as_str(),
                spec["type"].as_str(),
            ) {
                (Some(command), _, None | Some("stdio")) => McpServerConfig {
                    command: self.expand(command),
                    args: spec["args"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|arg| Some(self.expand(arg.as_str()?)))
                        .collect(),
                    env: strings(&spec["env"]),
                    ..McpServerConfig::default()
                },
                (None, Some(url), None | Some("http")) => McpServerConfig {
                    url: Some(url.to_string()),
                    headers: strings(&spec["headers"]),
                    ..McpServerConfig::default()
                },
                (_, _, kind) => {
                    return Err(format!(
                        "{}: server {server} is of a kind that is not supported ({})",
                        file.display(),
                        kind.unwrap_or("no command or url")
                    ));
                }
            };
            found.push((name, config));
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_plugin_directory_gives_its_name_skills_hooks_and_servers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("release-tools");
        std::fs::create_dir_all(root.join(".claude-plugin")).unwrap();
        std::fs::write(
            root.join(".claude-plugin/plugin.json"),
            r#"{"name": "relkit", "version": "1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join(".mcp.json"),
            json!({"mcpServers": {
                "notes": {"command": "python3", "args": ["${CLAUDE_PLUGIN_ROOT}/server.py"], "env": {"DATA": "${CLAUDE_PLUGIN_ROOT}/data"}},
                "tracker": {"type": "http", "url": "https://tracker.example/mcp", "headers": {"Authorization": "Bearer $TRACKER"}},
            }})
            .to_string(),
        )
        .unwrap();

        let plugin = ClaudePlugin::open(&root).unwrap();
        assert_eq!(plugin.name, "relkit");
        assert_eq!(plugin.skills_root(), root.join("skills"));
        assert_eq!(plugin.hooks_file(), root.join("hooks/hooks.json"));
        let servers = plugin.mcp_servers().unwrap();
        assert_eq!(servers[0].0, "relkit_notes");
        assert_eq!(servers[0].1.command, "python3");
        // The placeholder is replaced as text, so what follows it keeps
        // the separator the plugin wrote.
        assert_eq!(servers[0].1.args, [format!("{}/server.py", root.display())]);
        assert_eq!(servers[0].1.env["DATA"], format!("{}/data", root.display()));
        assert_eq!(servers[1].0, "relkit_tracker");
        assert_eq!(
            servers[1].1.url.as_deref(),
            Some("https://tracker.example/mcp")
        );
        assert_eq!(servers[1].1.headers["Authorization"], "Bearer $TRACKER");

        // Without a manifest the directory names the plugin; without an
        // .mcp.json there are simply no servers.
        let bare = dir.path().join("bare");
        std::fs::create_dir_all(&bare).unwrap();
        let plugin = ClaudePlugin::open(&bare).unwrap();
        assert_eq!(plugin.name, "bare");
        assert!(plugin.mcp_servers().unwrap().is_empty());

        std::fs::write(
            bare.join(".mcp.json"),
            r#"{"old": {"type": "sse", "url": "https://x.example/sse"}}"#,
        )
        .unwrap();
        assert!(
            plugin
                .mcp_servers()
                .unwrap_err()
                .contains("not supported (sse)")
        );
        assert!(ClaudePlugin::open(&dir.path().join("missing")).is_err());
    }
}
