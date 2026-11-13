use std::path::Path;

/// Largest instruction file that is loaded whole; longer ones are cut.
const MAX_INSTRUCTION_BYTES: usize = 32 * 1024;

/// Standing instructions for the agent: the user's `$HX_HOME/AGENTS.md`,
/// then the project's `AGENTS.md` and `CLAUDE.md`. Returns text to append
/// to the system prompt, empty when there are none. Files that resolve to
/// the same file or hold the same text are included once.
pub fn load_instructions(home: &Path, cwd: &Path) -> String {
    let mut seen_paths = Vec::new();
    let mut seen_texts: Vec<String> = Vec::new();
    let mut out = String::new();
    for path in [
        home.join("AGENTS.md"),
        cwd.join("AGENTS.md"),
        cwd.join("CLAUDE.md"),
    ] {
        let Ok(real) = path.canonicalize() else {
            continue;
        };
        if seen_paths.contains(&real) {
            continue;
        }
        let Ok(mut text) = std::fs::read_to_string(&real) else {
            continue;
        };
        seen_paths.push(real);
        if text.len() > MAX_INSTRUCTION_BYTES {
            let mut end = MAX_INSTRUCTION_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push_str("\n[truncated]");
        }
        let text = text.trim().to_string();
        if text.is_empty() || seen_texts.contains(&text) {
            continue;
        }
        out.push_str(&format!(
            "\n\n# Instructions from {}\n\n{text}",
            path.display()
        ));
        seen_texts.push(text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn user_then_project_instructions_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let (home, cwd) = (dir.path().join("home"), dir.path().join("work"));
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        assert_eq!(load_instructions(&home, &cwd), "");

        fs::write(home.join("AGENTS.md"), "Reply in Vietnamese.\n").unwrap();
        fs::write(cwd.join("AGENTS.md"), "Run `make test` before finishing.\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(cwd.join("AGENTS.md"), cwd.join("CLAUDE.md")).unwrap();

        let text = load_instructions(&home, &cwd);
        let user = text.find("Reply in Vietnamese.").unwrap();
        let project = text.find("Run `make test` before finishing.").unwrap();
        assert!(user < project);
        assert_eq!(text.matches("make test").count(), 1, "{text}");
        assert_eq!(text.matches("# Instructions from").count(), 2);
    }
}
