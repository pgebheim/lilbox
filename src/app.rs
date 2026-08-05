use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, OptionalExtension};

use crate::config::Config;
use crate::model::{BoxRow, Template};
use crate::templates::{builtin_template, validate_template_name};
use crate::util::find_program;

pub(crate) struct App {
    pub(crate) config_dir: PathBuf,
    pub(crate) data_dir: PathBuf,
    pub(crate) state_dir: PathBuf,
    pub(crate) db: Connection,
    pub(crate) tailscale: Option<PathBuf>,
}

fn migrate(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS boxes(
            name TEXT PRIMARY KEY, image TEXT NOT NULL, guest_port INTEGER,
            host_port INTEGER, serve_port INTEGER, public INTEGER DEFAULT 0,
            url TEXT, created TEXT, comment TEXT, template TEXT, volume TEXT,
            expires INTEGER, stopped_reason TEXT, tailscale_node TEXT
        );",
    )?;
    for (name, ty) in [
        ("template", "TEXT"),
        ("volume", "TEXT"),
        ("expires", "INTEGER"),
        ("stopped_reason", "TEXT"),
        ("tailscale_node", "TEXT"),
    ] {
        let present = db
            .prepare("PRAGMA table_info(boxes)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|column| column == name);
        if !present {
            db.execute(&format!("ALTER TABLE boxes ADD COLUMN {name} {ty}"), [])?;
        }
    }
    Ok(())
}

fn resolve_dir(xdg_dir: Option<PathBuf>, kind: &str) -> Result<PathBuf> {
    match xdg_dir {
        Some(dir) => Ok(dir.join("lilbox")),
        None => dirs::home_dir()
            .map(|home| home.join(".lilbox"))
            .ok_or_else(|| anyhow!("could not determine {kind} directory or home directory")),
    }
}

/// Best-effort move of a legacy `~/.lilbox` file/directory to its new XDG
/// home. Never errors -- a failure is only warned to stderr, since a
/// migration hiccup must never block `App::new()`.
fn move_best_effort(src: &Path, dest: &Path) {
    if !src.exists() || src == dest {
        return;
    }
    // Never clobber something already at the destination (e.g. a hand-created
    // ~/.config/lilbox/config.toml): leave the legacy copy in place and warn.
    if dest.exists() {
        eprintln!(
            "warning: not migrating {} — {} already exists",
            src.display(),
            dest.display()
        );
        return;
    }
    if let Some(parent) = dest.parent()
        && let Err(err) = fs::create_dir_all(parent)
    {
        eprintln!(
            "warning: could not prepare {} for migration: {err:#}",
            parent.display()
        );
        return;
    }
    if let Err(err) = fs::rename(src, dest) {
        eprintln!(
            "warning: could not migrate {} to {}: {err:#}",
            src.display(),
            dest.display()
        );
    }
}

/// One-time migration from the legacy single `~/.lilbox` dotdir to the XDG
/// layout. Only runs when the legacy dir exists and the new state db hasn't
/// been created yet, so it never clobbers a box that's already migrated.
/// Best-effort throughout: any failure is warned, never fatal.
fn migrate_legacy_dir(legacy: &Path, config_dir: &Path, data_dir: &Path, state_dir: &Path) {
    if !legacy.exists() || state_dir.join("state.db").exists() {
        return;
    }
    println!(
        "migrating {} to the XDG state/config/data dirs ...",
        legacy.display()
    );
    move_best_effort(&legacy.join("config.toml"), &config_dir.join("config.toml"));
    move_best_effort(&legacy.join("state.db"), &state_dir.join("state.db"));
    move_best_effort(&legacy.join("logs"), &state_dir.join("logs"));
    move_best_effort(&legacy.join("templates"), &data_dir.join("templates"));
    move_best_effort(&legacy.join("workspaces"), &data_dir.join("workspaces"));
}

impl App {
    pub(crate) fn new() -> Result<Self> {
        let config_dir = resolve_dir(dirs::config_dir(), "config")?;
        let data_dir = resolve_dir(dirs::data_dir(), "data")?;
        let state_dir = resolve_dir(dirs::state_dir(), "state")?;
        fs::create_dir_all(&config_dir)?;
        fs::create_dir_all(&data_dir)?;
        fs::create_dir_all(&state_dir)?;
        if let Some(legacy) = dirs::home_dir().map(|home| home.join(".lilbox")) {
            migrate_legacy_dir(&legacy, &config_dir, &data_dir, &state_dir);
        }
        let db = Connection::open(state_dir.join("state.db"))?;
        migrate(&db)?;
        Ok(Self {
            config_dir,
            data_dir,
            state_dir,
            db,
            tailscale: find_program("tailscale"),
        })
    }

    pub(crate) fn config_path(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub(crate) fn db_path(&self) -> PathBuf {
        self.state_dir.join("state.db")
    }

    pub(crate) fn logs_dir(&self) -> PathBuf {
        self.state_dir.join("logs")
    }

    pub(crate) fn workspaces_dir(&self) -> PathBuf {
        self.data_dir.join("workspaces")
    }

    pub(crate) fn templates_dir(&self) -> PathBuf {
        self.data_dir.join("templates")
    }

    pub(crate) fn row(&self, name: &str) -> Result<Option<BoxRow>> {
        self.db.query_row(
            "SELECT name,image,guest_port,host_port,serve_port,public,url,template,volume,expires,stopped_reason,created,tailscale_node FROM boxes WHERE name=?1",
            [name],
            |r| Ok(BoxRow {
                name: r.get(0)?, image: r.get(1)?, guest_port: r.get(2)?, host_port: r.get(3)?,
                serve_port: r.get(4)?, public: r.get::<_, i64>(5)? != 0, url: r.get(6)?,
                template: r.get(7)?, volume: r.get(8)?, expires: r.get(9)?, stopped_reason: r.get(10)?,
                created: r.get(11)?, tailscale_node: r.get(12)?,
            }),
        ).optional().map_err(Into::into)
    }

    pub(crate) fn require_row(&self, name: &str) -> Result<BoxRow> {
        self.row(name)?
            .ok_or_else(|| anyhow!("no box named '{name}' (see: lilbox ls)"))
    }

    pub(crate) fn rows(&self) -> Result<Vec<BoxRow>> {
        let mut stmt = self.db.prepare(
            "SELECT name,image,guest_port,host_port,serve_port,public,url,template,volume,expires,stopped_reason,created,tailscale_node FROM boxes ORDER BY created",
        )?;
        Ok(stmt
            .query_map([], |r| {
                Ok(BoxRow {
                    name: r.get(0)?,
                    image: r.get(1)?,
                    guest_port: r.get(2)?,
                    host_port: r.get(3)?,
                    serve_port: r.get(4)?,
                    public: r.get::<_, i64>(5)? != 0,
                    url: r.get(6)?,
                    template: r.get(7)?,
                    volume: r.get(8)?,
                    expires: r.get(9)?,
                    stopped_reason: r.get(10)?,
                    created: r.get(11)?,
                    tailscale_node: r.get(12)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn config(&self) -> Result<Config> {
        let path = self.config_path();
        if !path.exists() {
            return Ok(Config::default());
        }
        toml::from_str(&fs::read_to_string(&path)?)
            .with_context(|| format!("invalid {}", path.display()))
    }

    pub(crate) fn template(&self, name: &str) -> Result<Template> {
        validate_template_name(name)?;
        let user = self.templates_dir().join(name);
        if user.join("template.json").is_file() {
            let manifest = serde_json::from_str(&fs::read_to_string(user.join("template.json"))?)?;
            let setup = user.join("setup.sh");
            return Ok(Template {
                name: name.into(),
                source: "user",
                dockerfile: user.join("Dockerfile").is_file(),
                setup: setup
                    .is_file()
                    .then(|| fs::read_to_string(setup))
                    .transpose()?,
                dir: Some(user),
                manifest,
            });
        }
        builtin_template(name)
            .ok_or_else(|| anyhow!("no template named '{name}' (see: lilbox templates)"))
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::params;

    use super::*;

    /// The `boxes.tailscale_node` column: written on join, read back by both
    /// the single-row and list queries, and absent for a box that never joined.
    mod tailscale_node {
        use super::*;

        fn test_app() -> App {
            let db = Connection::open_in_memory().unwrap();
            migrate(&db).unwrap();
            App {
                config_dir: PathBuf::new(),
                data_dir: PathBuf::new(),
                state_dir: PathBuf::new(),
                db,
                tailscale: None,
            }
        }

        #[test]
        fn round_trips() {
            let app = test_app();
            app.db
                .execute(
                    "INSERT INTO boxes(name,image,created) VALUES(?1,?2,?3)",
                    params!["box1", "python", "2024-01-01"],
                )
                .unwrap();
            app.db
                .execute(
                    "UPDATE boxes SET tailscale_node=?1 WHERE name=?2",
                    params!["box1.tail1234.ts.net", "box1"],
                )
                .unwrap();
            let row = app.row("box1").unwrap().unwrap();
            assert_eq!(row.tailscale_node.as_deref(), Some("box1.tail1234.ts.net"));
        }

        /// A box that never joined a tailnet has no node name.
        #[test]
        fn defaults_to_none() {
            let app = test_app();
            app.db
                .execute(
                    "INSERT INTO boxes(name,image,created) VALUES(?1,?2,?3)",
                    params!["box2", "python", "2024-01-01"],
                )
                .unwrap();
            let row = app.row("box2").unwrap().unwrap();
            assert!(row.tailscale_node.is_none());
        }

        /// `rows()` selects the column too, not just `row()`.
        #[test]
        fn appears_in_rows() {
            let app = test_app();
            app.db
                .execute(
                    "INSERT INTO boxes(name,image,created,tailscale_node) VALUES(?1,?2,?3,?4)",
                    params!["box3", "python", "2024-01-01", "box3.tail1234.ts.net"],
                )
                .unwrap();
            let rows = app.rows().unwrap();
            assert_eq!(
                rows[0].tailscale_node.as_deref(),
                Some("box3.tail1234.ts.net")
            );
        }
    }

    mod resolve_dir {
        use super::*;

        /// An XDG base dir is used as-is, with `lilbox` appended.
        #[test]
        fn uses_xdg_when_present() {
            let resolved = resolve_dir(Some(PathBuf::from("/xdg/config")), "config").unwrap();
            assert_eq!(resolved, PathBuf::from("/xdg/config/lilbox"));
        }
    }

    mod migrate_legacy_dir {
        use super::*;

        fn temp_dir(label: &str) -> PathBuf {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            std::env::temp_dir().join(format!(
                "lilbox-app-test-{label}-{}-{nonce}",
                std::process::id()
            ))
        }

        /// Each piece of the old single-directory layout lands in its XDG home:
        /// config with config, the state DB and logs with state, templates and
        /// workspaces with data -- and the legacy copies are gone afterwards.
        #[test]
        fn moves_pieces() {
            let root = temp_dir("migrate");
            let legacy = root.join("legacy");
            let config_dir = root.join("config");
            let data_dir = root.join("data");
            let state_dir = root.join("state");
            fs::create_dir_all(legacy.join("logs")).unwrap();
            fs::create_dir_all(legacy.join("templates").join("my-template")).unwrap();
            fs::create_dir_all(legacy.join("workspaces")).unwrap();
            fs::write(legacy.join("config.toml"), "image = \"alpine\"").unwrap();
            fs::write(legacy.join("state.db"), b"fake-db").unwrap();
            fs::write(legacy.join("logs").join("box-setup.log"), "log").unwrap();
            fs::create_dir_all(&config_dir).unwrap();
            fs::create_dir_all(&data_dir).unwrap();
            fs::create_dir_all(&state_dir).unwrap();

            migrate_legacy_dir(&legacy, &config_dir, &data_dir, &state_dir);

            assert!(config_dir.join("config.toml").is_file());
            assert!(state_dir.join("state.db").is_file());
            assert!(state_dir.join("logs").join("box-setup.log").is_file());
            assert!(data_dir.join("templates").join("my-template").is_dir());
            assert!(data_dir.join("workspaces").is_dir());
            assert!(!legacy.join("config.toml").exists());
            assert!(!legacy.join("state.db").exists());

            fs::remove_dir_all(&root).unwrap();
        }

        /// Migration is once-only: an existing state DB in the new home means
        /// this already ran, so leave both sides untouched rather than
        /// overwriting migrated state with the legacy copy.
        #[test]
        fn skips_when_db_exists() {
            let root = temp_dir("migrate-skip");
            let legacy = root.join("legacy");
            let config_dir = root.join("config");
            let data_dir = root.join("data");
            let state_dir = root.join("state");
            fs::create_dir_all(&legacy).unwrap();
            fs::write(legacy.join("state.db"), b"legacy-db").unwrap();
            fs::create_dir_all(&config_dir).unwrap();
            fs::create_dir_all(&data_dir).unwrap();
            fs::create_dir_all(&state_dir).unwrap();
            fs::write(state_dir.join("state.db"), b"already-migrated").unwrap();

            migrate_legacy_dir(&legacy, &config_dir, &data_dir, &state_dir);

            assert!(legacy.join("state.db").is_file());
            assert_eq!(
                fs::read(state_dir.join("state.db")).unwrap(),
                b"already-migrated"
            );

            fs::remove_dir_all(&root).unwrap();
        }
    }
}
