mod setup;
mod upgrade;

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use clap::{Args, Parser, Subcommand, ValueEnum};
use pending_api::{ServeOptions, serve};
use pending_app::{AppOptions, Scene};
use pending_core::api::DEFAULT_LISTEN_ADDR;

#[derive(Debug, Parser)]
#[command(
    name = "tasks-pending",
    about = "TasksPending: your pending work, in one window",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Open the dashboard window (the default).
    App(AppCommand),
    /// Run the daemon: the sources and their local API, for the window.
    Serve(ServeCommand),
    /// Open the window over simulated cards, without configuration or credentials.
    Sandbox(SandboxCommand),
    /// Create the config files if missing, refresh the launcher caches and start (or restart) the daemon.
    Setup(SetupCommand),
    /// Fetch the latest release, check it against its published checksum and install it.
    Upgrade(UpgradeCommand),
}

#[derive(Debug, Args, Default)]
struct SetupCommand {
    /// Only the files; leave the daemon alone.
    #[arg(long)]
    no_start: bool,
}

#[derive(Debug, Args, Default)]
struct UpgradeCommand {
    /// Only say whether a newer release exists.
    #[arg(long)]
    check: bool,
    /// Install this version instead of the latest, e.g. 0.6.1 (`latest`
    /// is the latest). The installer reads the same variable.
    #[arg(long, env = "TASKS_PENDING_VERSION")]
    version: Option<String>,
    /// Install the asked release even when it is older than this one.
    #[arg(long)]
    allow_downgrade: bool,
}

#[derive(Debug, Args, Default)]
struct SandboxCommand {
    /// Picture the window into this PNG file once drawn, then close it.
    #[arg(long)]
    screenshot: Option<PathBuf>,
    /// What the picture shows.
    #[arg(long, default_value = "board", requires = "screenshot")]
    scene: SceneArg,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum SceneArg {
    /// The board, with two cards marked as in progress.
    #[default]
    Board,
    /// The same in the light scheme.
    Light,
    /// The Sources dialog.
    Sources,
    /// The Settings dialog.
    Settings,
    /// The Snoozed dialog, with one card snoozed.
    Snoozed,
}

impl From<SceneArg> for Scene {
    fn from(scene: SceneArg) -> Self {
        match scene {
            SceneArg::Board => Self::Board,
            SceneArg::Light => Self::Light,
            SceneArg::Sources => Self::Sources,
            SceneArg::Settings => Self::Settings,
            SceneArg::Snoozed => Self::Snoozed,
        }
    }
}

#[derive(Debug, Args, Default)]
struct AppCommand {
    /// Config file for the sources when no daemon runs. Defaults to
    /// $TASKS_PENDING_CONFIG, then $XDG_CONFIG_HOME/tasks-pending/config.toml,
    /// then ~/.config/tasks-pending/config.toml.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Where the daemon listens.
    #[arg(long, default_value = pending_client::DEFAULT_DAEMON_URL)]
    daemon: String,
    /// Run the sources inside the window instead of looking for the daemon.
    #[arg(long)]
    no_daemon: bool,
}

#[derive(Debug, Args)]
struct ServeCommand {
    /// Loopback address and port for the daemon. Non-loopback addresses
    /// are rejected because the API has no authentication.
    #[arg(long, default_value = DEFAULT_LISTEN_ADDR)]
    listen: SocketAddr,
    /// Config file. Defaults to $TASKS_PENDING_CONFIG, then
    /// $XDG_CONFIG_HOME/tasks-pending/config.toml, then
    /// ~/.config/tasks-pending/config.toml.
    #[arg(long)]
    config: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        None => run_app(AppCommand::default()),
        Some(Command::App(command)) => run_app(command),
        Some(Command::Serve(command)) => run_serve(command),
        Some(Command::Sandbox(command)) => pending_app::run(AppOptions {
            sandbox: true,
            screenshot: command.screenshot,
            scene: command.scene.into(),
            ..AppOptions::default()
        }),
        Some(Command::Setup(command)) => setup::run(setup::Options {
            no_start: command.no_start,
        }),
        Some(Command::Upgrade(command)) => upgrade::run(upgrade::Options {
            check: command.check,
            version: command.version,
            allow_downgrade: command.allow_downgrade,
        }),
    }
}

fn run_app(command: AppCommand) -> anyhow::Result<()> {
    pending_app::run(AppOptions {
        config: command.config,
        daemon: Some(command.daemon),
        no_daemon: command.no_daemon,
        sandbox: false,
        screenshot: None,
        scene: Scene::Board,
    })
}

fn run_serve(command: ServeCommand) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new().context("starting async runtime")?;
    runtime.block_on(serve(ServeOptions {
        listen: command.listen,
        config: command.config,
    }))
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{Cli, Command};

    #[test]
    fn cli_exposes_the_window_the_daemon_and_the_sandbox() {
        Cli::command().debug_assert();
        let command = Cli::command();
        assert!(command.find_subcommand("app").is_some());
        assert!(command.find_subcommand("serve").is_some());
        assert!(command.find_subcommand("sandbox").is_some());
        assert!(command.find_subcommand("setup").is_some());
        assert!(command.find_subcommand("upgrade").is_some());
        assert!(command.find_subcommand("tui").is_none());
        assert!(
            Cli::command()
                .try_get_matches_from(["tasks-pending", "serve", "--listen", "127.0.0.1:9000"])
                .is_ok()
        );
        assert!(
            Cli::command()
                .try_get_matches_from(["tasks-pending", "app", "--config", "dashboard.toml"])
                .is_ok()
        );
        assert!(
            Cli::command()
                .try_get_matches_from(["tasks-pending", "sandbox", "--screenshot", "x.png"])
                .is_ok()
        );
        assert!(
            Cli::command()
                .try_get_matches_from([
                    "tasks-pending",
                    "sandbox",
                    "--screenshot",
                    "x.png",
                    "--scene",
                    "snoozed"
                ])
                .is_ok()
        );
        assert!(
            Cli::command()
                .try_get_matches_from(["tasks-pending", "sandbox", "--scene", "light"])
                .is_err(),
            "a scene needs a screenshot"
        );
        assert!(
            Cli::command()
                .try_get_matches_from(["tasks-pending", "setup", "--no-start"])
                .is_ok()
        );
        assert!(
            Cli::command()
                .try_get_matches_from(["tasks-pending", "upgrade", "--check"])
                .is_ok()
        );
    }

    /// `TASKS_PENDING_VERSION` pins the version for `upgrade` the way it
    /// does for the installer.
    #[test]
    fn upgrade_reads_the_version_from_the_installer_variable() {
        let command = Cli::command();
        let upgrade = command.find_subcommand("upgrade").unwrap();
        let version = upgrade
            .get_arguments()
            .find(|arg| arg.get_id() == "version")
            .unwrap();
        assert_eq!(
            version.get_env().and_then(|env| env.to_str()),
            Some("TASKS_PENDING_VERSION")
        );
    }

    /// `--allow-downgrade` is what lets `upgrade` install an older release.
    #[test]
    fn upgrade_takes_a_downgrade_only_when_asked() {
        let cli = Cli::try_parse_from(["tasks-pending", "upgrade", "--version", "0.5.0"]).unwrap();
        let Some(Command::Upgrade(command)) = cli.command else {
            panic!("expected the upgrade command");
        };
        assert!(!command.allow_downgrade);
        let cli = Cli::try_parse_from([
            "tasks-pending",
            "upgrade",
            "--version",
            "0.5.0",
            "--allow-downgrade",
        ])
        .unwrap();
        let Some(Command::Upgrade(command)) = cli.command else {
            panic!("expected the upgrade command");
        };
        assert!(command.allow_downgrade);
    }

    /// Without a subcommand the window opens, looking for the daemon at
    /// its default address.
    #[test]
    fn the_window_is_the_default_and_finds_the_daemon() {
        let cli = Cli::try_parse_from(["tasks-pending"]).unwrap();
        assert!(cli.command.is_none());

        let cli = Cli::try_parse_from(["tasks-pending", "app"]).unwrap();
        let Some(Command::App(command)) = cli.command else {
            panic!("expected the app command");
        };
        assert_eq!(command.daemon, "http://127.0.0.1:61000");
        assert!(!command.no_daemon);
    }

    #[test]
    fn serve_uses_the_uncommon_local_default_port() {
        let cli = Cli::try_parse_from(["tasks-pending", "serve"]).unwrap();
        let Some(Command::Serve(command)) = cli.command else {
            panic!("expected the serve command");
        };
        assert_eq!(command.listen, "127.0.0.1:61000".parse().unwrap());
    }
}
