use std::{fs, path::PathBuf};

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, OptionalExtension};

use crate::config::Config;
use crate::model::{BoxRow, Template};
use crate::templates::{builtin_template, validate_template_name};
use crate::util::find_program;

pub(crate) struct App {
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

impl App {
    pub(crate) fn new() -> Result<Self> {
        let state_dir = dirs::home_dir()
            .ok_or_else(|| anyhow!("could not determine home directory"))?
            .join(".lilbox");
        fs::create_dir_all(&state_dir)?;
        let db = Connection::open(state_dir.join("state.db"))?;
        migrate(&db)?;
        Ok(Self {
            state_dir,
            db,
            tailscale: find_program("tailscale"),
        })
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
        let path = self.state_dir.join("config.toml");
        if !path.exists() {
            return Ok(Config::default());
        }
        toml::from_str(&fs::read_to_string(&path)?)
            .with_context(|| format!("invalid {}", path.display()))
    }

    pub(crate) fn template(&self, name: &str) -> Result<Template> {
        validate_template_name(name)?;
        let user = self.state_dir.join("templates").join(name);
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

    fn test_app() -> App {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        App {
            state_dir: PathBuf::new(),
            db,
            tailscale: None,
        }
    }

    #[test]
    fn records_and_reads_back_tailscale_node() {
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

    #[test]
    fn tailscale_node_defaults_to_none() {
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

    #[test]
    fn rows_lists_tailscale_node() {
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
