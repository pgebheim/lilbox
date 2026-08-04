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

impl App {
    pub(crate) fn new() -> Result<Self> {
        let state_dir = dirs::home_dir()
            .ok_or_else(|| anyhow!("could not determine home directory"))?
            .join(".lilbox");
        fs::create_dir_all(&state_dir)?;
        let db = Connection::open(state_dir.join("state.db"))?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS boxes(
                name TEXT PRIMARY KEY, image TEXT NOT NULL, guest_port INTEGER,
                host_port INTEGER, serve_port INTEGER, public INTEGER DEFAULT 0,
                url TEXT, created TEXT, comment TEXT, template TEXT, volume TEXT,
                expires INTEGER, stopped_reason TEXT
            );",
        )?;
        for (name, ty) in [
            ("template", "TEXT"),
            ("volume", "TEXT"),
            ("expires", "INTEGER"),
            ("stopped_reason", "TEXT"),
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
        Ok(Self {
            state_dir,
            db,
            tailscale: find_program("tailscale"),
        })
    }

    pub(crate) fn row(&self, name: &str) -> Result<Option<BoxRow>> {
        self.db.query_row(
            "SELECT name,image,guest_port,host_port,serve_port,public,url,template,volume,expires,stopped_reason,created FROM boxes WHERE name=?1",
            [name],
            |r| Ok(BoxRow {
                name: r.get(0)?, image: r.get(1)?, guest_port: r.get(2)?, host_port: r.get(3)?,
                serve_port: r.get(4)?, public: r.get::<_, i64>(5)? != 0, url: r.get(6)?,
                template: r.get(7)?, volume: r.get(8)?, expires: r.get(9)?, stopped_reason: r.get(10)?,
                created: r.get(11)?,
            }),
        ).optional().map_err(Into::into)
    }

    pub(crate) fn require_row(&self, name: &str) -> Result<BoxRow> {
        self.row(name)?
            .ok_or_else(|| anyhow!("no box named '{name}' (see: lilbox ls)"))
    }

    pub(crate) fn rows(&self) -> Result<Vec<BoxRow>> {
        let mut stmt = self.db.prepare(
            "SELECT name,image,guest_port,host_port,serve_port,public,url,template,volume,expires,stopped_reason,created FROM boxes ORDER BY created",
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
