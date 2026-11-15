//! Configurations to compare: each arm is a config.toml for the agent,
//! from a file or the base configuration, with `--set` overrides.

use std::collections::BTreeMap;

/// One configuration under test.
#[derive(Debug, Clone, PartialEq)]
pub struct Arm {
    pub name: String,
    /// The agent's config.toml.
    pub config: String,
    /// What was set on top of the base, by dotted key, for the record.
    pub overrides: BTreeMap<String, serde_json::Value>,
    /// The config file the arm started from, if any.
    pub source: Option<String>,
}

/// An override: `[ARM:]section.key=value`. The value is read as a TOML
/// value (`true`, `3`, `"text"`, `[1, 2]`), and as a plain string when it
/// is not one.
#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub arm: Option<String>,
    pub key: Vec<String>,
    pub value: toml::Value,
}

pub fn parse_setting(raw: &str) -> Result<Setting, String> {
    let (target, value) = raw
        .split_once('=')
        .ok_or_else(|| format!("--set {raw:?}: expected [ARM:]key=value"))?;
    let (arm, key) = match target.split_once(':') {
        Some((arm, key)) => (Some(arm.trim().to_string()), key),
        None => (None, target),
    };
    let key: Vec<String> = key
        .trim()
        .split('.')
        .map(|part| part.trim().to_string())
        .collect();
    if key.iter().any(String::is_empty) {
        return Err(format!("--set {raw:?}: empty key"));
    }
    let value = format!("v = {}", value.trim())
        .parse::<toml::Table>()
        .ok()
        .and_then(|mut table| table.remove("v"))
        .unwrap_or_else(|| toml::Value::String(value.trim().to_string()));
    Ok(Setting { arm, key, value })
}

fn apply(table: &mut toml::Table, key: &[String], value: toml::Value) -> Result<(), String> {
    let (last, path) = key.split_last().expect("non-empty key");
    let mut here = table;
    for part in path {
        here = here
            .entry(part.clone())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or_else(|| format!("{} is not a table", key.join(".")))?;
    }
    here.insert(last.clone(), value);
    Ok(())
}

fn to_json(value: &toml::Value) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

/// The arms to run. `arms` are `NAME` or `NAME=CONFIG.toml`; `read`
/// reads a config file. Settings without an arm apply to every arm. With
/// no arms there is one, `default`, made of the base configuration.
pub fn build_arms(
    base: &str,
    arms: &[String],
    settings: &[String],
    read: impl Fn(&str) -> Result<String, String>,
) -> Result<Vec<Arm>, String> {
    let settings: Vec<Setting> = settings
        .iter()
        .map(|raw| parse_setting(raw))
        .collect::<Result<_, _>>()?;
    let specs: Vec<(String, Option<String>)> = if arms.is_empty() {
        vec![("default".into(), None)]
    } else {
        arms.iter()
            .map(|raw| match raw.split_once('=') {
                Some((name, file)) => (name.trim().to_string(), Some(file.trim().to_string())),
                None => (raw.trim().to_string(), None),
            })
            .collect()
    };
    let mut names = std::collections::BTreeSet::new();
    for (name, _) in &specs {
        if name.is_empty() || !names.insert(name.clone()) {
            return Err(format!("arm names must be unique and non-empty: {name:?}"));
        }
    }
    for setting in &settings {
        if let Some(arm) = &setting.arm
            && !names.contains(arm)
        {
            return Err(format!(
                "--set names arm {arm:?}, which is not one of the --arm values"
            ));
        }
    }
    specs
        .into_iter()
        .map(|(name, file)| {
            let text = match &file {
                Some(file) => read(file)?,
                None => base.to_string(),
            };
            let mut table: toml::Table = text
                .parse()
                .map_err(|e| format!("arm {name}: invalid configuration: {e}"))?;
            let mut overrides = BTreeMap::new();
            for setting in settings
                .iter()
                .filter(|s| s.arm.as_deref().is_none_or(|arm| arm == name))
            {
                apply(&mut table, &setting.key, setting.value.clone())?;
                overrides.insert(setting.key.join("."), to_json(&setting.value));
            }
            Ok(Arm {
                config: if overrides.is_empty() {
                    text
                } else {
                    toml::to_string(&table).map_err(|e| e.to_string())?
                },
                name,
                overrides,
                source: file,
            })
        })
        .collect()
}

/// The order runs are made in: every run index, every task, every arm,
/// with the arm order rotated from one task and run to the next so no arm
/// always goes first (provider load drifts over time). Returns
/// `(run_index, task_index, arm_index)`, run indexes from 1.
pub fn interleave(runs: u32, tasks: usize, arms: usize) -> Vec<(u32, usize, usize)> {
    let mut order = Vec::new();
    for run in 1..=runs {
        for task in 0..tasks {
            for k in 0..arms {
                order.push((run, task, (k + task + run as usize - 1) % arms));
            }
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_parse_as_toml_values_with_an_optional_arm() {
        let s = parse_setting("lean:context.dedupe_results=false").unwrap();
        assert_eq!(s.arm.as_deref(), Some("lean"));
        assert_eq!(s.key, ["context", "dedupe_results"]);
        assert_eq!(s.value, toml::Value::Boolean(false));
        let s = parse_setting("model.name=claude-haiku-5-5").unwrap();
        assert_eq!(s.arm, None);
        assert_eq!(s.value, toml::Value::String("claude-haiku-5-5".into()));
        assert_eq!(
            parse_setting("limits.max_steps=40").unwrap().value,
            toml::Value::Integer(40)
        );
        assert!(parse_setting("nothing").is_err());
        assert!(parse_setting("a..b=1").is_err());
    }

    #[test]
    fn arms_combine_a_base_or_file_with_their_overrides() {
        let read = |file: &str| match file {
            "b.toml" => Ok("[model]\nedit_format = \"whole-file\"\n".to_string()),
            other => Err(format!("no {other}")),
        };
        let arms = build_arms(
            "[limits]\nmax_steps = 30\n",
            &["a".into(), "b=b.toml".into()],
            &[
                "a:context.scripts=false".into(),
                "limits.max_steps=20".into(),
            ],
            read,
        )
        .unwrap();
        let a: toml::Table = arms[0].config.parse().unwrap();
        assert_eq!(a["limits"]["max_steps"].as_integer(), Some(20));
        assert_eq!(a["context"]["scripts"].as_bool(), Some(false));
        assert_eq!(arms[0].overrides.len(), 2);
        let b: toml::Table = arms[1].config.parse().unwrap();
        assert_eq!(b["model"]["edit_format"].as_str(), Some("whole-file"));
        assert_eq!(b["limits"]["max_steps"].as_integer(), Some(20));
        assert!(b.get("context").is_none());
        assert_eq!(arms[1].source.as_deref(), Some("b.toml"));

        let default = build_arms("", &[], &[], read).unwrap();
        assert_eq!(default.len(), 1);
        assert_eq!(default[0].name, "default");
        assert!(build_arms("", &["a".into(), "a".into()], &[], read).is_err());
        assert!(build_arms("", &["a".into()], &["z:x=1".into()], read).is_err());
    }

    #[test]
    fn runs_are_interleaved_and_arm_order_rotates() {
        let order = interleave(2, 2, 2);
        assert_eq!(order.len(), 8);
        assert_eq!(
            order,
            [
                (1, 0, 0),
                (1, 0, 1),
                (1, 1, 1),
                (1, 1, 0),
                (2, 0, 1),
                (2, 0, 0),
                (2, 1, 0),
                (2, 1, 1)
            ]
        );
    }
}
