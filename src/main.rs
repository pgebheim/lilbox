mod app;
mod cli;
mod commands;
mod config;
mod logs;
mod model;
mod provision;
mod sandbox;
mod tailscale;
mod templates;
mod util;

use clap::Parser;

use app::App;
use cli::Cli;

#[tokio::main]
async fn main() {
    let command = Cli::parse().command;
    let code = match App::new() {
        Ok(app) => match commands::dispatch(&app, command).await {
            Ok(code) => code,
            Err(error) => {
                eprintln!("error: {error:#}");
                1
            }
        },
        Err(error) => {
            eprintln!("error: {error:#}");
            1
        }
    };
    std::process::exit(code);
}
