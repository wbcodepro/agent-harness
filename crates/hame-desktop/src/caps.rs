//! Which capability each desktop operation needs.
//!
//! The mapping is deliberately conservative: a control whose name says it
//! sends, saves, deletes or pays is a commit even when it is reached
//! through an ordinary press, because the name is all hame knows about
//! what pressing it does. Names come from the application and are
//! untrusted, but here they can only make an action need *more*
//! authority, never less.
//!
//! Physical input is classified the same way. A key, a newline in typed
//! text, or a click at a pixel can do whatever the control under it does:
//! Return presses a dialog's default button, Space presses the focused
//! button, and a click lands on whatever is drawn there. So `key`, `type`
//! and `click_at` look at where focus is (or what is under the point) at
//! dispatch time and need Commit, and FileWrite in a save panel, when the
//! equivalent `press` would. When that cannot be determined they need the
//! stricter set.

use hame_session::contract::{Capability, Channel};
use serde_json::{Value, json};

use crate::keys::KeyCombo;

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
        "press" | "set_value" | "focus" | "menu" => vec![Capability::Interact],
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

/// The capabilities an operation needs, with the reason for each one
/// beyond the operation's base capability, so a refusal can say why.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Need {
    pub caps: Vec<Capability>,
    pub reasons: Vec<(Capability, String)>,
}

impl Need {
    fn base(cap: Capability) -> Self {
        Self {
            caps: vec![cap],
            reasons: Vec::new(),
        }
    }

    fn add(&mut self, cap: Capability, why: impl Into<String>) {
        if !self.caps.contains(&cap) {
            self.caps.push(cap);
            self.caps.sort();
        }
        if !self.reasons.iter().any(|(c, _)| *c == cap) {
            self.reasons.push((cap, why.into()));
        }
    }

    /// Why `cap` is needed, if it is more than the operation's base.
    pub fn reason(&self, cap: Capability) -> Option<&str> {
        self.reasons
            .iter()
            .find(|(c, _)| *c == cap)
            .map(|(_, why)| why.as_str())
    }

    /// Whether the operation commits an effect, and so needs an
    /// idempotency key.
    pub fn commits(&self) -> bool {
        self.caps.contains(&Capability::Commit)
    }

    fn declared(mut self, commit: bool) -> Self {
        if commit {
            self.add(Capability::Commit, "the action was declared commit: true");
        }
        self
    }
}

/// The capability's name as grants spell it.
pub fn grant_name(cap: Capability) -> &'static str {
    match cap {
        Capability::Observe => "observe",
        Capability::Interact => "interact",
        Capability::Commit => "commit",
        Capability::FileWrite => "file_write",
        Capability::Clipboard => "clipboard",
        Capability::GlobalInput => "global_input",
        Capability::Screen => "screen",
    }
}

/// Where keyboard focus is when physical input is sent, as the helper
/// reports it right before dispatch.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FocusContext {
    /// The focused element's role, e.g. `AXTextArea`.
    pub role: String,
    pub subrole: String,
    pub name: String,
    /// The element's value, shortened; part of the idempotency key only.
    pub value: String,
    /// The focus is inside a sheet, a dialog, or an open or save panel.
    pub in_dialog: bool,
    /// The focus is inside a save panel (confirming it writes a file).
    pub save_panel: bool,
}

/// Roles that take text, where Space types a space instead of pressing.
const TEXT_INPUT_ROLES: &[&str] = &[
    "AXTextArea",
    "AXTextField",
    "AXSecureTextField",
    "AXComboBox",
    "AXSearchField",
];

impl FocusContext {
    /// Reads the helper's description; `None` when the helper could not
    /// tell what has focus.
    pub fn from_json(v: &Value) -> Option<Self> {
        if v["known"].as_bool() != Some(true) {
            return None;
        }
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        let role = s("role");
        if role.is_empty() {
            return None;
        }
        Some(Self {
            role,
            subrole: s("subrole"),
            name: s("name"),
            value: s("value"),
            // A missing answer is the stricter one.
            in_dialog: v["in_dialog"].as_bool().unwrap_or(true),
            save_panel: v["save_panel"].as_bool().unwrap_or(true),
        })
    }

    /// What the helper must still find at dispatch time for this
    /// classification to hold.
    pub fn expectation(&self) -> Value {
        json!({
            "role": self.role,
            "subrole": self.subrole,
            "in_dialog": self.in_dialog,
            "save_panel": self.save_panel,
        })
    }

    pub fn is_text_input(&self) -> bool {
        TEXT_INPUT_ROLES.contains(&self.role.as_str())
            || TEXT_INPUT_ROLES.contains(&self.subrole.as_str())
    }

    /// A multi-line text area in an ordinary window, where Return only
    /// inserts a line break.
    fn takes_line_breaks(&self) -> bool {
        self.role == "AXTextArea" && !self.in_dialog && !self.save_panel
    }

    /// The facts an idempotency key is made of.
    pub fn facts(&self) -> ElementFacts {
        ElementFacts {
            name: self.name.clone(),
            role: self.role.clone(),
            save_panel: self.save_panel,
        }
    }

    fn describe(&self) -> String {
        describe_element(&self.role, &self.name)
    }
}

fn describe_element(role: &str, name: &str) -> String {
    let role = role.trim_start_matches("AX").to_lowercase();
    if name.trim().is_empty() {
        format!("an unnamed {role}")
    } else {
        format!("{role} \"{}\"", name.chars().take(40).collect::<String>())
    }
}

/// What Return (a key, or a newline in typed text) may do with this focus.
fn return_needs(need: &mut Need, what: &str, focus: Option<&FocusContext>) {
    match focus {
        None => {
            let why = "when hame cannot tell what has keyboard focus (it may press a default \
                       button or confirm a save panel)";
            need.add(Capability::Commit, format!("{what} needs commit {why}"));
            need.add(
                Capability::FileWrite,
                format!("{what} needs file_write {why}"),
            );
        }
        Some(f) if f.save_panel => {
            need.add(
                Capability::Commit,
                format!("{what} in a save panel needs commit (it presses the default button)"),
            );
            need.add(
                Capability::FileWrite,
                format!("{what} in a save panel needs file_write (it can save a file)"),
            );
        }
        Some(f) if f.in_dialog => need.add(
            Capability::Commit,
            format!("{what} in a dialog or sheet needs commit (it presses the default button)"),
        ),
        Some(f) if f.takes_line_breaks() => {}
        Some(f) => need.add(
            Capability::Commit,
            format!(
                "{what} with focus on {} needs commit (outside a multi-line text area it can \
                 press a default button or submit)",
                f.describe()
            ),
        ),
    }
}

/// What Space (a key, or a space in typed text) may do with this focus.
fn space_needs(need: &mut Need, what: &str, focus: Option<&FocusContext>) {
    match focus {
        None => {
            let why = "when hame cannot tell what has keyboard focus (it presses the focused \
                       control)";
            need.add(Capability::Commit, format!("{what} needs commit {why}"));
            need.add(
                Capability::FileWrite,
                format!("{what} needs file_write {why}"),
            );
        }
        Some(f) if f.is_text_input() => {}
        Some(f) => {
            need.add(
                Capability::Commit,
                format!(
                    "{what} with focus on {} needs commit (it presses the focused control)",
                    f.describe()
                ),
            );
            if f.save_panel {
                need.add(
                    Capability::FileWrite,
                    format!("{what} on a control of a save panel needs file_write"),
                );
            }
        }
    }
}

/// Whether classifying `combo` depends on where focus is, so the helper
/// must be asked first.
pub fn key_needs_focus(combo: &KeyCombo) -> bool {
    let command = combo.has("cmd") || combo.has("ctrl");
    match combo.key.as_str() {
        "return" | "keypadenter" => true,
        "space" | "delete" | "forwarddelete" => !command,
        _ => false,
    }
}

/// Every capability a `key` needs. `focus` is where keyboard focus is; for
/// a key whose meaning depends on it (see [`key_needs_focus`]), `None`
/// means it could not be determined and the stricter set is needed.
///
/// | Key | Needs, besides GlobalInput |
/// |---|---|
/// | return, enter, keypad enter | Commit, unless focus is a text area outside any sheet, dialog or panel; plus FileWrite in a save panel or when focus is unknown. With cmd, ctrl or opt always Commit |
/// | space (no cmd/ctrl) | Commit unless focus is a text input; plus FileWrite on a save panel's control or when focus is unknown |
/// | delete, forward delete (no cmd/ctrl) | Commit unless focus is a text input |
/// | cmd+s, cmd+shift+s, cmd+opt+s (any cmd+…+s) | Commit and FileWrite |
/// | cmd+q, cmd+w, cmd+d, cmd+delete (with any further modifiers) | Commit |
/// | cmd+c, cmd+x, cmd+v (with any further modifiers) | Clipboard |
/// | anything else | nothing more |
pub fn key_needs(combo: &KeyCombo, focus: Option<&FocusContext>, commit: bool) -> Need {
    let mut need = Need::base(Capability::GlobalInput);
    let label = combo.label.as_str();
    let cmd = combo.has("cmd");
    let command = cmd || combo.has("ctrl");
    match combo.key.as_str() {
        "return" | "keypadenter" => {
            if command || combo.has("opt") {
                need.add(
                    Capability::Commit,
                    format!("{label} needs commit (it sends or confirms in many applications)"),
                );
            }
            return_needs(&mut need, label, focus);
        }
        "space" if !command => space_needs(&mut need, label, focus),
        "delete" | "forwarddelete" if !command => match focus {
            Some(f) if f.is_text_input() => {}
            Some(f) => need.add(
                Capability::Commit,
                format!(
                    "{label} with focus on {} needs commit (it deletes the selection)",
                    f.describe()
                ),
            ),
            None => need.add(
                Capability::Commit,
                format!(
                    "{label} needs commit when hame cannot tell what has keyboard focus (it can \
                     delete the selection)"
                ),
            ),
        },
        "s" if cmd => {
            need.add(
                Capability::Commit,
                format!("{label} needs commit (it saves the document)"),
            );
            need.add(
                Capability::FileWrite,
                format!("{label} needs file_write (it saves the document)"),
            );
        }
        "q" | "w" if cmd => need.add(
            Capability::Commit,
            format!("{label} needs commit (it quits or closes, which can save or discard changes)"),
        ),
        "d" if cmd => need.add(
            Capability::Commit,
            format!(
                "{label} needs commit (it chooses \"Don't Save\" or sends in some applications)"
            ),
        ),
        "delete" | "forwarddelete" if cmd => need.add(
            Capability::Commit,
            format!("{label} needs commit (it deletes or moves to the Trash)"),
        ),
        "c" | "x" | "v" if cmd => need.add(
            Capability::Clipboard,
            format!("{label} needs clipboard (it reads or writes the clipboard)"),
        ),
        _ => {}
    }
    need.declared(commit)
}

/// Characters that `type` sends as Return, or that an application may
/// take as Return or Enter: newlines and other control characters.
fn acts_as_return(c: char) -> bool {
    c != '\t' && (c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
}

/// Whether classifying `text` depends on where focus is.
pub fn type_needs_focus(text: &str) -> bool {
    text.chars().any(|c| acts_as_return(c) || c == ' ')
}

/// Every capability a `type` needs. A newline (`\n`, `\r`, or another
/// control character) counts as Return and a space as Space, with the
/// rules of [`key_needs`]: a line break in a text area outside any dialog,
/// or a space in any text input, needs nothing more than GlobalInput.
pub fn type_needs(text: &str, focus: Option<&FocusContext>, commit: bool) -> Need {
    let mut need = Need::base(Capability::GlobalInput);
    if text.chars().any(acts_as_return) {
        return_needs(&mut need, "a newline in typed text (sent as return)", focus);
    }
    if text.contains(' ') {
        space_needs(&mut need, "a space in typed text", focus);
    }
    need.declared(commit)
}

/// One element under a clicked point, as the helper's hit test reports it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HitElement {
    pub role: String,
    pub subrole: String,
    pub name: String,
}

/// What lies under a point that `click_at` would click.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HitFacts {
    /// From the element under the point up to (not including) its window,
    /// innermost first. Empty when nothing could be resolved.
    pub chain: Vec<HitElement>,
    /// The point is inside a sheet, a dialog, or an open or save panel.
    pub in_dialog: bool,
    /// The point is inside a save panel.
    pub save_panel: bool,
}

/// Roles a click activates.
const CLICKABLE_ROLES: &[&str] = &[
    "AXButton",
    "AXMenuItem",
    "AXMenuBarItem",
    "AXMenuButton",
    "AXPopUpButton",
    "AXCheckBox",
    "AXRadioButton",
    "AXLink",
    "AXDisclosureTriangle",
    "AXIncrementor",
    "AXSlider",
    "AXColorWell",
    "AXDockItem",
];

/// Roles where a single click only places the caret, selects or scrolls.
const INERT_ROLES: &[&str] = &[
    "AXTextArea",
    "AXTextField",
    "AXSecureTextField",
    "AXComboBox",
    "AXStaticText",
    "AXScrollArea",
    "AXTable",
    "AXOutline",
    "AXList",
    "AXRow",
    "AXCell",
    "AXColumn",
    "AXSplitGroup",
];

impl HitFacts {
    pub fn from_json(v: &Value) -> Self {
        let s = |e: &Value, k: &str| e[k].as_str().unwrap_or("").to_string();
        Self {
            chain: v["chain"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|e| HitElement {
                    role: s(e, "role"),
                    subrole: s(e, "subrole"),
                    name: s(e, "name"),
                })
                .filter(|e| !e.role.is_empty())
                .collect(),
            in_dialog: v["in_dialog"].as_bool().unwrap_or(true),
            save_panel: v["save_panel"].as_bool().unwrap_or(true),
        }
    }

    /// The control a click at this point operates: the innermost element,
    /// or an ancestor of it, that a click activates.
    pub fn control(&self) -> Option<&HitElement> {
        self.chain
            .iter()
            .find(|e| CLICKABLE_ROLES.contains(&e.role.as_str()))
    }

    /// The facts an idempotency key is made of.
    pub fn facts(&self) -> ElementFacts {
        let e = self.control().or(self.chain.first());
        ElementFacts {
            name: e.map(|e| e.name.clone()).unwrap_or_default(),
            role: e.map(|e| e.role.clone()).unwrap_or_default(),
            save_panel: self.save_panel,
        }
    }

    /// What the helper must still find under the point at dispatch time.
    pub fn expectation(&self) -> Value {
        match self.chain.first() {
            Some(e) => json!({"role": e.role, "name": e.name}),
            None => json!({"role": "", "name": ""}),
        }
    }
}

/// Every capability a `click_at` needs, from what the helper's hit test
/// found under the point (`None` when the hit test failed). A named
/// control is classified as `press` classifies it (commit words, the
/// confirming button of a save panel); anything that cannot be classified
/// needs Commit.
pub fn click_needs(hit: Option<&HitFacts>, count: u64, commit: bool) -> Need {
    let mut need = Need::base(Capability::GlobalInput);
    let unclassified = |need: &mut Need, what: &str, save_panel: bool| {
        need.add(
            Capability::Commit,
            format!("a click on {what} needs commit (hame cannot tell what it does)"),
        );
        if save_panel {
            need.add(
                Capability::FileWrite,
                format!("a click on {what} in a save panel needs file_write"),
            );
        }
    };
    let Some(hit) = hit.filter(|h| !h.chain.is_empty()) else {
        let save_panel = hit.is_none_or(|h| h.save_panel);
        unclassified(
            &mut need,
            "a point with no accessibility element",
            save_panel,
        );
        return need.declared(commit);
    };
    let in_web = hit.chain.iter().any(|e| e.role == "AXWebArea");
    match hit.control() {
        Some(control) if control.name.trim().is_empty() => {
            let mut what = describe_element(&control.role, "");
            if !control.subrole.is_empty() {
                what.push_str(&format!(" ({})", control.subrole));
            }
            unclassified(&mut need, &what, hit.save_panel);
        }
        Some(control) => {
            let facts = ElementFacts {
                name: control.name.clone(),
                role: control.role.clone(),
                save_panel: hit.save_panel,
            };
            let what = format!(
                "a click on {}",
                describe_element(&control.role, &control.name)
            );
            let pressed = required("press", Some(&facts), false);
            if pressed.contains(&Capability::Commit) {
                need.add(
                    Capability::Commit,
                    format!("{what} needs commit (its name says it commits)"),
                );
            }
            if pressed.contains(&Capability::FileWrite) {
                need.add(
                    Capability::FileWrite,
                    format!("{what} needs file_write (it confirms a save panel)"),
                );
            }
        }
        None if in_web => unclassified(
            &mut need,
            "web content that is not a named control",
            hit.save_panel,
        ),
        None => {
            let inner = &hit.chain[0];
            if !INERT_ROLES.contains(&inner.role.as_str()) {
                unclassified(
                    &mut need,
                    &describe_element(&inner.role, &inner.name),
                    hit.save_panel,
                );
            } else if count >= 2 && hit.in_dialog {
                need.add(
                    Capability::Commit,
                    "a double-click in a dialog or panel needs commit (it can confirm it)",
                );
                if hit.save_panel {
                    need.add(
                        Capability::FileWrite,
                        "a double-click in a save panel needs file_write (it can save a file)",
                    );
                }
            }
        }
    }
    need.declared(commit)
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

    fn focus(role: &str, in_dialog: bool, save_panel: bool) -> FocusContext {
        FocusContext {
            role: role.into(),
            name: "x".into(),
            in_dialog,
            save_panel,
            ..FocusContext::default()
        }
    }

    fn key(combo: &str, focus: Option<&FocusContext>) -> Vec<Capability> {
        key_needs(&crate::keys::parse(combo).unwrap(), focus, false).caps
    }

    #[test]
    fn return_is_a_commit_except_in_a_document_text_area() {
        use Capability::*;
        let doc = focus("AXTextArea", false, false);
        for combo in ["return", "enter", "shift+return", "kpenter"] {
            assert_eq!(key(combo, Some(&doc)), vec![GlobalInput], "{combo}");
        }
        // A default button, a single-line field, a text area in a sheet.
        assert_eq!(
            key("return", Some(&focus("AXButton", true, false))),
            vec![Commit, GlobalInput]
        );
        assert_eq!(
            key("return", Some(&focus("AXTextField", false, false))),
            vec![Commit, GlobalInput]
        );
        assert_eq!(
            key("return", Some(&focus("AXTextArea", true, false))),
            vec![Commit, GlobalInput]
        );
        // In a save panel (the name field, or the Go To Folder sheet on it)
        // Return can save: FileWrite too.
        let need = key_needs(
            &crate::keys::parse("return").unwrap(),
            Some(&focus("AXTextField", true, true)),
            false,
        );
        assert_eq!(need.caps, vec![Commit, FileWrite, GlobalInput]);
        assert!(
            need.reason(FileWrite)
                .unwrap()
                .contains("save panel needs file_write"),
            "{need:?}"
        );
        assert!(need.commits());
        // Unknown focus: the stricter set.
        assert_eq!(key("return", None), vec![Commit, FileWrite, GlobalInput]);
        // cmd+return sends in many apps, even from a text area.
        assert_eq!(key("cmd+return", Some(&doc)), vec![Commit, GlobalInput]);
        assert!(key_needs_focus(&crate::keys::parse("return").unwrap()));
        assert!(!key_needs_focus(&crate::keys::parse("cmd+s").unwrap()));
    }

    #[test]
    fn space_and_delete_are_commits_unless_focus_takes_text() {
        use Capability::*;
        for role in [
            "AXTextArea",
            "AXTextField",
            "AXComboBox",
            "AXSecureTextField",
        ] {
            assert_eq!(
                key("space", Some(&focus(role, true, true))),
                vec![GlobalInput],
                "{role}"
            );
            assert_eq!(
                key("delete", Some(&focus(role, true, false))),
                vec![GlobalInput]
            );
        }
        let search = FocusContext {
            subrole: "AXSearchField".into(),
            ..focus("AXTextField", false, false)
        };
        assert!(search.is_text_input());
        assert_eq!(
            key("space", Some(&focus("AXButton", false, false))),
            vec![Commit, GlobalInput]
        );
        assert_eq!(
            key("space", Some(&focus("AXButton", true, true))),
            vec![Commit, FileWrite, GlobalInput]
        );
        assert_eq!(key("space", None), vec![Commit, FileWrite, GlobalInput]);
        assert_eq!(
            key("delete", Some(&focus("AXRow", false, false))),
            vec![Commit, GlobalInput]
        );
        assert_eq!(key("forwarddelete", None), vec![Commit, GlobalInput]);
    }

    #[test]
    fn command_keys_follow_what_they_do() {
        use Capability::*;
        for combo in ["cmd+s", "cmd+shift+s", "cmd+opt+s", "cmd+opt+shift+s"] {
            assert_eq!(
                key(combo, None),
                vec![Commit, FileWrite, GlobalInput],
                "{combo}"
            );
        }
        for combo in [
            "cmd+q",
            "cmd+w",
            "cmd+opt+w",
            "cmd+d",
            "cmd+shift+d",
            "cmd+delete",
            "cmd+backspace",
            "cmd+forwarddelete",
        ] {
            assert_eq!(key(combo, None), vec![Commit, GlobalInput], "{combo}");
        }
        for combo in ["cmd+c", "cmd+x", "cmd+v", "cmd+opt+shift+v"] {
            assert_eq!(key(combo, None), vec![Clipboard, GlobalInput], "{combo}");
        }
        for combo in [
            "cmd+shift+g",
            "cmd+a",
            "tab",
            "escape",
            "cmd+z",
            "left",
            "a",
            "cmd+space",
        ] {
            assert_eq!(key(combo, None), vec![GlobalInput], "{combo}");
        }
        // A declared commit only adds.
        let need = key_needs(&crate::keys::parse("tab").unwrap(), None, true);
        assert_eq!(need.caps, vec![Commit, GlobalInput]);
        assert!(need.reason(Commit).unwrap().contains("commit: true"));
        assert_eq!(
            key_needs(&crate::keys::parse("cmd+s").unwrap(), None, false).reason(GlobalInput),
            None
        );
    }

    #[test]
    fn newlines_and_spaces_in_typed_text_follow_the_key_rules() {
        use Capability::*;
        let doc = focus("AXTextArea", false, false);
        let field = focus("AXTextField", false, false);
        assert!(!type_needs_focus("plain-text"));
        assert!(type_needs_focus("a b") && type_needs_focus("a\nb"));
        assert_eq!(
            type_needs("plain-text", None, false).caps,
            vec![GlobalInput]
        );
        // A document's text area takes lines and spaces.
        assert_eq!(
            type_needs("Agenda\n1. Budget review", Some(&doc), false).caps,
            vec![GlobalInput]
        );
        // A single-line field: a newline is Return there.
        let need = type_needs("a\nb", Some(&field), false);
        assert_eq!(need.caps, vec![Commit, GlobalInput]);
        assert!(need.reason(Commit).unwrap().contains("newline"), "{need:?}");
        assert_eq!(
            type_needs("a b", Some(&field), false).caps,
            vec![GlobalInput]
        );
        for text in ["a\rb", "a\r\nb", "x\u{3}", "x\u{2028}y"] {
            assert_eq!(
                type_needs(text, Some(&field), false).caps,
                vec![Commit, GlobalInput],
                "{text:?}"
            );
        }
        // A tab only moves focus.
        assert_eq!(
            type_needs("a\tb", Some(&field), false).caps,
            vec![GlobalInput]
        );
        // A space with a button focused presses it.
        assert_eq!(
            type_needs("ok go", Some(&focus("AXButton", false, false)), false).caps,
            vec![Commit, GlobalInput]
        );
        // In a save panel's name field, a newline saves.
        assert_eq!(
            type_needs("notes\n", Some(&focus("AXTextField", true, true)), false).caps,
            vec![Commit, FileWrite, GlobalInput]
        );
        assert_eq!(
            type_needs("a\nb", None, false).caps,
            vec![Commit, FileWrite, GlobalInput]
        );
    }

    fn hit(chain: &[(&str, &str)], in_dialog: bool, save_panel: bool) -> HitFacts {
        HitFacts {
            chain: chain
                .iter()
                .map(|(role, name)| HitElement {
                    role: (*role).into(),
                    name: (*name).into(),
                    ..HitElement::default()
                })
                .collect(),
            in_dialog,
            save_panel,
        }
    }

    #[test]
    fn a_click_is_classified_by_the_control_under_the_point() {
        use Capability::*;
        let click = |h: &HitFacts, count| click_needs(Some(h), count, false).caps;
        // The label inside a button: the button decides.
        let send = hit(
            &[("AXStaticText", "Send"), ("AXButton", "Send")],
            false,
            false,
        );
        assert_eq!(click(&send, 1), vec![Commit, GlobalInput]);
        assert_eq!(
            send.facts().role,
            "AXButton",
            "the key is made of the control"
        );
        let save = hit(&[("AXButton", "Save")], true, true);
        assert_eq!(click(&save, 1), vec![Commit, FileWrite, GlobalInput]);
        let bold = hit(&[("AXCheckBox", "Bold"), ("AXGroup", "")], false, false);
        assert_eq!(click(&bold, 1), vec![GlobalInput]);
        let cancel = hit(&[("AXButton", "Cancel")], true, true);
        assert_eq!(click(&cancel, 1), vec![GlobalInput]);
        // Placing the caret in text, selecting a row.
        let text = hit(&[("AXTextArea", ""), ("AXScrollArea", "")], false, false);
        assert_eq!(click(&text, 1), vec![GlobalInput]);
        let row = hit(
            &[("AXCell", ""), ("AXRow", ""), ("AXOutline", "")],
            true,
            true,
        );
        assert_eq!(click(&row, 1), vec![GlobalInput]);
        // A double-click in a panel can confirm it.
        assert_eq!(click(&row, 2), vec![Commit, FileWrite, GlobalInput]);
        // Not classifiable: nothing under the point, an unnamed button, a
        // generic group, web content that is not a named control.
        let need = click_needs(Some(&hit(&[], false, false)), 1, false);
        assert_eq!(need.caps, vec![Commit, GlobalInput]);
        assert!(need.reason(Commit).unwrap().contains("cannot tell"));
        assert_eq!(
            click_needs(None, 1, false).caps,
            vec![Commit, FileWrite, GlobalInput]
        );
        assert_eq!(
            click(&hit(&[("AXButton", " ")], false, false), 1),
            vec![Commit, GlobalInput]
        );
        assert_eq!(
            click(&hit(&[("AXGroup", "")], false, false), 1),
            vec![Commit, GlobalInput]
        );
        assert_eq!(
            click(
                &hit(
                    &[("AXStaticText", "Pay"), ("AXGroup", ""), ("AXWebArea", "")],
                    false,
                    false
                ),
                1
            ),
            vec![Commit, GlobalInput]
        );
        assert_eq!(
            click(
                &hit(&[("AXLink", "Docs"), ("AXWebArea", "")], false, false),
                1
            ),
            vec![GlobalInput]
        );
        assert_eq!(
            hit(&[("AXImage", "")], false, false).expectation(),
            json!({"role": "AXImage", "name": ""})
        );
    }

    #[test]
    fn the_helpers_answers_are_read_strictly() {
        assert_eq!(FocusContext::from_json(&json!({"known": false})), None);
        assert_eq!(
            FocusContext::from_json(&json!({"known": true, "role": ""})),
            None
        );
        // A missing surroundings answer is read as the stricter one.
        let f = FocusContext::from_json(&json!({"known": true, "role": "AXTextArea"})).unwrap();
        assert!(f.in_dialog && f.save_panel);
        let f = FocusContext::from_json(&json!({
            "known": true, "role": "AXTextArea", "subrole": "", "name": "",
            "value": "x", "in_dialog": false, "save_panel": false
        }))
        .unwrap();
        assert_eq!(
            f.expectation(),
            json!({"role": "AXTextArea", "subrole": "", "in_dialog": false, "save_panel": false})
        );
        let h = HitFacts::from_json(
            &json!({"chain": [{"role": "AXButton", "name": "Send"}, {"name": "no role"}]}),
        );
        assert_eq!(h.chain.len(), 1);
        assert!(h.in_dialog && h.save_panel);
    }
}
