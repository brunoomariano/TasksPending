//! The desktop app: a GTK4 window over the dashboard, with the daemon's
//! API as the only way in.

mod activity;
mod alert;
mod backend;
mod clock;
mod dropdown;
mod keymap;
mod omarchy_bar;
mod present;
mod session;
mod snooze;
mod state;
mod theme;
mod todo_store;
mod ui;
mod view;

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use pending_runtime::live::{Env, process_env};

/// The application id, also the window's app-id on Wayland and the name of
/// its desktop entry.
pub const APP_ID: &str = "io.github.brunoomariano.TasksPending";

/// What a screenshot shows: the Activity area with cards at each stage,
/// the same in the light scheme or in a sample Omarchy theme, the Board
/// with two cards picked, or one of the dialogs (the keys dialog among
/// them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scene {
    #[default]
    Activity,
    Light,
    Omarchy,
    Board,
    Sources,
    Settings,
    Snoozed,
    Todo,
    Keys,
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
    let todo_path = todo_file(&options, &env);
    let status = ui::run(
        &runtime,
        backend::Options {
            config: options.config,
            daemon: options.daemon,
            no_daemon: options.no_daemon,
            sandbox: options.sandbox,
        },
        env,
        ui::Files {
            view: view_path,
            todos: todo_path,
        },
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

/// `tasks-pending toggle`, what the icon on Omarchy's bar runs: the
/// window drops down over the screen, or goes away when it is shown there.
/// With none open, one opens as the drop-down, born floating in its own
/// workspace; off Hyprland, or when Hyprland will not start it, `open`
/// opens it here the usual way.
pub fn toggle(open: impl FnOnce() -> anyhow::Result<()>) -> anyhow::Result<()> {
    let bus = gtk4::gio::bus_get_sync(gtk4::gio::BusType::Session, gtk4::gio::Cancellable::NONE)
        .context("connecting to the session bus")?;
    toggle_through(&Live { bus }, open)
}

/// What `tasks-pending toggle` reaches, apart so that its order is tested
/// without a session bus or Hyprland.
trait Reach {
    /// Whether the window is open.
    fn running(&self) -> anyhow::Result<bool>;
    /// Asks the open window to toggle, and waits for its answer.
    fn ask_window(&self) -> anyhow::Result<()>;
    /// Has Hyprland start the window dropped down; `None` off Hyprland.
    fn spawn_dropdown(&self) -> Option<anyhow::Result<()>>;
}

/// The open window is asked; when it does not take it (an older one,
/// without the action), it is raised the usual way by `open`, which hands
/// it to the open one. With none open, Hyprland starts it dropped down,
/// and without Hyprland, or when it refuses, `open` opens it here.
fn toggle_through(
    reach: &impl Reach,
    open: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if reach.running()? {
        match reach.ask_window() {
            Ok(()) => return Ok(()),
            Err(error) => tracing::warn!(%error, "the window did not toggle; raising it"),
        }
    } else if let Some(spawned) = reach.spawn_dropdown() {
        match spawned {
            Ok(()) => return Ok(()),
            Err(error) => tracing::warn!(%error, "Hyprland did not start the drop-down"),
        }
    }
    open()
}

/// The session bus and Hyprland, as `tasks-pending toggle` finds them.
struct Live {
    bus: gtk4::gio::DBusConnection,
}

impl Reach for Live {
    fn running(&self) -> anyhow::Result<bool> {
        running(&self.bus)
    }

    fn ask_window(&self) -> anyhow::Result<()> {
        use gtk4::glib::Variant;
        use gtk4::prelude::*;
        // GApplication's `org.gtk.Actions.Activate` (action, parameter,
        // platform data), called so that a refusal comes back.
        let call = (
            "toggle",
            Vec::<Variant>::new(),
            HashMap::<String, Variant>::new(),
        );
        self.bus
            .call_sync(
                Some(APP_ID),
                &object_path(APP_ID),
                "org.gtk.Actions",
                "Activate",
                Some(&call.to_variant()),
                None,
                gtk4::gio::DBusCallFlags::NONE,
                2000,
                gtk4::gio::Cancellable::NONE,
            )
            .context("asking the window to toggle")?;
        Ok(())
    }

    fn spawn_dropdown(&self) -> Option<anyhow::Result<()>> {
        let env = process_env();
        let text = |name: &str| env(name).and_then(|value| value.into_string().ok());
        let socket = dropdown::socket(&text)?;
        Some((|| {
            let exe = std::env::current_exe().context("finding the tasks-pending executable")?;
            let spawn = dropdown::Dispatch::Spawn(dropdown::spawn_command(&exe));
            Ok(dropdown::Hyprland::new(socket).run(&[spawn])?)
        })())
    }
}

/// Whether the window is open: its application owns its name on the bus.
fn running(bus: &gtk4::gio::DBusConnection) -> anyhow::Result<bool> {
    use gtk4::prelude::*;
    let reply = bus
        .call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            Some(&(APP_ID,).to_variant()),
            Some(gtk4::glib::VariantTy::new("(b)").expect("a variant type")),
            gtk4::gio::DBusCallFlags::NONE,
            1000,
            gtk4::gio::Cancellable::NONE,
        )
        .context("asking the session bus whether the window is open")?;
    let (owned,) = reply
        .get::<(bool,)>()
        .context("reading the session bus's answer")?;
    Ok(owned)
}

/// Where an application with the id `id` publishes its actions on the
/// bus, as GApplication names it.
fn object_path(id: &str) -> String {
    format!("/{}", id.replace('.', "/").replace('-', "_"))
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

/// Where the window keeps the todos: nowhere for the sandbox and a
/// screenshot, which keep their own in memory.
fn todo_file(options: &AppOptions, env: &Env) -> Option<PathBuf> {
    if options.sandbox || options.screenshot.is_some() {
        return None;
    }
    pending_runtime::config::data_dir(&**env).map(|dir| dir.join(todo_store::FILE_NAME))
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

    /// A stand-in for what `tasks-pending toggle` reaches, recording what
    /// it was asked.
    struct Stand {
        running: Option<bool>,
        window: bool,
        hyprland: Option<bool>,
        asked: std::cell::RefCell<Vec<&'static str>>,
    }

    impl Stand {
        fn new(running: Option<bool>, window: bool, hyprland: Option<bool>) -> Self {
            Self {
                running,
                window,
                hyprland,
                asked: Default::default(),
            }
        }

        fn toggle(&self) -> (anyhow::Result<()>, Vec<&'static str>) {
            let outcome = toggle_through(self, || {
                self.asked.borrow_mut().push("open");
                Ok(())
            });
            (outcome, self.asked.take())
        }
    }

    impl Reach for Stand {
        fn running(&self) -> anyhow::Result<bool> {
            self.asked.borrow_mut().push("running");
            self.running.ok_or_else(|| anyhow::anyhow!("no bus"))
        }
        fn ask_window(&self) -> anyhow::Result<()> {
            self.asked.borrow_mut().push("window");
            self.window
                .then_some(())
                .ok_or_else(|| anyhow::anyhow!("no toggle action"))
        }
        fn spawn_dropdown(&self) -> Option<anyhow::Result<()>> {
            self.asked.borrow_mut().push("hyprland");
            self.hyprland
                .map(|ok| ok.then_some(()).ok_or_else(|| anyhow::anyhow!("refused")))
        }
    }

    /// The running window is asked to toggle, and when it does not take
    /// it (an older one, without the action) it is raised the usual way.
    /// With none running, Hyprland starts one dropped down; without
    /// Hyprland, or when it refuses, it opens here. A bus that cannot say
    /// whether one runs is an error.
    #[test]
    fn toggle_asks_the_window_then_hyprland_then_opens() {
        let run = |running, window, hyprland| Stand::new(running, window, hyprland).toggle();
        let (outcome, asked) = run(Some(true), true, Some(true));
        assert!(outcome.is_ok());
        assert_eq!(asked, ["running", "window"]);
        let (outcome, asked) = run(Some(true), false, Some(true));
        assert!(outcome.is_ok());
        assert_eq!(asked, ["running", "window", "open"]);
        let (outcome, asked) = run(Some(false), true, Some(true));
        assert!(outcome.is_ok());
        assert_eq!(asked, ["running", "hyprland"]);
        let (outcome, asked) = run(Some(false), true, Some(false));
        assert!(outcome.is_ok());
        assert_eq!(asked, ["running", "hyprland", "open"]);
        let (outcome, asked) = run(Some(false), true, None);
        assert!(outcome.is_ok());
        assert_eq!(asked, ["running", "hyprland", "open"]);
        let (outcome, asked) = run(None, true, Some(true));
        assert!(outcome.is_err());
        assert_eq!(asked, ["running"]);
    }

    /// The bar's click reaches the running window where GApplication
    /// publishes its actions.
    #[test]
    fn finds_the_running_windows_actions() {
        assert_eq!(object_path(APP_ID), "/io/github/brunoomariano/TasksPending");
        assert_eq!(object_path("org.example.my-app"), "/org/example/my_app");
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

    /// The todos are kept in the data dir, as XDG names it; the sandbox
    /// and a screenshot keep theirs in memory, away from the user's.
    #[test]
    fn only_the_real_dashboard_keeps_the_todos() {
        let env = home("/home/me");
        assert_eq!(
            todo_file(&AppOptions::default(), &env),
            Some(PathBuf::from(
                "/home/me/.local/share/tasks-pending/todos.sqlite3"
            ))
        );
        let data: Env =
            Arc::new(|key: &str| (key == "XDG_DATA_HOME").then(|| OsString::from("/data")));
        assert_eq!(
            todo_file(&AppOptions::default(), &data),
            Some(PathBuf::from("/data/tasks-pending/todos.sqlite3"))
        );
        let sandbox = AppOptions {
            sandbox: true,
            ..AppOptions::default()
        };
        assert_eq!(todo_file(&sandbox, &env), None);
        let screenshot = AppOptions {
            screenshot: Some(PathBuf::from("board.png")),
            ..AppOptions::default()
        };
        assert_eq!(todo_file(&screenshot, &env), None);
    }
}
