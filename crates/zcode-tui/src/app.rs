use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as Panel, Borders, Paragraph};
use unicode_width::UnicodeWidthChar;
use zcode_session::{Block, Delta, Event, Record, TurnEndReason};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    User,
    Assistant,
    Tool,
    ToolOk,
    ToolError,
    Notice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    kind: Kind,
    text: String,
}

/// Everything the interface shows.
#[derive(Debug, Default)]
pub struct App {
    entries: Vec<Entry>,
    /// Whether the last assistant entry is still receiving deltas.
    streaming: bool,
    pub input: String,
    /// A turn is running.
    pub busy: bool,
    /// An approval question awaiting y or n.
    pub approval: Option<String>,
    pub plan_mode: bool,
    /// Short facts for the status bar: session, sandbox, model.
    pub status: String,
    /// Lines scrolled up from the bottom of the transcript.
    pub scroll: usize,
    context_tokens: u64,
}

fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("");
    let mut shown: String = line.chars().take(max).collect();
    if shown.len() < text.trim_end().len() {
        shown.push_str(" …");
    }
    shown
}

/// Breaks `text` into rows at most `width` columns wide, honouring newlines
/// and wide characters.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for line in text.split('\n') {
        let mut row = String::new();
        let mut used = 0;
        for ch in line.chars() {
            let w = ch.width().unwrap_or(0);
            if used + w > width {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            row.push(ch);
            used += w;
        }
        rows.push(row);
    }
    rows
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, kind: Kind, text: impl Into<String>) {
        self.entries.push(Entry {
            kind,
            text: text.into(),
        });
        self.streaming = false;
    }

    /// A line from the interface itself rather than the session.
    pub fn notice(&mut self, text: impl Into<String>) {
        self.push(Kind::Notice, text);
    }

    /// Folds in a record appended while a turn runs.
    pub fn apply(&mut self, record: &Record) {
        match &record.event {
            Event::AssistantDelta {
                delta: Delta::Text { text },
            } => {
                if self.streaming {
                    if let Some(last) = self.entries.last_mut() {
                        last.text.push_str(text);
                    }
                } else {
                    self.push(Kind::Assistant, text.clone());
                    self.streaming = true;
                }
            }
            Event::AssistantDelta { .. } => {}
            Event::AssistantMessage { usage, .. } => {
                self.streaming = false;
                self.context_tokens = usage.input_tokens
                    + usage.cache_read_tokens
                    + usage.cache_write_tokens
                    + usage.output_tokens;
            }
            other => self.apply_settled(other),
        }
    }

    /// Empties the transcript, for switching to another session.
    pub fn reset(&mut self) {
        self.entries.clear();
        self.streaming = false;
        self.context_tokens = 0;
    }

    /// Folds in a record of a stored session being replayed: messages
    /// arrive whole, so the deltas that once streamed them are skipped.
    pub fn apply_replayed(&mut self, record: &Record) {
        match &record.event {
            Event::AssistantDelta { .. } => {}
            Event::AssistantMessage { content, usage, .. } => {
                for block in content {
                    if let Block::Text { text } = block {
                        self.push(Kind::Assistant, text.clone());
                    }
                }
                self.context_tokens = usage.input_tokens
                    + usage.cache_read_tokens
                    + usage.cache_write_tokens
                    + usage.output_tokens;
            }
            other => self.apply_settled(other),
        }
    }

    /// Rebuilds the transcript from a stored session.
    pub fn load(&mut self, records: &[Record]) {
        self.reset();
        for record in records {
            self.apply_replayed(record);
        }
    }

    fn apply_settled(&mut self, event: &Event) {
        match event {
            Event::UserMessage { content } => {
                let text = match content.first() {
                    Some(Block::Text { text }) => text.as_str(),
                    _ => "",
                };
                if text.starts_with("[zcode verify]") {
                    self.push(Kind::Notice, "checks failed; sent back to the model");
                } else {
                    self.push(Kind::User, text);
                }
            }
            Event::AssistantAttempt { .. } => {
                if self.streaming
                    && let Some(last) = self.entries.last_mut()
                {
                    last.text.push_str(" [interrupted]");
                }
                self.streaming = false;
            }
            Event::ToolCall { name, input, .. } => {
                let subject = input["command"]
                    .as_str()
                    .or_else(|| input["path"].as_str())
                    .or_else(|| input["pattern"].as_str())
                    .map(String::from)
                    .unwrap_or_else(|| input.to_string());
                self.push(Kind::Tool, format!("{name} {}", first_line(&subject, 200)));
            }
            Event::ToolResult {
                content, is_error, ..
            } => {
                let text = match content.first() {
                    Some(Block::Text { text }) => text.as_str(),
                    _ => "",
                };
                let kind = if *is_error {
                    Kind::ToolError
                } else {
                    Kind::ToolOk
                };
                self.push(kind, first_line(text, 200));
            }
            Event::Compaction { .. } => self.push(Kind::Notice, "earlier history summarized"),
            Event::TurnEnd { reason, .. } => match reason {
                TurnEndReason::Completed => {}
                TurnEndReason::Cancelled => self.push(Kind::Notice, "cancelled"),
                TurnEndReason::Failed => self.push(Kind::Notice, "the turn failed"),
                TurnEndReason::Interrupted => self.push(Kind::Notice, "interrupted by a crash"),
            },
            _ => {}
        }
    }

    fn transcript_rows(&self, width: usize) -> Vec<Line<'static>> {
        let mut rows = Vec::new();
        for entry in &self.entries {
            let (prefix, style) = match entry.kind {
                Kind::User => (
                    "› ",
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Kind::Assistant => ("", Style::new()),
                Kind::Tool => ("  ▸ ", Style::new().fg(Color::Yellow)),
                Kind::ToolOk => ("    ✓ ", Style::new().fg(Color::DarkGray)),
                Kind::ToolError => ("    ✗ ", Style::new().fg(Color::Red)),
                Kind::Notice => ("· ", Style::new().fg(Color::Magenta)),
            };
            if matches!(entry.kind, Kind::User | Kind::Assistant) && !rows.is_empty() {
                rows.push(Line::default());
            }
            let indent = " ".repeat(prefix.chars().count());
            let wrapped = wrap(&entry.text, width.saturating_sub(indent.len()));
            for (index, row) in wrapped.into_iter().enumerate() {
                let lead = if index == 0 {
                    prefix.to_string()
                } else {
                    indent.clone()
                };
                rows.push(Line::from(Span::styled(lead + &row, style)));
            }
        }
        rows
    }

    pub fn render(&self, frame: &mut Frame) {
        let [transcript, input, status] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        let rows = self.transcript_rows(transcript.width as usize);
        let height = transcript.height as usize;
        let end = rows
            .len()
            .saturating_sub(self.scroll.min(rows.len().saturating_sub(height)));
        let start = end.saturating_sub(height);
        frame.render_widget(Paragraph::new(rows[start..end].to_vec()), transcript);

        let (title, body, style) = match &self.approval {
            Some(question) => (
                " approve? y / n ",
                question.clone(),
                Style::new().fg(Color::Yellow),
            ),
            None if self.busy => (
                " working — Esc to cancel ",
                self.input.clone(),
                Style::new().fg(Color::DarkGray),
            ),
            None => (
                " message — Enter to send, /help for commands ",
                self.input.clone(),
                Style::new(),
            ),
        };
        // Show the end of a long input rather than its start.
        let room = input.width.saturating_sub(4) as usize;
        let chars: Vec<char> = body.chars().collect();
        let shown: String = chars[chars.len().saturating_sub(room)..].iter().collect();
        frame.render_widget(
            Paragraph::new(format!("› {shown}"))
                .style(style)
                .block(Panel::new().borders(Borders::ALL).title(title)),
            input,
        );

        let mode = if self.plan_mode { "plan" } else { "act" };
        let tokens = if self.context_tokens > 0 {
            format!(" · context {} tok", self.context_tokens)
        } else {
            String::new()
        };
        frame.render_widget(
            Paragraph::new(format!(" {mode} · {}{tokens}", self.status))
                .style(Style::new().fg(Color::DarkGray)),
            status,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use serde_json::json;
    use zcode_session::{StopReason, Usage};

    fn record(event: Event) -> Record {
        Record {
            seq: 0,
            ts: 0,
            event,
        }
    }

    fn screen(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..height {
            for x in 0..width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn text_delta(text: &str) -> Record {
        record(Event::AssistantDelta {
            delta: Delta::Text { text: text.into() },
        })
    }

    #[test]
    fn a_turn_renders_as_it_streams() {
        let mut app = App::new();
        app.status = "s1 · workspace-write".into();
        app.apply(&record(Event::UserMessage {
            content: vec![Block::text("List files")],
        }));
        app.apply(&text_delta("Let me "));
        app.apply(&text_delta("look."));
        app.apply(&record(Event::ToolCall {
            call_id: "c1".into(),
            name: "bash".into(),
            input: json!({"command": "ls -la"}),
        }));
        app.apply(&record(Event::ToolResult {
            call_id: "c1".into(),
            content: vec![Block::text("a.rs\nb.rs")],
            is_error: false,
        }));
        app.apply(&record(Event::AssistantMessage {
            content: vec![],
            stop: StopReason::ToolUse,
            usage: Usage {
                input_tokens: 1200,
                output_tokens: 34,
                ..Usage::default()
            },
        }));
        app.apply(&text_delta("Two files."));

        let screen = screen(&app, 60, 14);
        assert!(screen.contains("› List files"), "{screen}");
        assert!(screen.contains("Let me look."));
        assert!(screen.contains("▸ bash ls -la"));
        assert!(screen.contains("✓ a.rs …"));
        assert!(screen.contains("Two files."));
        assert!(screen.contains("act · s1 · workspace-write · context 1234 tok"));
    }

    #[test]
    fn loading_a_session_shows_messages_once_and_wraps_long_lines() {
        let mut app = App::new();
        let records = vec![
            record(Event::UserMessage {
                content: vec![Block::text("Hi")],
            }),
            text_delta("ignored when loading"),
            record(Event::AssistantMessage {
                content: vec![Block::text("word ".repeat(30).trim().to_string())],
                stop: StopReason::EndTurn,
                usage: Usage::default(),
            }),
            record(Event::TurnEnd {
                turn: 1,
                reason: TurnEndReason::Cancelled,
            }),
        ];
        app.load(&records);
        let screen = screen(&app, 40, 16);
        assert!(!screen.contains("ignored"));
        assert_eq!(screen.matches("word").count(), 30, "{screen}");
        assert!(screen.contains("· cancelled"));

        assert_eq!(wrap("ab\ncdefg", 3), ["ab", "cde", "fg"]);
        assert_eq!(wrap("日本語", 4), ["日本", "語"]);
    }

    #[test]
    fn the_input_box_reflects_approval_and_busy_states() {
        let mut app = App::new();
        app.input = "hello".into();
        assert!(screen(&app, 60, 8).contains("› hello"));
        app.busy = true;
        assert!(screen(&app, 60, 8).contains("Esc to cancel"));
        app.approval = Some("Allow bash rm -rf build?".into());
        let screen = screen(&app, 60, 8);
        assert!(screen.contains("approve? y / n") && screen.contains("Allow bash rm -rf build?"));
    }
}
