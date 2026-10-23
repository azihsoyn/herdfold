//! The agent skill that comes with herdfold, and putting it where an agent
//! finds it, as herdr's `--skill` and `integration` commands do for herdr.

use std::path::PathBuf;

use crate::cli::CliError;

/// The skill, as it is printed by `herdfold --skill`.
pub const SKILL: &str = include_str!("../skills/herdfold/SKILL.md");

/// The line an installed skill carries after its front matter, naming the
/// herdfold that installed it.
const MARK: &str = "<!-- installed by herdfold ";

/// Agents the skill can be installed for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Target {
    Claude,
}

impl Target {
    fn name(self) -> &'static str {
        match self {
            Target::Claude => "claude",
        }
    }

    /// Where the agent looks for it: `$CLAUDE_CONFIG_DIR/skills/herdfold/SKILL.md`
    /// (default `~/.claude/skills/herdfold/SKILL.md`).
    fn path(self) -> Option<PathBuf> {
        match self {
            Target::Claude => {
                let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude"))
                    })?;
                Some(dir.join("skills").join(crate::NAME).join("SKILL.md"))
            }
        }
    }
}

/// The skill as it is installed: marked with this herdfold's version.
fn installed() -> String {
    mark(SKILL, env!("CARGO_PKG_VERSION"))
}

fn mark(skill: &str, version: &str) -> String {
    let line = format!("{MARK}{version} -->\n");
    // After the front matter, which must come first.
    match skill
        .strip_prefix("---\n")
        .and_then(|rest| rest.find("\n---\n"))
    {
        Some(end) => {
            let at = 4 + end + 5;
            format!("{}\n{line}{}", &skill[..at], &skill[at..])
        }
        None => format!("{line}{skill}"),
    }
}

/// The version of herdfold that installed `text`, if herdfold did.
fn installed_by(text: &str) -> Option<&str> {
    let rest = &text[text.find(MARK)? + MARK.len()..];
    Some(rest[..rest.find(" -->")?].trim())
}

/// How the skill at hand stands against this herdfold's.
#[derive(Debug, PartialEq)]
enum Status {
    NotInstalled,
    Installed,
    /// Installed by another version of herdfold.
    Outdated(String),
    /// Installed by this version, and changed since.
    Modified,
    /// A skill of the same name that herdfold did not install.
    Foreign,
}

fn status_of(text: Option<&str>) -> Status {
    let Some(text) = text else {
        return Status::NotInstalled;
    };
    match installed_by(text) {
        None => Status::Foreign,
        Some(v) if v != env!("CARGO_PKG_VERSION") => Status::Outdated(v.to_string()),
        Some(_) if text != installed() => Status::Modified,
        Some(_) => Status::Installed,
    }
}

fn path_of(target: Target) -> Result<PathBuf, CliError> {
    target
        .path()
        .ok_or_else(|| CliError::new("no_home", "no HOME to install the skill in"))
}

fn read(path: &PathBuf) -> Result<Option<String>, CliError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CliError::io(e)),
    }
}

/// `herdfold integration install`: writes the skill, keeping a skill of
/// the same name that herdfold did not install (or that was changed) as
/// `SKILL.md.bak` (or the first of `SKILL.md.bak.2`, ... not taken).
pub fn install(target: Target) -> Result<(), CliError> {
    let path = path_of(target)?;
    let old = read(&path)?;
    let name = target.name();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(CliError::io)?;
    }
    let mut kept = None;
    if matches!(
        status_of(old.as_deref()),
        Status::Foreign | Status::Modified
    ) {
        // Never over an earlier one: SKILL.md.bak, then SKILL.md.bak.2, ...
        let backup = (1..)
            .map(|n| match n {
                1 => path.with_extension("md.bak"),
                n => path.with_extension(format!("md.bak.{n}")),
            })
            .find(|p| !p.exists())
            .expect("a free name");
        std::fs::copy(&path, &backup).map_err(CliError::io)?;
        kept = Some(backup);
    }
    std::fs::write(&path, installed()).map_err(CliError::io)?;
    println!("{name}: installed ({})", path.display());
    if let Some(backup) = kept {
        println!(
            "{name}: the skill that was there is kept at {}",
            backup.display()
        );
    }
    Ok(())
}

/// `herdfold integration uninstall`: removes the skill, if herdfold
/// installed it.
pub fn uninstall(target: Target) -> Result<(), CliError> {
    let path = path_of(target)?;
    let name = target.name();
    match status_of(read(&path)?.as_deref()) {
        Status::NotInstalled => println!("{name}: not installed ({})", path.display()),
        Status::Foreign => {
            return Err(CliError::new(
                "not_installed_by_herdfold",
                format!(
                    "{} was not installed by herdfold; left as it is",
                    path.display()
                ),
            ));
        }
        _ => {
            std::fs::remove_file(&path).map_err(CliError::io)?;
            if let Some(dir) = path.parent() {
                // Only if nothing else is in it.
                let _ = std::fs::remove_dir(dir);
            }
            println!("{name}: uninstalled ({})", path.display());
        }
    }
    Ok(())
}

/// `herdfold integration status`: how the skill stands for each agent.
pub fn status() -> Result<(), CliError> {
    for &target in <Target as clap::ValueEnum>::value_variants() {
        let path = path_of(target)?;
        let status = match status_of(read(&path)?.as_deref()) {
            Status::NotInstalled => "not installed".to_string(),
            Status::Installed => "installed".to_string(),
            Status::Outdated(v) => {
                format!("outdated (v{v}, this is v{})", env!("CARGO_PKG_VERSION"))
            }
            Status::Modified => "installed, changed since".to_string(),
            Status::Foreign => "another skill of the same name".to_string(),
        };
        println!("{}: {status} ({})", target.name(), path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_goes_after_the_front_matter() {
        let marked = mark("---\nname: x\n---\n\n# X\n", "1.2.3");
        assert_eq!(
            marked,
            "---\nname: x\n---\n\n<!-- installed by herdfold 1.2.3 -->\n\n# X\n"
        );
        assert_eq!(installed_by(&marked), Some("1.2.3"));
        assert!(installed().starts_with("---\nname: herdfold\n"));
    }

    #[test]
    fn a_skill_is_told_apart_by_who_installed_it_and_when() {
        assert_eq!(status_of(None), Status::NotInstalled);
        assert_eq!(status_of(Some(&installed())), Status::Installed);
        assert_eq!(status_of(Some(SKILL)), Status::Foreign);
        assert_eq!(
            status_of(Some(&mark(SKILL, "0.1.0"))),
            Status::Outdated("0.1.0".into())
        );
        assert_eq!(status_of(Some(&(installed() + "more\n"))), Status::Modified);
    }
}
