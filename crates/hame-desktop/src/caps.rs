//! Which capability each desktop operation needs.
//!
//! The mapping is deliberately conservative: a control whose name says it
//! sends, saves, deletes or pays is a commit even when it is reached
//! through an ordinary press, because the name is all hame knows about
//! what pressing it does. Names come from the application and are
//! untrusted, but here they can only make an action need *more*
//! authority, never less.

use hame_session::contract::{Capability, Channel};

/// Words that mark a control as committing an effect that leaves the
/// machine or cannot be undone. Matched as whole words, ignoring case.
pub const COMMIT_WORDS: &[&str] = &[
    "save",
    "send",
    "delete",
    "submit",
    "purchase",
    "pay",
    "replace",
    "buy",
    "erase",
    "publish",
    "transfer",
    "trash",
    "overwrite",
    "discard",
    // A save panel opened by an export confirms with "Export".
    "export",
];

/// Whether a control's name marks it as a commit.
pub fn is_commit_name(name: &str) -> bool {
    name.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| COMMIT_WORDS.contains(&word))
}

/// Whether a control's title ends in an ellipsis, which by platform
/// convention means it asks for more input (opens a dialog) instead of
/// acting. The commit then happens in that dialog.
pub fn opens_dialog(name: &str) -> bool {
    let name = name.trim_end();
    name.ends_with('…') || name.ends_with("...")
}

/// What hame knows about the element an action is aimed at.
#[derive(Debug, Clone, Default)]
pub struct ElementFacts {
    pub name: String,
    pub role: String,
    /// The element sits inside an open or save panel that writes a file.
    pub save_panel: bool,
}

/// The channel an operation acts through.
pub fn channel_of(action: &str) -> Channel {
    match action {
        "screenshot" | "click_at" => Channel::Visual,
        _ => Channel::Desktop,
    }
}

/// Every capability `action` needs on its target. All of them must be
/// granted. `commit` is the caller's explicit declaration that the action
/// commits an effect, which only ever adds to the requirement.
pub fn required(action: &str, element: Option<&ElementFacts>, commit: bool) -> Vec<Capability> {
    let mut caps = match action {
        "apps" | "windows" | "observe" | "wait" => vec![Capability::Observe],
        "press" | "set_value" | "focus" | "select" | "menu" => vec![Capability::Interact],
        "type" | "key" | "click_at" => vec![Capability::GlobalInput],
        "clipboard" => vec![Capability::Clipboard],
        "screenshot" => vec![Capability::Screen],
        _ => vec![Capability::Interact],
    };
    if let Some(element) = element
        && matches!(action, "press" | "menu")
    {
        let commits = is_commit_name(&element.name) && !opens_dialog(&element.name);
        if commits {
            caps.push(Capability::Commit);
        }
        // Confirming a save panel writes a file wherever the panel points.
        if element.save_panel && commits {
            caps.push(Capability::FileWrite);
        }
    }
    if commit {
        caps.push(Capability::Commit);
    }
    caps.sort();
    caps.dedup();
    caps
}

/// The capability recorded on a receipt: the most consequential one.
pub fn strongest(caps: &[Capability]) -> Capability {
    const ORDER: [Capability; 7] = [
        Capability::Commit,
        Capability::FileWrite,
        Capability::GlobalInput,
        Capability::Clipboard,
        Capability::Screen,
        Capability::Interact,
        Capability::Observe,
    ];
    ORDER
        .into_iter()
        .find(|c| caps.contains(c))
        .unwrap_or(Capability::Observe)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(name: &str, save_panel: bool) -> ElementFacts {
        ElementFacts {
            name: name.into(),
            role: "AXButton".into(),
            save_panel,
        }
    }

    #[test]
    fn commit_names_are_whole_words() {
        for name in [
            "Save",
            "Save…",
            "Save As...",
            "Don't Save",
            "Send",
            "Delete",
            "Submit order",
            "Purchase",
            "Pay now",
            "Replace",
        ] {
            assert!(is_commit_name(name), "{name}");
        }
        for name in ["Display", "Paypal settings", "Cancel", "Sender", "OK", ""] {
            assert!(!is_commit_name(name), "{name}");
        }
    }

    #[test]
    fn capability_mapping_follows_the_operation_and_the_control() {
        use Capability::*;
        assert_eq!(required("observe", None, false), vec![Observe]);
        assert_eq!(required("windows", None, false), vec![Observe]);
        assert_eq!(required("apps", None, false), vec![Observe]);
        assert_eq!(
            required("press", Some(&el("Bold", false)), false),
            vec![Interact]
        );
        assert_eq!(
            required("press", Some(&el("Send", false)), false),
            vec![Interact, Commit]
        );
        assert_eq!(
            required("press", Some(&el("Save", true)), false),
            vec![Interact, Commit, FileWrite]
        );
        // Numbers and Pages confirm their export save panels with "Export".
        assert_eq!(
            required("press", Some(&el("Export", true)), false),
            vec![Interact, Commit, FileWrite]
        );
        assert_eq!(
            required("menu", Some(&el("Export as PDF…", false)), false),
            vec![Interact]
        );
        // Cancelling a save panel writes nothing.
        assert_eq!(
            required("press", Some(&el("Cancel", true)), false),
            vec![Interact]
        );
        // An ellipsis opens a dialog; the commit is the dialog's button.
        assert_eq!(
            required("menu", Some(&el("Save…", false)), false),
            vec![Interact]
        );
        assert_eq!(
            required("menu", Some(&el("Save As...", false)), false),
            vec![Interact]
        );
        assert_eq!(
            required("menu", Some(&el("Save", false)), false),
            vec![Interact, Commit]
        );
        assert_eq!(
            required("set_value", Some(&el("Save As:", true)), false),
            vec![Interact]
        );
        assert_eq!(required("set_value", None, true), vec![Interact, Commit]);
        assert_eq!(required("type", None, false), vec![GlobalInput]);
        assert_eq!(required("key", None, true), vec![Commit, GlobalInput]);
        assert_eq!(required("click_at", None, false), vec![GlobalInput]);
        assert_eq!(required("clipboard", None, false), vec![Clipboard]);
        assert_eq!(required("screenshot", None, false), vec![Screen]);
        assert_eq!(channel_of("click_at"), Channel::Visual);
        assert_eq!(channel_of("screenshot"), Channel::Visual);
        assert_eq!(channel_of("press"), Channel::Desktop);
        assert_eq!(strongest(&[Interact, Commit, FileWrite]), Commit);
        assert_eq!(strongest(&[GlobalInput]), GlobalInput);
    }
}
