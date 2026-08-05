use std::{fs, path::Path, path::PathBuf, process::Command as ProcessCommand};

use anyhow::{Result, anyhow, bail};

use crate::app::App;
use crate::cli::TemplateCommand;
use crate::templates::validate_template_name;
use crate::util::find_program;

fn templates_dir(app: &App) -> PathBuf {
    app.templates_dir()
}

/// True if `s` looks like a git remote (as opposed to a local path).
pub(crate) fn is_git_source(s: &str) -> bool {
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("git@")
        || s.starts_with("ssh://")
        || s.ends_with(".git")
}

/// Derive the template's install name from its source, honoring an
/// explicit `--name` override. Strips a trailing `.git` and any path
/// separators, leaving just the final path component.
pub(crate) fn derive_template_name(source: &str, explicit: Option<&str>) -> String {
    if let Some(name) = explicit {
        return name.to_string();
    }
    source
        .trim_end_matches('/')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(source)
        .trim_end_matches(".git")
        .to_string()
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else if ty.is_file() {
            fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}

pub(crate) fn cmd_template(app: &App, command: TemplateCommand) -> Result<()> {
    match command {
        TemplateCommand::Add {
            source,
            name,
            force,
        } => add(app, &source, name, force),
        TemplateCommand::Remove { name } => remove(app, &name),
    }
}

/// Install a template from a local directory `source` into `templates_dir`
/// under `name`. Self-contained (no `App` dependency) so it can be exercised
/// directly with temp dirs in tests.
fn install_local(templates_dir: &Path, source: &Path, name: &str, force: bool) -> Result<()> {
    let dest = templates_dir.join(name);
    if dest.exists() {
        if !force {
            bail!("template '{name}' already exists (use --force to overwrite)");
        }
        fs::remove_dir_all(&dest)?;
    }
    if !source.is_dir() {
        bail!("source not found: {}", source.display());
    }
    fs::create_dir_all(dest.parent().unwrap())?;

    if let Err(err) = copy_dir_recursive(source, &dest) {
        let _ = fs::remove_dir_all(&dest);
        return Err(err);
    }

    if !dest.join("template.json").is_file() {
        let _ = fs::remove_dir_all(&dest);
        bail!(
            "'{}' does not look like a template (missing template.json)",
            source.display()
        );
    }

    Ok(())
}

/// Remove the installed template named `name` from `templates_dir`.
fn remove_local(templates_dir: &Path, name: &str) -> Result<()> {
    let dest = templates_dir.join(name);
    if !dest.is_dir() {
        bail!("no user template named '{name}'");
    }
    fs::remove_dir_all(&dest)?;
    Ok(())
}

fn add(app: &App, source: &str, name: Option<String>, force: bool) -> Result<()> {
    let name = derive_template_name(source, name.as_deref());
    validate_template_name(&name)?;
    let templates_root = templates_dir(app);

    if is_git_source(source) {
        let dest = templates_root.join(&name);
        if dest.exists() {
            if !force {
                bail!("template '{name}' already exists (use --force to overwrite)");
            }
            fs::remove_dir_all(&dest)?;
        }
        fs::create_dir_all(dest.parent().unwrap())?;

        let git = find_program("git").ok_or_else(|| anyhow!("git not found"))?;
        let status = ProcessCommand::new(git)
            .args([
                "-c",
                "protocol.ext.allow=never",
                "clone",
                "--depth",
                "1",
                "--",
            ])
            .arg(source)
            .arg(&dest)
            .status()?;
        if !status.success() {
            let _ = fs::remove_dir_all(&dest);
            bail!("git clone failed");
        }

        if !dest.join("template.json").is_file() {
            let _ = fs::remove_dir_all(&dest);
            bail!("'{source}' does not look like a template (missing template.json)");
        }
    } else {
        install_local(&templates_root, Path::new(source), &name, force)?;
    }

    println!("installed template '{name}' from {source}");
    Ok(())
}

fn remove(app: &App, name: &str) -> Result<()> {
    validate_template_name(name)?;
    remove_local(&templates_dir(app), name)?;
    println!("removed template '{name}'");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "lilbox-template-test-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    mod is_git_source {
        use super::*;

        /// Every shape of git remote we accept: https, http, scp-style ssh,
        /// ssh://, and a local path carrying the `.git` suffix.
        #[test]
        fn detects_git() {
            assert!(is_git_source("https://github.com/foo/bar"));
            assert!(is_git_source("http://example.com/repo.git"));
            assert!(is_git_source("git@github.com:foo/bar.git"));
            assert!(is_git_source("ssh://git@example.com/foo/bar"));
            assert!(is_git_source("/some/local/path.git"));
        }

        /// A plain path or bare name is a local source, not something to clone.
        #[test]
        fn rejects_non_git() {
            assert!(!is_git_source("/some/local/path"));
            assert!(!is_git_source("./relative/dir"));
            assert!(!is_git_source("plain-name"));
        }
    }

    mod derive_template_name {
        use super::*;

        /// The repo name, with the `.git` suffix dropped.
        #[test]
        fn derives_from_git_url() {
            assert_eq!(
                derive_template_name("https://github.com/foo/my-template.git", None),
                "my-template"
            );
        }

        /// The last path segment, with or without a trailing slash.
        #[test]
        fn derives_from_local_path() {
            assert_eq!(derive_template_name("/tmp/some/dir", None), "dir");
            assert_eq!(derive_template_name("/tmp/some/dir/", None), "dir");
        }

        /// An explicitly supplied name skips derivation entirely.
        #[test]
        fn explicit_wins() {
            assert_eq!(
                derive_template_name("https://github.com/foo/bar.git", Some("custom")),
                "custom"
            );
        }
    }

    mod copy_dir_recursive {
        use super::*;

        /// Nested files come across, not just the top level.
        #[test]
        fn copies_nested_tree() {
            let source = temp_dir("source");
            fs::create_dir_all(&source).unwrap();
            fs::write(source.join("template.json"), "{}").unwrap();
            fs::create_dir_all(source.join("nested")).unwrap();
            fs::write(source.join("nested").join("setup.sh"), "echo hi").unwrap();

            let templates_root = temp_dir("templates");
            let dest = templates_root.join("my-template");

            copy_dir_recursive(&source, &dest).unwrap();
            assert!(dest.join("template.json").is_file());
            assert!(dest.join("nested").join("setup.sh").is_file());

            fs::remove_dir_all(&dest).unwrap();
            assert!(!dest.exists());

            let _ = fs::remove_dir_all(&source);
            let _ = fs::remove_dir_all(&templates_root);
        }
    }

    mod install_local {
        use super::*;

        fn make_valid_template_source(label: &str) -> PathBuf {
            let source = temp_dir(label);
            fs::create_dir_all(&source).unwrap();
            fs::write(source.join("template.json"), "{}").unwrap();
            source
        }

        /// Installing over an existing template needs `--force`; without it the
        /// error says so rather than silently replacing the user's copy.
        #[test]
        fn refuses_existing() {
            let source = make_valid_template_source("il-source-1");
            let templates_root = temp_dir("il-templates-1");
            fs::create_dir_all(templates_root.join("my-template")).unwrap();

            let err = install_local(&templates_root, &source, "my-template", false).unwrap_err();
            assert!(err.to_string().contains("already exists"));

            let _ = fs::remove_dir_all(&source);
            let _ = fs::remove_dir_all(&templates_root);
        }

        /// `--force` replaces rather than merges: stale files from the previous
        /// install must not survive.
        #[test]
        fn overwrites_with_force() {
            let source = make_valid_template_source("il-source-2");
            let templates_root = temp_dir("il-templates-2");
            let dest = templates_root.join("my-template");
            fs::create_dir_all(&dest).unwrap();
            fs::write(dest.join("stale.txt"), "old").unwrap();

            install_local(&templates_root, &source, "my-template", true).unwrap();
            assert!(dest.join("template.json").is_file());
            assert!(!dest.join("stale.txt").exists());

            let _ = fs::remove_dir_all(&source);
            let _ = fs::remove_dir_all(&templates_root);
        }

        /// A source without `template.json` isn't a template. The failure must
        /// leave nothing behind at the destination.
        #[test]
        fn cleans_up_on_error() {
            let source = temp_dir("il-source-3");
            fs::create_dir_all(&source).unwrap();
            fs::write(source.join("not-a-template.txt"), "hi").unwrap();
            let templates_root = temp_dir("il-templates-3");

            let err = install_local(&templates_root, &source, "my-template", false).unwrap_err();
            assert!(err.to_string().contains("template.json"));
            assert!(!templates_root.join("my-template").exists());

            let _ = fs::remove_dir_all(&source);
            let _ = fs::remove_dir_all(&templates_root);
        }

        #[test]
        fn copies_template() {
            let source = make_valid_template_source("il-source-4");
            fs::create_dir_all(source.join("nested")).unwrap();
            fs::write(source.join("nested").join("setup.sh"), "echo hi").unwrap();
            let templates_root = temp_dir("il-templates-4");

            install_local(&templates_root, &source, "my-template", false).unwrap();
            let dest = templates_root.join("my-template");
            assert!(dest.join("template.json").is_file());
            assert!(dest.join("nested").join("setup.sh").is_file());

            let _ = fs::remove_dir_all(&source);
            let _ = fs::remove_dir_all(&templates_root);
        }
    }

    mod remove_local {
        use super::*;

        #[test]
        fn errors_when_missing() {
            let templates_root = temp_dir("rl-templates-1");
            fs::create_dir_all(&templates_root).unwrap();

            let err = remove_local(&templates_root, "does-not-exist").unwrap_err();
            assert!(err.to_string().contains("no user template named"));

            let _ = fs::remove_dir_all(&templates_root);
        }
    }
}
