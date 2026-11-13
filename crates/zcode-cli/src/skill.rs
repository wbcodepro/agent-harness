//! `zcode skill`: install skills from elsewhere, after scanning them.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;
use zcode_ext::skill_install::{Severity, install};

#[derive(Subcommand)]
pub enum SkillCommand {
    /// Copy a skill into your skills directory after scanning it for
    /// hidden instructions and dangerous commands.
    Install {
        /// A directory holding a SKILL.md, or a git repository URL.
        source: String,
        /// Where the skill is inside the repository or directory.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Install even though the scan found something dangerous, and
        /// replace a skill of the same name.
        #[arg(long)]
        force: bool,
    },
    /// List the skills the agent can load in this directory.
    List,
}

pub fn run(home: &Path, command: SkillCommand) -> Result<ExitCode, String> {
    match command {
        SkillCommand::List => {
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            let skills = zcode_ext::skills::discover(&[
                cwd.join(".zcode/skills"),
                cwd.join(".claude/skills"),
                home.join("skills"),
            ]);
            for skill in skills {
                println!(
                    "{}\t{}\t{}",
                    skill.name,
                    skill.dir.display(),
                    skill.description
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        SkillCommand::Install {
            source,
            path,
            force,
        } => {
            // A local directory is used where it is; anything else is
            // fetched with git into a directory that is removed afterwards.
            let local = PathBuf::from(&source);
            let fetched = if local.is_dir() {
                None
            } else {
                let dir = std::env::temp_dir().join(format!("zcode-skill-{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&dir);
                let cloned = std::process::Command::new("git")
                    .args(["clone", "--quiet", "--depth", "1", "--"])
                    .arg(&source)
                    .arg(&dir)
                    .status()
                    .map_err(|e| format!("cannot run git: {e}"))?;
                if !cloned.success() {
                    return Err(format!(
                        "{source} is neither a directory nor a repository git can clone"
                    ));
                }
                Some(dir)
            };
            let base = fetched.clone().unwrap_or(local);
            let skill_dir = match &path {
                Some(path) => base.join(path),
                None => base,
            };
            let outcome = install(&skill_dir, &home.join("skills"), force);
            if let Some(dir) = fetched {
                let _ = std::fs::remove_dir_all(dir);
            }
            let (installed, findings) = outcome?;

            for finding in &findings {
                let place = match finding.line {
                    0 => finding.file.clone(),
                    line => format!("{}:{line}", finding.file),
                };
                let label = match finding.severity {
                    Severity::Danger => "DANGER",
                    Severity::Notice => "notice",
                };
                eprintln!("{label}  {place}  {}", finding.what);
            }
            match installed {
                Some(installed) => {
                    println!(
                        "installed {} in {}",
                        installed.name,
                        installed.dir.display()
                    );
                    Ok(ExitCode::SUCCESS)
                }
                None => {
                    eprintln!(
                        "zcode: not installed. Read the lines marked DANGER above; if you have \
                         checked them and still want the skill, run again with --force."
                    );
                    Ok(ExitCode::FAILURE)
                }
            }
        }
    }
}
