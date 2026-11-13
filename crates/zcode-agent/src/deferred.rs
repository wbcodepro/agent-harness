//! Tools the model can use without having them all in its prompt.
//!
//! Every declared tool costs prompt tokens on every request, and a few MCP
//! servers can add hundreds. Deferred tools are left out of the declared
//! list; the model finds one with `tool_search` and runs it with
//! `tool_call`. The declared list never changes, so the provider's prompt
//! cache stays valid, and the call is checked by the permission layers
//! under the real tool's name, not under `tool_call`.

use std::sync::Arc;

use serde_json::{Value, json};
use zcode_kernel::BoxFuture;
use zcode_session::ToolSpec;
use zcode_tools::{Effect, Tool, ToolContext, ToolOutput, ToolSet};

pub const SEARCH_TOOL: &str = "tool_search";
pub const CALL_TOOL: &str = "tool_call";

/// Query words that do not help tell tools apart.
const FILLER: [&str; 12] = [
    "an", "the", "to", "of", "in", "on", "for", "and", "or", "with", "is", "it",
];

/// How many tools one search returns in full.
const MAX_RESULTS: usize = 5;

/// Looks deferred tools up by what they do.
pub struct ToolSearch {
    specs: Vec<ToolSpec>,
}

impl ToolSearch {
    pub fn new(deferred: &ToolSet) -> Self {
        Self {
            specs: deferred.specs(),
        }
    }

    /// The tools matching `query`, best first: by how many of the query's
    /// words appear in the name or description, with a match in the name
    /// counting double.
    fn search(&self, query: &str) -> Vec<&ToolSpec> {
        let words: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric())
            .map(str::to_lowercase)
            // Words that are in every description say nothing.
            .filter(|word| word.len() > 1 && !FILLER.contains(&word.as_str()))
            .collect();
        let mut scored: Vec<(usize, &ToolSpec)> = self
            .specs
            .iter()
            .map(|spec| {
                let (name, description) =
                    (spec.name.to_lowercase(), spec.description.to_lowercase());
                let score = words
                    .iter()
                    .map(|word| {
                        2 * usize::from(name.contains(word))
                            + usize::from(description.contains(word))
                    })
                    .sum();
                (score, spec)
            })
            .filter(|(score, _)| *score > 0)
            .collect();
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        scored
            .into_iter()
            .map(|(_, spec)| spec)
            .take(MAX_RESULTS)
            .collect()
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

impl Tool for ToolSearch {
    fn spec(&self) -> ToolSpec {
        // The names are listed so the model knows what exists without
        // searching blind; the list is fixed for the session.
        let mut names = String::new();
        for spec in &self.specs {
            if names.len() > 3000 {
                names.push_str(", ...");
                break;
            }
            if !names.is_empty() {
                names.push_str(", ");
            }
            names.push_str(&spec.name);
        }
        ToolSpec {
            name: SEARCH_TOOL.into(),
            description: format!(
                "Find one of {} more tools by what it does, and get its description and input \
                 schema. Then run it with {CALL_TOOL}. The tools: {names}",
                self.specs.len()
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Words describing what you need, or a tool's name."}
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    fn host_side(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, input: &'a Value, _ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let query = input["query"].as_str().unwrap_or("");
            let found = self.search(query);
            if found.is_empty() {
                let all: Vec<String> = self
                    .specs
                    .iter()
                    .take(100)
                    .map(|spec| format!("{}: {}", spec.name, first_line(&spec.description)))
                    .collect();
                return ToolOutput::ok(format!(
                    "No tool matches {query:?}. All tools:\n{}",
                    all.join("\n")
                ));
            }
            let shown: Vec<String> = found
                .iter()
                .map(|spec| {
                    format!(
                        "{}\n{}\nInput schema: {}",
                        spec.name, spec.description, spec.input_schema
                    )
                })
                .collect();
            ToolOutput::ok(format!(
                "{}\n\nRun one with {CALL_TOOL}: {{\"name\": \"<tool name>\", \"input\": {{...}}}}",
                shown.join("\n\n")
            ))
        })
    }
}

/// Declares `tool_call` to the model. The agent runs the named tool in its
/// place, so this body only ever answers for a call it could not resolve.
pub struct ToolCall;

impl Tool for ToolCall {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: CALL_TOOL.into(),
            description: format!("Run a tool found with {SEARCH_TOOL}."),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "The tool's exact name."},
                    "input": {"type": "object", "description": "The tool's input, following its schema."}
                },
                "required": ["name", "input"]
            }),
        }
    }

    // Unknown until the named tool is resolved; assume the most.
    fn effect(&self) -> Effect {
        Effect::Execute
    }

    fn host_side(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, input: &'a Value, _ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            ToolOutput::error(format!(
                "There is no tool named {}. Find the exact name with {SEARCH_TOOL}.",
                input["name"]
            ))
        })
    }
}

/// Adds `tool_search` and `tool_call` for `deferred` to the declared tools.
pub fn declare_deferred(declared: &mut ToolSet, deferred: &ToolSet) {
    if !deferred.specs().is_empty() {
        declared.insert(Arc::new(ToolSearch::new(deferred)));
        declared.insert(Arc::new(ToolCall));
    }
}
