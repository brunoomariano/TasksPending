//! The desktop app: a GTK4 window over the dashboard, with the daemon's
//! API as the only way in.

mod activity;
mod backend;
mod clock;
mod present;
mod snooze;
mod state;
mod theme;
mod ui;
mod view;

use std::path::PathBuf;

use anyhow::Context;
use pending_runtime::live::{Env, process_env};

/// The application id, also the window's app-id on Wayland and the name of
/// its desktop entry.
pub const APP_ID: &str = "io.github.brunoomariano.TasksPending";

/// What a screenshot shows: the board with two cards marked, the same in
/// the light scheme, or one of the dialogs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scene {
    #[default]
    Board,
    Light,
    Sources,
    Settings,
    Snoozed,
}

#[derive(Debug, Default)]
pub struct AppOptions {
    /// Config file for the sources when no daemon runs.
    pub config: Option<PathBuf>,
    /// Where to look for the daemon.
    pub daemon: Option<String>,
    /// Run the sources inside the app, without looking for a daemon.
    pub no_daemon: bool,
    /// Simulated cards.
    pub sandbox: bool,
    /// Write a picture of the window to this file once it is drawn, then
    /// close it. For the README.
    pub screenshot: Option<PathBuf>,
    /// What the picture shows.
    pub scene: Scene,
}

/// Opens the window and runs until it closes.
pub fn run(options: AppOptions) -> anyhow::Result<()> {
    pending_api::init_tracing();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting async runtime")?;
    let unique = !options.sandbox && options.screenshot.is_none();
    let env = process_env();
    let view_path = view_file(&options, &env);
    let status = ui::run(
        &runtime,
        backend::Options {
            config: options.config,
            daemon: options.daemon,
            no_daemon: options.no_daemon,
            sandbox: options.sandbox,
        },
        env,
        view_path,
        options.screenshot.map(|path| (path, options.scene)),
        unique,
    )?;
    // Quit right away: an in-flight request must not keep the process
    // alive after the window closed.
    runtime.shutdown_background();
    if status == 0 {
        Ok(())
    } else {
        anyhow::bail!("the app exited with status {status}")
    }
}

/// Where the window remembers the view: nowhere for the sandbox and a
/// screenshot, which must neither show the user's filters nor save their
/// own over them.
fn view_file(options: &AppOptions, env: &Env) -> Option<PathBuf> {
    if options.sandbox || options.screenshot.is_some() {
        return None;
    }
    view::view_path(&**env)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::sync::Arc;

    use super::*;

    fn home(path: &str) -> Env {
        let path = OsString::from(path);
        Arc::new(move |key: &str| (key == "HOME").then(|| path.clone()))
    }

    /// The window remembers the view in the state directory the
    /// environment names, but the sandbox and a screenshot neither read
    /// nor write it: they would show the user's filters, and save the
    /// demo's over them.
    #[test]
    fn only_the_real_dashboard_remembers_the_view() {
        let env = home("/home/me");
        assert_eq!(
            view_file(&AppOptions::default(), &env),
            Some(PathBuf::from(
                "/home/me/.local/state/tasks-pending/app-view.json"
            ))
        );
        let sandbox = AppOptions {
            sandbox: true,
            ..AppOptions::default()
        };
        assert_eq!(view_file(&sandbox, &env), None);
        let screenshot = AppOptions {
            screenshot: Some(PathBuf::from("board.png")),
            ..AppOptions::default()
        };
        assert_eq!(view_file(&screenshot, &env), None);
    }
}
