use std::{env, fs, process::Command as ProcessCommand};

use anyhow::{Result, anyhow, bail};
use chrono::Local;
use microsandbox::Sandbox;
use rusqlite::params;

use crate::app::App;
use crate::cli::AgentArgs;
use crate::sandbox::{run_guest, with_secret_env};
use crate::util::{DEFAULT_GUEST_PORT, alloc_host_port, find_program, random_name};

const AGENT_WORKDIR: &str = "/workspace";

pub(crate) async fn cmd_agent(app: &App, args: AgentArgs) -> Result<i32> {
    let name = match args.name {
        Some(name) => name,
        None => random_name(app)?,
    };
    if app.row(&name)?.is_some() {
        bail!("box '{name}' already exists");
    }
    let workspace = if let Some(url) = args.clone {
        let repo = url
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("repo")
            .trim_end_matches(".git");
        let dest = app.workspaces_dir().join(repo);
        if dest.exists() {
            bail!("clone target already exists: {}", dest.display());
        }
        fs::create_dir_all(dest.parent().unwrap())?;
        let git = find_program("git").ok_or_else(|| anyhow!("git not found"))?;
        let status = ProcessCommand::new(git)
            .args(["clone", "--depth", "1", &url])
            .arg(&dest)
            .status()?;
        if !status.success() {
            bail!("git clone failed");
        }
        dest
    } else {
        args.workspace
            .unwrap_or(env::current_dir()?)
            .canonicalize()?
    };
    if !workspace.is_dir() {
        bail!("workspace not found: {}", workspace.display());
    }
    let host_port = alloc_host_port()?;
    let mut builder = Sandbox::builder(&name)
        .image(args.image.as_str())
        .port(host_port, DEFAULT_GUEST_PORT)
        .workdir(AGENT_WORKDIR)
        .volume(AGENT_WORKDIR, |m| m.bind(&workspace))
        .detached(true)
        .label("dev.lilbox.managed", "true")
        .label("dev.lilbox.kind", "agent");
    if env::var(&args.key_env).is_ok_and(|value| !value.is_empty()) {
        builder = with_secret_env(builder, &args.key_env, &args.key_host);
    } else {
        eprintln!(
            "warning: {} is not set; the agent cannot authenticate",
            args.key_env
        );
    }
    println!(
        "booting agent box {name} ({}, workspace {}) ...",
        args.image,
        workspace.display()
    );
    let sandbox = builder.create_detached().await?;
    app.db.execute("INSERT INTO boxes(name,image,guest_port,host_port,created,comment) VALUES(?1,?2,?3,?4,?5,'agent')",
        params![name, args.image, DEFAULT_GUEST_PORT, host_port, Local::now().format("%Y-%m-%d %H:%M:%S").to_string()])?;
    if let Some(source) = args.agents_file {
        let target = workspace.join("AGENTS.md");
        if source.canonicalize()? != target.canonicalize().unwrap_or_else(|_| target.clone()) {
            fs::copy(source, target)?;
        }
    }
    let install = "command -v claude >/dev/null 2>&1 || { command -v curl >/dev/null 2>&1 || ((apt-get update -qq && apt-get install -y -qq curl) 2>/dev/null || apk add --no-cache curl); curl -fsSL https://claude.ai/install.sh | bash; [ ! -x \"$HOME/.local/bin/claude\" ] || ln -sf \"$HOME/.local/bin/claude\" /usr/local/bin/claude; }";
    let output = sandbox.shell(install).await?;
    if !output.status().success {
        eprintln!(
            "warning: agent install failed: {}",
            output.stderr().unwrap_or_default()
        );
    }
    println!("agent box {name} ready (workspace mounted at {AGENT_WORKDIR})");
    if args.task.is_empty() {
        return Ok(0);
    }
    let task = args.task.join(" ");
    let command = vec![
        "sh".into(),
        "-c".into(),
        "IS_SANDBOX=1 exec claude -p \"$1\" --dangerously-skip-permissions".into(),
        "sh".into(),
        task,
    ];
    run_guest(&sandbox, &command, Some(AGENT_WORKDIR)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn agent_secret_is_stored_as_an_environment_reference() {
        let config = with_secret_env(
            Sandbox::builder("agent-secret-test").image("python"),
            "ANTHROPIC_API_KEY",
            "api.anthropic.com",
        )
        .build()
        .await
        .unwrap();
        let serialized = serde_json::to_string(&config).unwrap();

        assert!(serialized.contains("ANTHROPIC_API_KEY"));
        assert!(serialized.contains("\"kind\":\"env\""));
        assert!(!serialized.contains("sk-ant-test-secret"));
    }
}
