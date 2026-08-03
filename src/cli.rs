use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::util::DEFAULT_IMAGE;

#[derive(Parser)]
#[command(
    name = "vm",
    about = "a lil exe.dev you run yourself - microVMs on your tailnet"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Create a new persistent microVM.
    New(NewArgs),
    /// List boxes.
    Ls {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Reap boxes whose TTL has elapsed.
    Gc,
    /// List available templates.
    Templates,
    /// Re-run a box template's setup script.
    Provision { name: String },
    /// Run a command in a box.
    Exec(ExecArgs),
    /// Open a shell (or run a command) in a box.
    Ssh(ExecArgs),
    /// Copy a file to or from a box (box side is NAME:/path).
    Cp { src: String, dst: String },
    /// Show a box's captured output.
    Logs(LogsArgs),
    /// Boot an ephemeral box, run a command, and discard it.
    Run(RunArgs),
    /// Publish a box over Tailscale HTTPS.
    Expose {
        name: String,
        #[arg(long)]
        public: bool,
    },
    /// Stop publishing a box.
    Unexpose { name: String },
    /// Stop a running box.
    Stop { name: String },
    /// Start a stopped box.
    Start { name: String },
    /// Restart a box.
    Restart { name: String },
    /// Remove a box.
    Rm {
        name: String,
        #[arg(long)]
        keep_data: bool,
    },
    /// Snapshot a box and boot a clone.
    Fork {
        name: String,
        newname: Option<String>,
    },
    /// Recreate a box on an image while preserving its home volume.
    Rebuild {
        name: String,
        #[arg(long)]
        image: Option<String>,
    },
    /// List persistent home volumes.
    Volumes,
    /// Manage the embedded microsandbox image cache.
    Image(ImageArgs),
    /// Show detailed box information.
    Stat { name: String },
    /// Print a box's published URL.
    Url { name: String },
    /// Run Claude Code in a microVM against a mounted workspace.
    Agent(AgentArgs),
    /// Check the local runtime and Tailscale installation.
    Doctor,
}

#[derive(Args)]
pub(crate) struct ImageArgs {
    #[command(subcommand)]
    pub(crate) command: ImageCommand,
}

#[derive(Subcommand)]
pub(crate) enum ImageCommand {
    /// Import an OCI or Docker archive into the image cache.
    Load {
        archive: PathBuf,
        #[arg(short, long)]
        tag: String,
    },
    /// List cached images.
    Ls,
}

#[derive(Args)]
pub(crate) struct NewArgs {
    pub(crate) name: Option<String>,
    #[arg(long)]
    pub(crate) template: Option<String>,
    #[arg(long)]
    pub(crate) image: Option<String>,
    #[arg(long)]
    pub(crate) port: Option<u16>,
    #[arg(long)]
    pub(crate) cpus: Option<u8>,
    #[arg(long)]
    pub(crate) memory: Option<String>,
    #[arg(long)]
    pub(crate) rebuild: bool,
    #[arg(long)]
    pub(crate) no_persist: bool,
    #[arg(long)]
    pub(crate) volume: Option<String>,
    #[arg(long)]
    pub(crate) ttl: Option<String>,
    #[arg(long)]
    pub(crate) idle_timeout: Option<String>,
}

#[derive(Args)]
pub(crate) struct ExecArgs {
    pub(crate) name: String,
    #[arg(last = true)]
    pub(crate) cmd: Vec<String>,
}

#[derive(Args)]
pub(crate) struct RunArgs {
    #[arg(long, default_value = DEFAULT_IMAGE)]
    pub(crate) image: String,
    #[arg(long)]
    pub(crate) ttl: Option<String>,
    #[arg(last = true)]
    pub(crate) cmd: Vec<String>,
}

#[derive(Args)]
pub(crate) struct LogsArgs {
    pub(crate) name: String,
    /// Keep streaming newly appended output.
    #[arg(short, long)]
    pub(crate) follow: bool,
    /// Show only the last N captured chunks.
    #[arg(long)]
    pub(crate) tail: Option<usize>,
    /// Comma-separated stdout, stderr, output, system, or all.
    #[arg(long)]
    pub(crate) source: Option<String>,
}

#[derive(Args)]
pub(crate) struct AgentArgs {
    pub(crate) name: Option<String>,
    #[arg(long, default_value = DEFAULT_IMAGE)]
    pub(crate) image: String,
    #[arg(long, conflicts_with = "clone")]
    pub(crate) workspace: Option<PathBuf>,
    #[arg(long)]
    pub(crate) clone: Option<String>,
    #[arg(long)]
    pub(crate) agents_file: Option<PathBuf>,
    #[arg(long, default_value = "ANTHROPIC_API_KEY")]
    pub(crate) key_env: String,
    #[arg(long, default_value = "api.anthropic.com")]
    pub(crate) key_host: String,
    #[arg(last = true)]
    pub(crate) task: Vec<String>,
}
