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

pub use backend::Options as BackendOptions;

/// The application id, also the window's app-id on Wayland and the name of
/// its desktop entry.
pub const APP_ID: &str = "io.github.brunoomariano.TasksPending";

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
}

/// Opens the window and runs until it closes.
pub fn run(options: AppOptions) -> anyhow::Result<()> {
    pending_api::init_tracing();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting async runtime")?;
    let backend = backend::resolve(
        &runtime,
        BackendOptions {
            config: options.config,
            daemon: options.daemon,
            no_daemon: options.no_daemon,
            sandbox: options.sandbox,
        },
    )?;
    let status = ui::run(&runtime, backend, options.screenshot);
    // Quit right away: an in-flight request must not keep the process
    // alive after the window closed.
    runtime.shutdown_background();
    if status == 0 {
        Ok(())
    } else {
        anyhow::bail!("the app exited with status {status}")
    }
}
