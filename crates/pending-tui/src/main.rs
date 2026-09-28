mod app;
mod ui;

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use crossterm::ExecutableCommand;
use crossterm::cursor::Show;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use pending_runtime::Aggregator;
use pending_runtime::config::{Origin, load_plan};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::{Action, App};

#[derive(Debug, Parser)]
#[command(
    name = "pending-tui",
    about = "TasksPending terminal dashboard",
    version
)]
struct Cli {
    /// Config file. Defaults to $TASKS_PENDING_CONFIG, then
    /// $XDG_CONFIG_HOME/tasks-pending/config.toml, then
    /// ~/.config/tasks-pending/config.toml.
    #[arg(long)]
    config: Option<PathBuf>,
}

/// How often the screen reads the aggregator and checks for keys.
const TICK: Duration = Duration::from_millis(250);

struct TerminalSession;

impl TerminalSession {
    fn enter() -> anyhow::Result<Self> {
        enable_raw_mode().context("enabling raw mode")?;
        if let Err(error) = io::stdout().execute(EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error).context("entering alternate screen");
        }

        // Restore the terminal before a UI-thread panic message prints, so it
        // is readable instead of lost with the alternate screen. Panics on
        // other threads (a source bug) are handled by the runtime and must
        // leave the TUI running.
        let ui = std::thread::current().id();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if is_ui_thread(ui) {
                restore_terminal();
            }
            previous(info);
        }));
        Ok(Self)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn is_ui_thread(ui: std::thread::ThreadId) -> bool {
    std::thread::current().id() == ui
}

fn restore_terminal() {
    let _ = io::stdout().execute(Show);
    let _ = disable_raw_mode();
    let _ = io::stdout().execute(LeaveAlternateScreen);
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // Config errors print before the terminal switches to the alternate screen.
    let (plan, origin) = load_plan(cli.config, &|key| std::env::var_os(key))?;
    let header = match &origin {
        Origin::File(path) => format!("config: {}", path.display()),
        Origin::SampleDefault {
            searched: Some(path),
        } => {
            format!("sample source (no config at {})", path.display())
        }
        Origin::SampleDefault { searched: None } => "sample source (no config)".to_owned(),
    };

    let runtime = tokio::runtime::Runtime::new().context("starting async runtime")?;
    let result = {
        let _guard = runtime.enter();
        let aggregator = Aggregator::start(plan.specs, plan.timeout);
        run(&aggregator, &header)
    };
    // Quit right away: an in-flight DNS lookup on a blocking thread must not
    // keep the process alive after the terminal is restored.
    runtime.shutdown_background();
    result
}

fn run(aggregator: &Aggregator, header: &str) -> anyhow::Result<()> {
    let _session = TerminalSession::enter()?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).context("opening terminal")?;
    let mut app = App::new(aggregator.snapshot());

    loop {
        app.update(aggregator.snapshot());
        terminal
            .draw(|frame| ui::render(frame.area(), frame.buffer_mut(), &app, header))
            .context("drawing TUI")?;

        if !event::poll(TICK)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match app.handle_key(key) {
            Action::Quit => break,
            Action::RefreshNow => aggregator.refresh_now(),
            Action::Open(url) => {
                if let Err(error) = open_url(&url) {
                    app.set_notice(format!("could not open link: {error}"));
                }
            }
            Action::None => {}
        }
    }

    terminal.show_cursor().context("showing cursor")?;
    Ok(())
}

/// Opens a card link in the default browser. A background thread reaps the
/// opener so it does not linger as a zombie.
fn open_url(url: &str) -> io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|mut child| {
            std::thread::spawn(move || child.wait());
        })
        .map_err(|error| io::Error::new(error.kind(), format!("{opener}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O terminal só é restaurado por pânico da thread da interface; o pânico
    /// de uma fonte em outra thread é tratado pelo runtime e a TUI continua
    /// usável.
    #[test]
    fn only_ui_thread_panics_restore_the_terminal() {
        let ui = std::thread::current().id();
        assert!(is_ui_thread(ui));

        let from_worker = std::thread::spawn(move || is_ui_thread(ui)).join().unwrap();
        assert!(!from_worker);
    }
}
