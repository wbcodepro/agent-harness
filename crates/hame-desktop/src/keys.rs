//! Key combinations such as `cmd+s` or `cmd+shift+g`, parsed into macOS
//! virtual key codes (ANSI layout) and modifier names.

/// A parsed key combination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCombo {
    pub keycode: u16,
    /// `cmd`, `shift`, `opt`, `ctrl`, `fn`, in that order.
    pub modifiers: Vec<&'static str>,
    /// The canonical spelling, e.g. `cmd+shift+g`.
    pub label: String,
    /// The canonical name of the non-modifier key, e.g. `g` or `return`.
    pub key: String,
}

impl KeyCombo {
    pub fn has(&self, modifier: &str) -> bool {
        self.modifiers.contains(&modifier)
    }
}

const MODIFIERS: &[(&str, &[&str])] = &[
    ("cmd", &["cmd", "command", "⌘", "meta", "super"]),
    ("shift", &["shift", "⇧"]),
    ("opt", &["opt", "option", "alt", "⌥"]),
    ("ctrl", &["ctrl", "control", "⌃"]),
    ("fn", &["fn"]),
];

/// Virtual key codes from HIToolbox `Events.h` (ANSI layout).
const KEYS: &[(&str, u16)] = &[
    ("a", 0),
    ("s", 1),
    ("d", 2),
    ("f", 3),
    ("h", 4),
    ("g", 5),
    ("z", 6),
    ("x", 7),
    ("c", 8),
    ("v", 9),
    ("b", 11),
    ("q", 12),
    ("w", 13),
    ("e", 14),
    ("r", 15),
    ("y", 16),
    ("t", 17),
    ("1", 18),
    ("2", 19),
    ("3", 20),
    ("4", 21),
    ("6", 22),
    ("5", 23),
    ("=", 24),
    ("9", 25),
    ("7", 26),
    ("-", 27),
    ("8", 28),
    ("0", 29),
    ("]", 30),
    ("o", 31),
    ("u", 32),
    ("[", 33),
    ("i", 34),
    ("p", 35),
    ("return", 36),
    ("l", 37),
    ("j", 38),
    ("'", 39),
    ("k", 40),
    (";", 41),
    ("\\", 42),
    (",", 43),
    ("/", 44),
    ("n", 45),
    ("m", 46),
    (".", 47),
    ("tab", 48),
    ("space", 49),
    ("`", 50),
    ("delete", 51),
    ("escape", 53),
    ("keypadenter", 76),
    ("f17", 64),
    ("f18", 79),
    ("f19", 80),
    ("f20", 90),
    ("f5", 96),
    ("f6", 97),
    ("f7", 98),
    ("f3", 99),
    ("f8", 100),
    ("f9", 101),
    ("f11", 103),
    ("f13", 105),
    ("f16", 106),
    ("f14", 107),
    ("f10", 109),
    ("f12", 111),
    ("f15", 113),
    ("help", 114),
    ("home", 115),
    ("pageup", 116),
    ("forwarddelete", 117),
    ("f4", 118),
    ("end", 119),
    ("f2", 120),
    ("pagedown", 121),
    ("f1", 122),
    ("left", 123),
    ("right", 124),
    ("down", 125),
    ("up", 126),
];

const ALIASES: &[(&str, &str)] = &[
    ("enter", "return"),
    ("kpenter", "keypadenter"),
    ("numpadenter", "keypadenter"),
    ("ret", "return"),
    ("esc", "escape"),
    ("backspace", "delete"),
    ("del", "forwarddelete"),
    ("pgup", "pageup"),
    ("pgdn", "pagedown"),
    ("arrowleft", "left"),
    ("arrowright", "right"),
    ("arrowup", "up"),
    ("arrowdown", "down"),
    ("plus", "="),
    ("minus", "-"),
    ("comma", ","),
    ("period", "."),
    ("slash", "/"),
];

/// Parses `cmd+shift+g`, `Return`, `ctrl+alt+delete`. Modifiers may come
/// in any order; exactly one non-modifier key is required.
pub fn parse(combo: &str) -> Result<KeyCombo, String> {
    let combo = combo.trim();
    if combo.is_empty() {
        return Err("keys is empty; use a combination such as cmd+s or return.".into());
    }
    // `+` itself is spelled `plus`; split on `+` otherwise.
    let parts: Vec<String> = combo
        .split('+')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    let mut mods: Vec<&'static str> = Vec::new();
    let mut key: Option<(String, u16)> = None;
    for part in &parts {
        if let Some((canonical, _)) = MODIFIERS
            .iter()
            .find(|(_, names)| names.contains(&part.as_str()))
        {
            if !mods.contains(canonical) {
                mods.push(canonical);
            }
            continue;
        }
        let name = ALIASES
            .iter()
            .find(|(alias, _)| alias == part)
            .map(|(_, real)| real.to_string())
            .unwrap_or_else(|| part.clone());
        let Some((_, code)) = KEYS.iter().find(|(k, _)| *k == name) else {
            return Err(format!(
                "Unknown key {part:?} in {combo:?}. Use letters, digits, punctuation, \
                 return, keypadenter, tab, space, escape, delete, arrows, home/end, pageup/pagedown or f1-f20."
            ));
        };
        if key.is_some() {
            return Err(format!(
                "{combo:?} names more than one key; press them separately."
            ));
        }
        key = Some((name, *code));
    }
    let Some((name, keycode)) = key else {
        return Err(format!("{combo:?} has only modifiers; add a key."));
    };
    let order = |m: &&str| MODIFIERS.iter().position(|(c, _)| c == m).unwrap_or(9);
    mods.sort_by_key(order);
    let mut label: Vec<&str> = mods.clone();
    label.push(&name);
    Ok(KeyCombo {
        keycode,
        label: label.join("+"),
        modifiers: mods,
        key: name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_parse_to_key_codes_and_ordered_modifiers() {
        let s = parse("cmd+s").unwrap();
        assert_eq!((s.keycode, s.modifiers.clone()), (1, vec!["cmd"]));
        let g = parse("Shift+Command+G").unwrap();
        assert_eq!(g.keycode, 5);
        assert_eq!(g.modifiers, vec!["cmd", "shift"]);
        assert_eq!(g.label, "cmd+shift+g");
        assert_eq!(parse("Return").unwrap().keycode, 36);
        assert_eq!(parse("enter").unwrap().keycode, 36);
        assert_eq!(parse("enter").unwrap().key, "return");
        assert_eq!(parse("kpenter").unwrap().keycode, 76);
        assert!(parse("cmd+shift+s").unwrap().has("shift"));
        assert_eq!(parse("esc").unwrap().keycode, 53);
        assert_eq!(parse("alt+left").unwrap().modifiers, vec!["opt"]);
        assert_eq!(parse("cmd+plus").unwrap().keycode, 24);
        assert_eq!(parse("f12").unwrap().keycode, 111);
        assert!(parse("cmd+shift").is_err());
        assert!(parse("cmd+a+b").is_err());
        assert!(parse("hyper+x").is_err());
        assert!(parse("").is_err());
    }
}
