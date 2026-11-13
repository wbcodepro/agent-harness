use std::sync::Arc;

use zcode_agent::ToolRequest;
use zcode_kernel::{BoxFuture, Layer, Next};
use zcode_tools::{Effect, ToolOutput};

use crate::config::{Rule, RuleAction};
use crate::secrets::Secrets;

/// Matches `text` against `pattern`, where `*` matches any run of
/// characters (including none) and everything else is literal.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let mut parts: Vec<&str> = parts.collect();
    let Some(last) = parts.pop() else {
        // No `*` at all: the whole text must have matched.
        return rest.is_empty();
    };
    for part in parts {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// What a rule's pattern is matched against.
fn subject(request: &ToolRequest) -> &str {
    let key = if request.name == "bash" {
        "command"
    } else {
        "path"
    };
    request.input[key].as_str().unwrap_or("")
}

/// Asks the user whether a tool call may run.
pub trait Prompter: Send + Sync {
    /// `request` is the call awaiting a decision and `question` a one-line
    /// rendering of it. `None` means nobody can be asked.
    fn ask<'a>(&'a self, request: &'a ToolRequest, question: String)
    -> BoxFuture<'a, Option<bool>>;
}

/// Decides whether each tool call runs: the first matching rule, else the
/// default.
pub struct ApprovalLayer {
    pub rules: Vec<Rule>,
    /// Applied to calls that are not read-only when no rule matches.
    pub default: RuleAction,
    pub prompter: Arc<dyn Prompter>,
}

impl ApprovalLayer {
    fn decide(&self, request: &ToolRequest) -> RuleAction {
        let subject = subject(request);
        for rule in &self.rules {
            let tool_ok = rule.tool == "*" || rule.tool == request.name;
            let pattern_ok = rule
                .pattern
                .as_deref()
                .is_none_or(|pattern| glob_match(pattern, subject));
            if tool_ok && pattern_ok {
                return rule.action;
            }
        }
        if request.effect == Effect::ReadOnly {
            RuleAction::Allow
        } else {
            self.default
        }
    }
}

impl Layer<ToolRequest, ToolOutput> for ApprovalLayer {
    fn call<'a>(
        &'a self,
        request: ToolRequest,
        next: Next<'a, ToolRequest, ToolOutput>,
    ) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            match self.decide(&request) {
                RuleAction::Allow => next.run(request).await,
                RuleAction::Deny => ToolOutput::error(
                    "Denied by the user's permission rules. Do not retry this call.",
                ),
                RuleAction::Ask => {
                    let mut shown: String = subject(&request).chars().take(200).collect();
                    if shown.is_empty() {
                        shown = request.input.to_string().chars().take(200).collect();
                    }
                    let question = format!("Allow {} {}? [y/N] ", request.name, shown);
                    match self.prompter.ask(&request, question).await {
                        Some(true) => next.run(request).await,
                        Some(false) => ToolOutput::error("Denied by the user."),
                        None => ToolOutput::error(
                            "Denied: this call needs the user's approval and nobody is at the \
                             terminal to give it. Do not retry; tell the user what you needed.",
                        ),
                    }
                }
            }
        })
    }
}

/// Removes secret values from tool output before anything else sees it.
pub struct RedactLayer {
    pub secrets: Secrets,
}

impl Layer<ToolRequest, ToolOutput> for RedactLayer {
    fn call<'a>(
        &'a self,
        request: ToolRequest,
        next: Next<'a, ToolRequest, ToolOutput>,
    ) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let mut output = next.run(request).await;
            output.text = self.secrets.redact(&output.text);
            output
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;
    use zcode_kernel::Chain;

    #[test]
    fn globs_match_literally_except_for_stars() {
        assert!(glob_match("git status", "git status"));
        assert!(!glob_match("git status", "git status -s"));
        assert!(glob_match("git *", "git push origin"));
        assert!(glob_match("*", ""));
        assert!(glob_match("*.rs", "src/a.rs"));
        assert!(!glob_match("*.rs", "src/a.rsx"));
        assert!(glob_match("rm *-rf*", "rm -rf /"));
        assert!(glob_match("a*b*c", "a-b-b-c"));
        assert!(!glob_match("a*b*c", "a-c-b"));
        assert!(!glob_match("ab*ba", "aba"));
    }

    struct Scripted {
        answer: Option<bool>,
        asked: Mutex<Vec<String>>,
    }

    impl Prompter for Scripted {
        fn ask<'a>(
            &'a self,
            _request: &'a ToolRequest,
            question: String,
        ) -> BoxFuture<'a, Option<bool>> {
            self.asked.lock().unwrap().push(question);
            Box::pin(async move { self.answer })
        }
    }

    fn request(name: &str, input: serde_json::Value, effect: Effect) -> ToolRequest {
        ToolRequest {
            call_id: "c".into(),
            name: name.into(),
            input,
            effect,
        }
    }

    async fn ran(chain: &Chain<ToolRequest, ToolOutput>, request: ToolRequest) -> ToolOutput {
        chain
            .run(request, |_| async { ToolOutput::ok("ran") })
            .await
    }

    #[tokio::test]
    async fn rules_apply_in_order_then_the_default() {
        let rule = |tool: &str, pattern: Option<&str>, action| Rule {
            tool: tool.into(),
            pattern: pattern.map(String::from),
            action,
        };
        let prompter = Arc::new(Scripted {
            answer: Some(false),
            asked: Mutex::new(Vec::new()),
        });
        let chain = Chain::new();
        chain.push(ApprovalLayer {
            rules: vec![
                rule("bash", Some("git push*"), RuleAction::Ask),
                rule("bash", Some("rm *"), RuleAction::Deny),
                rule("write", Some("*/secrets/*"), RuleAction::Deny),
                rule("read", Some("*.pem"), RuleAction::Deny),
            ],
            default: RuleAction::Allow,
            prompter: prompter.clone(),
        });
        let bash = |command: &str| request("bash", json!({"command": command}), Effect::Execute);

        assert_eq!(ran(&chain, bash("ls")).await, ToolOutput::ok("ran"));
        assert!(
            ran(&chain, bash("rm -rf x"))
                .await
                .text
                .contains("permission rules")
        );
        assert_eq!(
            ran(&chain, bash("git push")).await.text,
            "Denied by the user."
        );
        assert_eq!(
            *prompter.asked.lock().unwrap(),
            ["Allow bash git push? [y/N] "]
        );
        let write = |path: &str| request("write", json!({"path": path}), Effect::Write);
        assert!(ran(&chain, write("/w/secrets/k")).await.is_error);
        assert!(!ran(&chain, write("/w/src/a.rs")).await.is_error);
        // Rules can restrict even read-only tools.
        let read = |path: &str| request("read", json!({"path": path}), Effect::ReadOnly);
        assert!(ran(&chain, read("id.pem")).await.is_error);
        assert!(!ran(&chain, read("a.rs")).await.is_error);
    }

    #[tokio::test]
    async fn asking_with_nobody_there_denies_and_reads_never_ask() {
        let chain = Chain::new();
        chain.push(ApprovalLayer {
            rules: Vec::new(),
            default: RuleAction::Ask,
            prompter: Arc::new(Scripted {
                answer: None,
                asked: Mutex::new(Vec::new()),
            }),
        });
        let denied = ran(
            &chain,
            request("bash", json!({"command": "ls"}), Effect::Execute),
        )
        .await;
        assert!(denied.is_error && denied.text.contains("nobody is at the terminal"));
        let read = ran(
            &chain,
            request("read", json!({"path": "a"}), Effect::ReadOnly),
        )
        .await;
        assert_eq!(read, ToolOutput::ok("ran"));
    }

    #[tokio::test]
    async fn redaction_wraps_whatever_ran_beneath_it() {
        let mut secrets = Secrets::new();
        secrets.insert("API_KEY", "sk-live-12345678");
        let chain = Chain::new();
        chain.push(RedactLayer { secrets });
        let output = chain
            .run(
                request("bash", json!({"command": "env"}), Effect::Execute),
                |_| async { ToolOutput::error("API_KEY=sk-live-12345678\n[exit code 1]") },
            )
            .await;
        assert_eq!(
            output,
            ToolOutput::error("API_KEY=[secret:API_KEY]\n[exit code 1]")
        );
    }
}
