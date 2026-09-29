use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use pending_api::{ServeOptions, serve};
use pending_tui::{TuiOptions, run};

#[derive(Debug, Parser)]
#[command(name = "tasks-pending", about = "TasksPending dashboard", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the local dashboard HTTP server.
    Serve(ServeCommand),
    /// Open the terminal dashboard.
    Tui(TuiCommand),
}

#[derive(Debug, Args)]
struct ServeCommand {
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: SocketAddr,
    /// Config file. Defaults to $TASKS_PENDING_CONFIG, then
    /// $XDG_CONFIG_HOME/tasks-pending/config.toml, then
    /// ~/.config/tasks-pending/config.toml.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Built frontend to serve at `/` instead of the embedded release assets.
    #[arg(long)]
    static_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct TuiCommand {
    /// Config file. Defaults to $TASKS_PENDING_CONFIG, then
    /// $XDG_CONFIG_HOME/tasks-pending/config.toml, then
    /// ~/.config/tasks-pending/config.toml.
    #[arg(long)]
    config: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Serve(command) => {
            let runtime = tokio::runtime::Runtime::new().context("starting async runtime")?;
            runtime.block_on(serve(ServeOptions {
                listen: command.listen,
                config: command.config,
                static_dir: command.static_dir,
            }))
        }
        Command::Tui(command) => run(TuiOptions {
            config: command.config,
        }),
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::Cli;

    #[test]
    fn cli_exposes_serve_and_tui_commands() {
        Cli::command().debug_assert();
        let command = Cli::command();
        assert!(command.find_subcommand("serve").is_some());
        assert!(command.find_subcommand("tui").is_some());
        assert!(
            Cli::command()
                .try_get_matches_from(["tasks-pending", "serve", "--listen", "127.0.0.1:9000"])
                .is_ok()
        );
        assert!(
            Cli::command()
                .try_get_matches_from(["tasks-pending", "tui", "--config", "dashboard.toml"])
                .is_ok()
        );
    }
}
