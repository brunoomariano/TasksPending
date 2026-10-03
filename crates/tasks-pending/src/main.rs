use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use pending_api::{SandboxOptions, ServeOptions, serve, serve_sandbox};
use pending_tui::{TuiOptions, run};

const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:61000";
const DEFAULT_SANDBOX_LISTEN_ADDR: &str = "127.0.0.1:61001";

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
    /// Run a dashboard seeded with simulated cards, without configuration or credentials.
    Sandbox(SandboxCommand),
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

#[derive(Debug, Args)]
struct SandboxCommand {
    /// Loopback address and port for the sandbox dashboard.
    #[arg(long, default_value = DEFAULT_SANDBOX_LISTEN_ADDR)]
    listen: SocketAddr,
    /// Built frontend to serve at `/` instead of the embedded release assets.
    #[arg(long)]
    static_dir: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Serve(command) => run_serve(command),
        Command::Tui(command) => run_tui(command),
        Command::Sandbox(command) => run_sandbox(command),
    }
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

fn run_sandbox(command: SandboxCommand) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new().context("starting async runtime")?;
    runtime.block_on(serve_sandbox(SandboxOptions {
        listen: command.listen,
        static_dir: command.static_dir,
    }))
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{Cli, Command};

    #[test]
    fn cli_exposes_dashboard_commands() {
        Cli::command().debug_assert();
        let command = Cli::command();
        assert!(command.find_subcommand("serve").is_some());
        assert!(command.find_subcommand("tui").is_some());
        assert!(command.find_subcommand("sandbox").is_some());
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
        assert!(
            Cli::command()
                .try_get_matches_from(["tasks-pending", "sandbox", "--listen", "127.0.0.1:9001"])
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
    fn sandbox_uses_its_own_local_port() {
        let cli = Cli::try_parse_from(["tasks-pending", "sandbox"]).unwrap();
        let Command::Sandbox(command) = cli.command else {
            panic!("expected the sandbox command");
        };

        assert_eq!(command.listen, "127.0.0.1:61001".parse().unwrap());
    }
}
