mod app;
mod clock;
mod looks;
mod snooze;
mod ui;

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;
use chrono::{DateTime, Local, Utc};
use crossterm::ExecutableCommand;
use crossterm::cursor::Show;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseEventKind,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use pending_runtime::Dashboard;
use pending_runtime::config::{Origin, cache_path};
use pending_runtime::live::{Live, process_env};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::{Action, App, Viewport};
use crate::looks::LookThrottle;
use crate::snooze::Choice;

#[derive(Debug)]
pub struct TuiOptions {
    pub config: Option<PathBuf>,
}

/// How often the config file is checked for changes.
const CONFIG_WATCH_INTERVAL: Duration = Duration::from_secs(2);

/// How often the screen reads the dashboard and checks for keys. Well under a
/// second, so the clock's seconds never skip; waiting for input in between
/// keeps the loop idle.
const TICK: Duration = Duration::from_millis(250);

struct TerminalSession;

impl TerminalSession {
    fn enter() -> anyhow::Result<Self> {
        enable_raw_mode().context("enabling raw mode")?;
        if let Err(error) = io::stdout().execute(EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error).context("entering alternate screen");
        }
        // Wheel and clicks. Terminals still select text with Shift held.
        if let Err(error) = io::stdout().execute(EnableMouseCapture) {
            restore_terminal();
            return Err(error).context("capturing the mouse");
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
    let _ = disable_raw_mode();
    reset_screen(&mut io::stdout());
}

/// Shows the cursor, releases the mouse and leaves the alternate screen,
/// each step regardless of the others failing.
fn reset_screen(out: &mut impl io::Write) {
    let _ = out.execute(Show);
    let _ = out.execute(DisableMouseCapture);
    let _ = out.execute(LeaveAlternateScreen);
}

pub fn run(options: TuiOptions) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new().context("starting async runtime")?;
    let result = {
        let _guard = runtime.enter();
        let env = process_env();
        let cache = cache_path(&*env);
        // Config errors print before the terminal switches to the alternate
        // screen; later edits of the file are reloaded while running.
        let (live, origin) = Live::start(options.config, env, cache)?;
        let header = match &origin {
            Origin::File(path) => format!("config: {}", path.display()),
            Origin::SampleDefault {
                searched: Some(path),
            } => format!("sample source (no config at {})", path.display()),
            Origin::SampleDefault { searched: None } => "sample source (no config)".to_owned(),
        };
        let _watch = live.watch(CONFIG_WATCH_INTERVAL);
        run_dashboard(&live, &header)
    };
    // Quit right away: an in-flight DNS lookup on a blocking thread must not
    // keep the process alive after the terminal is restored.
    runtime.shutdown_background();
    result
}

fn run_dashboard(dashboard: &dyn Dashboard, header: &str) -> anyhow::Result<()> {
    let _session = TerminalSession::enter()?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).context("opening terminal")?;
    // Opening the TUI is looking at it; after that, keys and the mouse are.
    let mut looks = LookThrottle::default();
    look(dashboard, &mut looks, Instant::now());
    let mut app = App::new(dashboard.snapshot());
    let mut redraw = true;
    let mut drawn_at = Instant::now();

    loop {
        // Redraw after each handled event and on every tick, but not for
        // mouse movement, which the terminal reports continuously.
        if redraw || drawn_at.elapsed() >= TICK {
            app.update(dashboard.snapshot());
            let mut viewport = Viewport::default();
            terminal
                .draw(|frame| {
                    viewport = ui::render(
                        frame.area(),
                        frame.buffer_mut(),
                        &app,
                        header,
                        chrono::Local::now(),
                    );
                })
                .context("drawing TUI")?;
            app.set_viewport(viewport);
            drawn_at = Instant::now();
        }
        redraw = false;

        if !event::poll(TICK.saturating_sub(drawn_at.elapsed()))? {
            continue;
        }
        let key = match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                look(dashboard, &mut looks, Instant::now());
                key
            }
            Event::Mouse(mouse) => {
                // The pointer merely crossing the terminal is not looking.
                let moved = matches!(mouse.kind, MouseEventKind::Moved);
                if !moved {
                    look(dashboard, &mut looks, Instant::now());
                }
                redraw = !moved;
                app.handle_mouse(mouse);
                continue;
            }
            Event::Resize(..) => {
                redraw = true;
                continue;
            }
            _ => continue,
        };
        redraw = true;
        match app.handle_key(key) {
            Action::Quit => break,
            Action::RefreshNow => dashboard.refresh_now(),
            Action::Open(url) => {
                if let Err(error) = open_url(&url) {
                    app.set_notice(format!("could not open link: {error}"));
                }
            }
            Action::SetMark { id, marked } => set_mark(dashboard, &mut app, &id, marked),
            Action::Snooze { id, choice } => {
                snooze_card(dashboard, &mut app, &id, choice, &chrono::Local::now());
            }
            Action::Wake { id } => wake_card(dashboard, &mut app, &id),
            Action::None => {}
        }
    }

    terminal.show_cursor().context("showing cursor")?;
    Ok(())
}

/// Tells the running dashboard that the user is looking (a key press, a
/// click or the wheel at `now`), unless it was told less than a minute ago.
fn look(dashboard: &dyn Dashboard, throttle: &mut LookThrottle, now: Instant) {
    if throttle.due(now) {
        dashboard.look();
    }
}

/// Marks or unmarks a card on the running dashboard; the next frame reads
/// the result from its snapshot. A failure shows in the footer.
fn set_mark(dashboard: &dyn Dashboard, app: &mut App, id: &str, marked: bool) {
    if let Err(error) = dashboard.set_mark(id, marked) {
        app.set_notice(format!("could not mark card: {error}"));
    }
}

/// Snoozes a card on the running dashboard for as long as `choice` says,
/// counted from `now`; the next frame reads the result from its snapshot.
/// The footer says until when, or why it failed.
fn snooze_card(
    dashboard: &dyn Dashboard,
    app: &mut App,
    id: &str,
    choice: Choice,
    now: &DateTime<Local>,
) {
    let until = snooze::until(choice, now).map(|at| at.with_timezone(&Utc));
    match dashboard.snooze(id, until) {
        Ok(()) => app.set_info(format!("snoozed {}", snooze::until_text(until))),
        Err(error) => app.set_notice(format!("could not snooze card: {error}")),
    }
}

/// Brings a snoozed card back on the running dashboard; a failure shows in
/// the footer.
fn wake_card(dashboard: &dyn Dashboard, app: &mut App, id: &str) {
    if let Err(error) = dashboard.wake(id) {
        app.set_notice(format!("could not wake card: {error}"));
    }
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
    use chrono::TimeZone;
    use pending_runtime::marks::MarkError;
    use pending_runtime::snoozes::SnoozeError;

    use super::*;

    /// A dashboard that counts how often it was told the user is looking.
    #[derive(Default)]
    struct Looking {
        looks: std::sync::atomic::AtomicUsize,
    }

    impl Dashboard for Looking {
        fn snapshot(&self) -> pending_core::DashboardSnapshot {
            pending_core::sample_snapshot()
        }

        fn refresh_now(&self) {}

        fn look(&self) {
            self.looks
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Activity (keys, the mouse) tells the dashboard the user is looking:
    /// right away the first time (startup), then at most once a minute
    /// however many events arrive.
    #[test]
    fn activity_tells_the_dashboard_at_most_once_a_minute() {
        let dashboard = Looking::default();
        let looks = || dashboard.looks.load(std::sync::atomic::Ordering::Relaxed);
        let mut throttle = LookThrottle::default();
        let start = Instant::now();

        look(&dashboard, &mut throttle, start);
        assert_eq!(looks(), 1, "at startup");

        for second in 1..60 {
            look(
                &dashboard,
                &mut throttle,
                start + Duration::from_secs(second),
            );
        }
        assert_eq!(looks(), 1, "a burst of keys within the minute");

        look(&dashboard, &mut throttle, start + Duration::from_secs(60));
        look(&dashboard, &mut throttle, start + Duration::from_secs(61));
        assert_eq!(looks(), 2, "a minute later");
    }

    /// What a [`Snoozing`] dashboard was asked: to snooze a card until a
    /// time (or until it changes), or to wake it.
    #[derive(Debug, PartialEq)]
    enum Asked {
        Snooze(String, Option<DateTime<Utc>>),
        Wake(String),
    }

    /// A dashboard whose snoozes always fail, and one that records them.
    struct Snoozing {
        fails: bool,
        calls: std::sync::Mutex<Vec<Asked>>,
    }

    impl Snoozing {
        fn new(fails: bool) -> Self {
            Self {
                fails,
                calls: Default::default(),
            }
        }
    }

    impl Dashboard for Snoozing {
        fn snapshot(&self) -> pending_core::DashboardSnapshot {
            pending_core::sample_snapshot()
        }

        fn refresh_now(&self) {}

        fn snooze(&self, id: &str, until: Option<DateTime<Utc>>) -> Result<(), SnoozeError> {
            if self.fails {
                return Err(SnoozeError::UnknownCard);
            }
            let mut calls = self.calls.lock().unwrap();
            calls.push(Asked::Snooze(id.to_owned(), until));
            Ok(())
        }

        fn wake(&self, id: &str) -> Result<(), SnoozeError> {
            if self.fails {
                return Err(SnoozeError::Unsupported);
            }
            self.calls.lock().unwrap().push(Asked::Wake(id.to_owned()));
            Ok(())
        }
    }

    /// Snoozing goes to the running dashboard with the time the choice ends
    /// (counted in local time from now), or with none to wait for a change;
    /// the footer then says until when. A failure shows there as an error.
    #[test]
    fn snoozes_go_to_the_dashboard_with_the_time_of_the_choice() {
        let mut app = App::new(pending_core::sample_snapshot());
        let working = Snoozing::new(false);
        // A Wednesday afternoon.
        let now = Local.with_ymd_and_hms(2026, 10, 7, 15, 30, 0).unwrap();
        let at = |day, hour, minute| {
            let local = Local.with_ymd_and_hms(2026, 10, day, hour, minute, 0);
            Some(local.unwrap().with_timezone(&Utc))
        };

        snooze_card(&working, &mut app, "card-1", Choice::Hour, &now);
        snooze_card(&working, &mut app, "card-1", Choice::Tomorrow, &now);
        assert_eq!(app.notice(), Some("snoozed until Thu 8 Oct 08:00"));
        assert!(!app.notice_is_error());
        snooze_card(&working, &mut app, "card-2", Choice::NextWeek, &now);
        snooze_card(&working, &mut app, "card-2", Choice::UntilChange, &now);
        assert_eq!(app.notice(), Some("snoozed until it changes"));

        assert_eq!(
            *working.calls.lock().unwrap(),
            [
                Asked::Snooze("card-1".to_owned(), at(7, 16, 30)),
                Asked::Snooze("card-1".to_owned(), at(8, 8, 0)),
                Asked::Snooze("card-2".to_owned(), at(12, 8, 0)),
                Asked::Snooze("card-2".to_owned(), None),
            ]
        );

        snooze_card(&Snoozing::new(true), &mut app, "card-1", Choice::Hour, &now);
        let notice = app.notice().unwrap_or_default();
        assert!(notice.contains("could not snooze card"), "{notice}");
        assert!(app.notice_is_error());
    }

    /// Waking goes to the running dashboard and leaves the footer alone; a
    /// failure shows in the footer as an error.
    #[test]
    fn wakes_go_to_the_dashboard_and_failures_show_in_the_footer() {
        let mut app = App::new(pending_core::sample_snapshot());
        let working = Snoozing::new(false);

        wake_card(&working, &mut app, "card-1");
        assert_eq!(
            *working.calls.lock().unwrap(),
            [Asked::Wake("card-1".to_owned())]
        );
        assert_eq!(app.notice(), None);

        wake_card(&Snoozing::new(true), &mut app, "card-1");
        let notice = app.notice().unwrap_or_default();
        assert!(notice.contains("could not wake card"), "{notice}");
        assert!(app.notice_is_error());
    }

    /// A dashboard whose marks always fail, and one that records them.
    struct Marking {
        fails: bool,
        calls: std::sync::Mutex<Vec<(String, bool)>>,
    }

    impl Dashboard for Marking {
        fn snapshot(&self) -> pending_core::DashboardSnapshot {
            pending_core::sample_snapshot()
        }

        fn refresh_now(&self) {}

        fn set_mark(&self, id: &str, marked: bool) -> Result<(), MarkError> {
            if self.fails {
                return Err(MarkError::UnknownCard);
            }
            self.calls.lock().unwrap().push((id.to_owned(), marked));
            Ok(())
        }
    }

    /// Marking goes to the running dashboard; a failure shows in the footer
    /// as an error, and success leaves the footer alone.
    #[test]
    fn marks_go_to_the_dashboard_and_failures_show_in_the_footer() {
        let mut app = App::new(pending_core::sample_snapshot());
        let working = Marking {
            fails: false,
            calls: Default::default(),
        };

        set_mark(&working, &mut app, "card-1", true);
        assert_eq!(
            *working.calls.lock().unwrap(),
            [("card-1".to_owned(), true)]
        );
        assert_eq!(app.notice(), None);

        let failing = Marking {
            fails: true,
            calls: Default::default(),
        };
        set_mark(&failing, &mut app, "card-1", true);
        let notice = app.notice().unwrap_or_default();
        assert!(notice.contains("could not mark card"), "{notice}");
        assert!(app.notice_is_error());
    }

    /// Only a UI-thread panic restores the terminal; a source panicking on
    /// another thread is handled by the runtime and the TUI stays usable.
    #[test]
    fn only_ui_thread_panics_restore_the_terminal() {
        let ui = std::thread::current().id();
        assert!(is_ui_thread(ui));

        let from_worker = std::thread::spawn(move || is_ui_thread(ui)).join().unwrap();
        assert!(!from_worker);
    }

    /// Restoring the terminal (on quit or on a UI panic) also releases the
    /// mouse, so the shell gets clicks and text selection back.
    #[test]
    fn restoring_the_terminal_releases_the_mouse() {
        let mut out = Vec::new();
        reset_screen(&mut out);
        let out = String::from_utf8(out).unwrap();

        assert!(
            out.contains("\x1b[?1000l"),
            "mouse capture left on: {out:?}"
        );
        assert!(
            out.contains("\x1b[?1049l"),
            "alternate screen left on: {out:?}"
        );
        assert!(out.contains("\x1b[?25h"), "cursor left hidden: {out:?}");
    }
}
