use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use pending_api::{ServeOptions, serve};
use pending_tui::{TuiOptions, run};

const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:61000";

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
    /// Loopback address and port for the local dashboard. Non-loopback
    /// addresses are rejected because the API has no authentication.
    #[arg(long, default_value = DEFAULT_LISTEN_ADDR)]
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

/// Compatibility parser for package upgrades that still invoke `pending-api`.
#[derive(Debug, Parser)]
#[command(name = "pending-api", about = "TasksPending dashboard API", version)]
struct LegacyServeCli {
    #[command(flatten)]
    command: ServeCommand,
}

/// Compatibility parser for package upgrades that still invoke `pending-tui`.
#[derive(Debug, Parser)]
#[command(
    name = "pending-tui",
    about = "TasksPending terminal dashboard",
    version
)]
struct LegacyTuiCli {
    #[command(flatten)]
    command: TuiCommand,
}

fn main() -> anyhow::Result<()> {
    if is_invoked_as("pending-api") {
        return run_serve(LegacyServeCli::parse().command);
    }
    if is_invoked_as("pending-tui") {
        return run_tui(LegacyTuiCli::parse().command);
    }

    match Cli::parse().command {
        Command::Serve(command) => run_serve(command),
        Command::Tui(command) => run_tui(command),
    }
}

fn is_invoked_as(name: &str) -> bool {
    std::env::args_os()
        .next()
        .as_deref()
        .and_then(|path| Path::new(path).file_name())
        .is_some_and(|file| file == name)
}

fn run_serve(command: ServeCommand) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new().context("starting async runtime")?;
    runtime.block_on(serve(ServeOptions {
        listen: command.listen,
        config: command.config,
        static_dir: command.static_dir,
    }))
}

fn run_tui(command: TuiCommand) -> anyhow::Result<()> {
    run(TuiOptions {
        config: command.config,
    })
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{Cli, Command, LegacyServeCli, LegacyTuiCli};

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

    #[test]
    fn serve_uses_the_uncommon_local_default_port() {
        let cli = Cli::try_parse_from(["tasks-pending", "serve"]).unwrap();
        let Command::Serve(command) = cli.command else {
            panic!("expected the serve command");
        };

        assert_eq!(command.listen, "127.0.0.1:61000".parse().unwrap());
    }

    #[test]
    fn legacy_package_aliases_accept_their_previous_arguments() {
        LegacyServeCli::command().debug_assert();
        LegacyTuiCli::command().debug_assert();
        assert!(
            LegacyServeCli::try_parse_from(["pending-api", "--listen", "127.0.0.1:9000"]).is_ok()
        );
        assert!(
            LegacyTuiCli::try_parse_from(["pending-tui", "--config", "dashboard.toml"]).is_ok()
        );
    }
}
