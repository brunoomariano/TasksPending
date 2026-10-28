//! Where the dashboard comes from: the daemon when one is running, the
//! same API started inside the app when none is, or the sandbox. Whatever
//! the origin, the window talks to it the same way, over the loopback
//! client, from a worker on the async runtime.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use pending_api::{LiveServer, Router, sandbox_app, serve_router, start_live};
use pending_client::{Client, DEFAULT_DAEMON_URL, Error};
use pending_core::{DashboardSnapshot, WeatherReport};
use pending_runtime::config::Origin;
use pending_runtime::live::{Env, Watch};
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::info;

/// Sources answer a refresh in the background; read the result shortly
/// after.
const AFTER_REFRESH: Duration = Duration::from_secs(2);

/// Longest the window waits, when it opens, to learn whether a daemon
/// answers. A daemon answers its health check from memory in well under
/// this; something that takes the connection and stays silent must not
/// hold the window up for a whole request timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// What the window asks of the dashboard.
#[derive(Debug)]
pub enum Command {
    /// Read the snapshot now.
    Poll,
    /// Refresh every source, then read the snapshot a moment later.
    Refresh,
    SetMark {
        id: String,
        marked: bool,
    },
    Snooze {
        id: String,
        until: Option<DateTime<Utc>>,
    },
    Wake {
        id: String,
    },
    /// The user is looking at the dashboard; read the snapshot after
    /// saying so, because what counts as changed depends on it.
    Look,
    Weather,
    /// Ask whether a newer version was published.
    Version,
}

/// What the dashboard tells the window.
#[derive(Debug)]
pub enum Event {
    Snapshot(Result<DashboardSnapshot, String>),
    Weather(Option<WeatherReport>),
    /// A refresh was accepted, or why not (e.g. "wait 7s").
    Refreshed(Result<(), String>),
    /// A mark, snooze or wake went through, or why not.
    Acted(Result<(), String>),
    /// A newer published version, when the daemon knows of one.
    Update(Option<String>),
}

/// How the dashboard was found.
#[derive(Debug)]
pub enum Source {
    /// `tasks-pending serve` is running there.
    Daemon { url: String, version: String },
    /// The sources run inside the app, from `origin`: no daemon answered,
    /// or none was looked for (`daemon_wanted` is false).
    BuiltIn {
        origin: Origin,
        address: SocketAddr,
        daemon_wanted: bool,
    },
    /// Simulated cards.
    Sandbox { address: SocketAddr },
}

impl Source {
    /// One line for the window's status.
    pub fn describe(&self) -> String {
        match self {
            Self::Daemon { url, version } => format!("daemon {version} at {url}"),
            Self::BuiltIn {
                origin,
                address,
                daemon_wanted,
            } => {
                let why = if *daemon_wanted {
                    "no daemon running"
                } else {
                    "without a daemon, as asked"
                };
                let what = match origin {
                    Origin::File(path) => format!("sources from {}", path.display()),
                    Origin::SampleDefault { .. } => "no config; sample source".to_owned(),
                };
                format!("{why}; {what} (built-in API at {address})")
            }
            Self::Sandbox { address } => {
                format!("sandbox: simulated cards, nothing is fetched (API at {address})")
            }
        }
    }
}

/// The dashboard the window talks to.
pub struct Backend {
    pub client: Client,
    pub source: Source,
    /// The built-in API's config watch, kept while the window runs.
    _watch: Option<Watch>,
}

#[derive(Debug, Default)]
pub struct Options {
    /// Config file for the built-in sources, when no daemon runs.
    pub config: Option<PathBuf>,
    /// Where to look for the daemon; [`DEFAULT_DAEMON_URL`] by default.
    pub daemon: Option<String>,
    /// Never look for a daemon: run the sources inside the app.
    pub no_daemon: bool,
    /// Simulated cards instead of any source.
    pub sandbox: bool,
}

/// Finds the dashboard: the sandbox when asked, else the daemon, else the
/// sources started here, reading `env` for their cache and tokens. Blocks
/// on `runtime` while doing so.
pub fn resolve(runtime: &Runtime, options: Options, env: Env) -> anyhow::Result<Backend> {
    runtime.block_on(async {
        if options.sandbox {
            let (client, address) = serve_here(sandbox_app()).await?;
            return Ok(Backend {
                client,
                source: Source::Sandbox { address },
                _watch: None,
            });
        }
        let url = options
            .daemon
            .clone()
            .unwrap_or_else(|| DEFAULT_DAEMON_URL.to_owned());
        if !options.no_daemon {
            let probe = Client::with_timeout(&url, PROBE_TIMEOUT)
                .with_context(|| format!("daemon address {url}"))?;
            match probe.health().await {
                Ok(health) => {
                    info!(url, version = health.version, "using the daemon");
                    return Ok(Backend {
                        client: Client::new(&url)?,
                        source: Source::Daemon {
                            url,
                            version: health.version,
                        },
                        _watch: None,
                    });
                }
                // Nothing listening, or something that never answers.
                Err(error @ (Error::Transport(_) | Error::Timeout)) => {
                    info!(url, %error, "no daemon answered; starting the sources here")
                }
                Err(error) => anyhow::bail!("the daemon at {url} is not usable: {error}"),
            }
        }
        let LiveServer {
            router,
            origin,
            watch,
        } = start_live(options.config, env)?;
        let (client, address) = serve_here(router).await?;
        Ok(Backend {
            client,
            source: Source::BuiltIn {
                origin,
                address,
                daemon_wanted: !options.no_daemon,
            },
            _watch: Some(watch),
        })
    })
}

/// Serves `router` on a loopback port of the system's choosing, for this
/// process only, and returns a client for it.
async fn serve_here(router: Router) -> anyhow::Result<(Client, SocketAddr)> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("binding a loopback port for the built-in API")?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        if let Err(error) = serve_router(listener, router).await {
            tracing::error!(%error, "the built-in API stopped");
        }
    });
    let client = Client::new(&format!("http://{address}"))?;
    Ok((client, address))
}

/// Runs the window's commands against `client` on the runtime, and reports
/// back through the returned receiver. Changes run one at a time, in
/// order. Reads run beside them, so a change never waits for a slow read,
/// and a read asked for again before it was made is made once.
pub fn spawn_worker(
    runtime: &Runtime,
    client: Client,
) -> (UnboundedSender<Command>, UnboundedReceiver<Event>) {
    let (commands, mut inbox) = unbounded_channel::<Command>();
    let (events, outbox) = unbounded_channel::<Event>();
    let (changes, change_inbox) = unbounded_channel::<Command>();
    let reads = Arc::new(ReadQueue::default());
    runtime.spawn(read_lane(client.clone(), reads.clone(), events.clone()));
    runtime.spawn(change_lane(client, change_inbox, reads.clone(), events));
    runtime.spawn(async move {
        while let Some(command) = inbox.recv().await {
            if let Some(change) = reads.ask(command)
                && changes.send(change).is_err()
            {
                break;
            }
        }
    });
    (commands, outbox)
}

/// Reads asked for and not made yet, each once however often it was
/// asked for.
#[derive(Debug, Default)]
struct Reads {
    look: bool,
    poll: bool,
    weather: bool,
    version: bool,
}

impl Reads {
    /// Makes the reads, telling `events`; false once nobody listens.
    async fn run(self, client: &Client, events: &UnboundedSender<Event>) -> bool {
        // The look first, so the snapshot read after it counts changes
        // from now. A look that failed reads nothing by itself: the daemon
        // is not answering, and the next poll says so.
        let mut poll = self.poll;
        if self.look {
            poll |= client.look().await.is_ok();
        }
        if poll {
            let snapshot = client.snapshot().await.map_err(text);
            if events.send(Event::Snapshot(snapshot)).is_err() {
                return false;
            }
        }
        if self.weather {
            let report = client.weather().await.ok().flatten();
            if events.send(Event::Weather(report)).is_err() {
                return false;
            }
        }
        if self.version {
            let available = client.version().await.ok().and_then(|v| v.available);
            if events.send(Event::Update(available)).is_err() {
                return false;
            }
        }
        true
    }
}

/// The reads waiting for the read lane, and the signal that there are some.
#[derive(Debug, Default)]
struct ReadQueue {
    pending: Mutex<Reads>,
    ready: Notify,
}

impl ReadQueue {
    /// Queues `command` if it is a read, and gives it back if it is not.
    fn ask(&self, command: Command) -> Option<Command> {
        {
            let mut pending = self.pending.lock().expect("read queue");
            match command {
                Command::Poll => pending.poll = true,
                Command::Look => pending.look = true,
                Command::Weather => pending.weather = true,
                Command::Version => pending.version = true,
                change => return Some(change),
            }
        }
        self.ready.notify_one();
        None
    }

    /// The reads asked for since the last call, once there are any.
    async fn next(&self) -> Reads {
        self.ready.notified().await;
        std::mem::take(&mut *self.pending.lock().expect("read queue"))
    }
}

async fn read_lane(client: Client, reads: Arc<ReadQueue>, events: UnboundedSender<Event>) {
    while reads.next().await.run(&client, &events).await {}
}

/// Makes the changes one at a time. After one went through, the snapshot
/// holds its result: it is read once the window has been told, right away
/// or, after a refresh, once the sources have had a moment.
async fn change_lane(
    client: Client,
    mut inbox: UnboundedReceiver<Command>,
    reads: Arc<ReadQueue>,
    events: UnboundedSender<Event>,
) {
    while let Some(command) = inbox.recv().await {
        let (event, read_after) = match command {
            Command::Refresh => {
                let result = client.refresh().await.map_err(text);
                let read_after = result.is_ok().then_some(AFTER_REFRESH);
                (Event::Refreshed(result), read_after)
            }
            Command::SetMark { id, marked } => acted(client.set_mark(&id, marked).await),
            Command::Snooze { id, until } => acted(client.snooze(&id, until).await),
            Command::Wake { id } => acted(client.wake(&id).await),
            // The read lane makes these.
            Command::Poll | Command::Look | Command::Weather | Command::Version => continue,
        };
        if events.send(event).is_err() {
            return;
        }
        if let Some(wait) = read_after {
            let reads = reads.clone();
            tokio::spawn(async move {
                tokio::time::sleep(wait).await;
                reads.ask(Command::Poll);
            });
        }
    }
}

/// The event for a change, and when to read the snapshot after it.
fn acted(result: Result<(), Error>) -> (Event, Option<Duration>) {
    let read_after = result.is_ok().then_some(Duration::ZERO);
    (Event::Acted(result.map_err(text)), read_after)
}

fn text(error: Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// A directory of this test's own, so nothing lands in the real state
    /// directory; it holds the config file and is the fake `XDG_STATE_HOME`.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("tasks-pending-app-{}-{name}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }

        fn sample_config(&self) -> PathBuf {
            let config = self.dir.join("config.toml");
            std::fs::write(
                &config,
                "[[sources]]\nname = \"sample\"\nkind = \"sample\"\n",
            )
            .unwrap();
            config
        }

        /// An environment with only `XDG_STATE_HOME`, pointing here.
        fn env(&self) -> Env {
            let dir = self.dir.clone();
            Arc::new(move |key| (key == "XDG_STATE_HOME").then(|| dir.clone().into()))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Serves `router` on a loopback port and gives its URL.
    fn serve(runtime: &Runtime, router: Router) -> String {
        let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
        let address = listener.local_addr().unwrap();
        runtime.spawn(async move { serve_router(listener, router).await });
        format!("http://{address}")
    }

    /// What a stand-in daemon answers to a method and path: how long it
    /// waits, the status and the body.
    type Answer = fn(&str, &str) -> (Duration, u16, String);

    /// A stand-in daemon on a loopback port, recording every request as
    /// "METHOD path".
    struct Fake {
        url: String,
        hits: Arc<Mutex<Vec<String>>>,
    }

    impl Fake {
        fn start(answer: Answer) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let hits = Arc::new(Mutex::new(Vec::new()));
            let recorded = hits.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let hits = recorded.clone();
                    std::thread::spawn(move || answer_one(stream, answer, &hits));
                }
            });
            Self { url, hits }
        }

        fn client(&self) -> Client {
            Client::new(&self.url).unwrap()
        }

        fn count(&self, request: &str) -> usize {
            let hits = self.hits.lock().unwrap();
            hits.iter().filter(|hit| *hit == request).count()
        }
    }

    /// Reads one request from `stream` and answers it, then closes.
    fn answer_one(mut stream: std::net::TcpStream, answer: Answer, hits: &Mutex<Vec<String>>) {
        use std::io::{Read, Write};
        let mut data = Vec::new();
        let mut buffer = [0; 4096];
        let mut read = |data: &mut Vec<u8>| match stream.read(&mut buffer) {
            Ok(0) | Err(_) => false,
            Ok(count) => {
                data.extend_from_slice(&buffer[..count]);
                true
            }
        };
        let head = loop {
            if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                break end + 4;
            }
            if !read(&mut data) {
                return;
            }
        };
        let text = String::from_utf8_lossy(&data[..head]).into_owned();
        let length = text
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap_or(0))
            })
            .unwrap_or(0);
        while data.len() < head + length {
            if !read(&mut data) {
                return;
            }
        }
        let mut words = text.split_whitespace();
        let (method, path) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
        hits.lock().unwrap().push(format!("{method} {path}"));
        let (wait, status, body) = answer(method, path);
        std::thread::sleep(wait);
        let _ = write!(
            stream,
            "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
             content-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
    }

    fn snapshot_json() -> String {
        serde_json::to_string(&pending_core::sandbox_snapshot()).unwrap()
    }

    /// The next event, failing the test after a while instead of hanging.
    fn next(runtime: &Runtime, events: &mut UnboundedReceiver<Event>) -> Event {
        runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(10), events.recv()).await })
            .expect("an event in time")
            .expect("the worker is running")
    }

    /// A change goes through while a slow read still waits for its answer.
    #[test]
    fn a_change_does_not_wait_behind_a_slow_read() {
        let runtime = runtime();
        let fake = Fake::start(|method, path| match (method, path) {
            ("GET", "/api/v1/snapshot") => (Duration::from_secs(3), 200, snapshot_json()),
            _ => (Duration::ZERO, 200, "{}".to_owned()),
        });
        let (commands, mut events) = spawn_worker(&runtime, fake.client());
        commands.send(Command::Poll).unwrap();
        commands
            .send(Command::SetMark {
                id: "x".to_owned(),
                marked: true,
            })
            .unwrap();
        let first = next(&runtime, &mut events);
        assert!(matches!(first, Event::Acted(Ok(()))), "{first:?}");
    }

    /// Polls asked for while one is under way are made once, not queued:
    /// five in a row against a slow daemon are at most two requests.
    #[test]
    fn polls_asked_for_meanwhile_are_made_once() {
        let runtime = runtime();
        let fake = Fake::start(|method, path| match (method, path) {
            ("GET", "/api/v1/snapshot") => (Duration::from_millis(300), 200, snapshot_json()),
            _ => (Duration::ZERO, 500, String::new()),
        });
        let (commands, mut events) = spawn_worker(&runtime, fake.client());
        for _ in 0..5 {
            commands.send(Command::Poll).unwrap();
        }
        commands.send(Command::Version).unwrap();
        let mut snapshots = 0;
        loop {
            match next(&runtime, &mut events) {
                Event::Snapshot(Ok(_)) => snapshots += 1,
                Event::Update(None) => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(snapshots <= 2, "{snapshots} snapshots");
        assert_eq!(fake.count("GET /api/v1/snapshot"), snapshots);
    }

    /// An accepted refresh is followed, a moment later, by a read of the
    /// snapshot the sources are filling.
    #[test]
    fn reads_the_snapshot_after_a_refresh() {
        let runtime = runtime();
        let fake = Fake::start(|method, path| match (method, path) {
            ("GET", "/api/v1/snapshot") => (Duration::ZERO, 200, snapshot_json()),
            _ => (Duration::ZERO, 200, "{}".to_owned()),
        });
        let (commands, mut events) = spawn_worker(&runtime, fake.client());
        commands.send(Command::Refresh).unwrap();
        let refreshed = next(&runtime, &mut events);
        assert!(
            matches!(refreshed, Event::Refreshed(Ok(()))),
            "{refreshed:?}"
        );
        let read = next(&runtime, &mut events);
        assert!(matches!(read, Event::Snapshot(Ok(_))), "{read:?}");
    }

    /// A change the daemon refuses says why, and reads nothing: there is
    /// no new snapshot to show. A look that fails reads nothing either.
    #[test]
    fn a_refused_change_or_look_reads_nothing() {
        let runtime = runtime();
        let fake = Fake::start(|method, path| match (method, path) {
            ("GET", "/api/v1/snapshot") => (Duration::ZERO, 200, snapshot_json()),
            _ => (Duration::ZERO, 500, String::new()),
        });
        let (commands, mut events) = spawn_worker(&runtime, fake.client());
        commands
            .send(Command::SetMark {
                id: "x".to_owned(),
                marked: true,
            })
            .unwrap();
        let refused = next(&runtime, &mut events);
        assert!(
            matches!(&refused, Event::Acted(Err(error)) if error == "HTTP 500"),
            "{refused:?}"
        );
        commands.send(Command::Look).unwrap();
        commands.send(Command::Weather).unwrap();
        let weather = next(&runtime, &mut events);
        assert!(matches!(weather, Event::Weather(None)), "{weather:?}");
        assert_eq!(fake.count("POST /api/v1/look"), 1);
        assert_eq!(fake.count("GET /api/v1/snapshot"), 0);
    }

    /// An address where no daemon answers: every connection is closed
    /// unanswered. Kept by the test, unlike a port let go, which another
    /// test running alongside could take.
    fn no_daemon() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || listener.incoming().for_each(drop));
        url
    }

    /// The sandbox serves on a port of its own, and the worker answers a
    /// poll with its cards, and a mark, snooze or wake with the snapshot
    /// that shows it.
    #[test]
    fn the_sandbox_backend_answers_through_the_worker() {
        let runtime = runtime();
        let scratch = Scratch::new("sandbox");
        let backend = resolve(
            &runtime,
            Options {
                sandbox: true,
                ..Options::default()
            },
            scratch.env(),
        )
        .unwrap();
        assert!(matches!(backend.source, Source::Sandbox { .. }));
        assert!(backend.source.describe().contains("sandbox"));

        let (commands, mut events) = spawn_worker(&runtime, backend.client.clone());
        commands.send(Command::Poll).unwrap();
        let Some(Event::Snapshot(Ok(snapshot))) = runtime.block_on(events.recv()) else {
            panic!("expected a snapshot");
        };
        let id = snapshot.boards[0].groups[0].columns[0].cards[0].id.clone();

        commands
            .send(Command::SetMark {
                id: id.clone(),
                marked: true,
            })
            .unwrap();
        assert!(matches!(
            runtime.block_on(events.recv()),
            Some(Event::Acted(Ok(())))
        ));
        let Some(Event::Snapshot(Ok(snapshot))) = runtime.block_on(events.recv()) else {
            panic!("expected the snapshot after the mark");
        };
        assert_eq!(snapshot.marked, vec![id.clone()]);

        commands
            .send(Command::Snooze {
                id: id.clone(),
                until: None,
            })
            .unwrap();
        assert!(matches!(next(&runtime, &mut events), Event::Acted(Ok(()))));
        let Event::Snapshot(Ok(snapshot)) = next(&runtime, &mut events) else {
            panic!("expected the snapshot after the snooze");
        };
        assert!(snapshot.snoozed.iter().any(|entry| entry.card.id == id));

        commands.send(Command::Wake { id: id.clone() }).unwrap();
        assert!(matches!(next(&runtime, &mut events), Event::Acted(Ok(()))));
        let Event::Snapshot(Ok(snapshot)) = next(&runtime, &mut events) else {
            panic!("expected the snapshot after the wake");
        };
        assert!(snapshot.snoozed.is_empty());

        commands.send(Command::Weather).unwrap();
        let Some(Event::Weather(Some(report))) = runtime.block_on(events.recv()) else {
            panic!("expected the sample weather");
        };
        assert_eq!(report.place, "Recife");

        commands.send(Command::Version).unwrap();
        let update = next(&runtime, &mut events);
        assert!(matches!(update, Event::Update(_)), "{update:?}");
    }

    /// A daemon answering at the address is used as it is: nothing is
    /// started here.
    #[test]
    fn uses_the_daemon_that_answers() {
        let runtime = runtime();
        let scratch = Scratch::new("daemon");
        let url = serve(&runtime, sandbox_app());
        let backend = resolve(
            &runtime,
            Options {
                daemon: Some(url.clone()),
                ..Options::default()
            },
            scratch.env(),
        )
        .unwrap();
        let Source::Daemon {
            url: found,
            version,
        } = &backend.source
        else {
            panic!("expected the daemon, got {:?}", backend.source);
        };
        assert_eq!(found, &url);
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            backend.source.describe(),
            format!("daemon {version} at {url}")
        );
        assert!(runtime.block_on(backend.client.snapshot()).is_ok());
    }

    /// With nothing listening at the daemon address, the app runs the
    /// sources itself, from the config it is given, and says so.
    #[test]
    fn falls_back_to_the_built_in_sources() {
        let runtime = runtime();
        let scratch = Scratch::new("fallback");
        let backend = resolve(
            &runtime,
            Options {
                config: Some(scratch.sample_config()),
                daemon: Some(no_daemon()),
                ..Options::default()
            },
            scratch.env(),
        )
        .unwrap();
        assert!(
            matches!(
                backend.source,
                Source::BuiltIn {
                    daemon_wanted: true,
                    ..
                }
            ),
            "{:?}",
            backend.source
        );
        let line = backend.source.describe();
        assert!(
            line.starts_with("no daemon running; sources from "),
            "{line}"
        );
        let snapshot = runtime.block_on(backend.client.snapshot()).unwrap();
        assert!(!snapshot.sources.is_empty());
        assert!(
            scratch.dir.join("tasks-pending").is_dir(),
            "the cache goes where the environment says"
        );
    }

    /// Something at the daemon address that takes the connection but
    /// never answers holds the window up for a moment, not for a whole
    /// request timeout: the app gives up on it and runs the sources here.
    #[test]
    fn gives_up_soon_on_an_address_that_never_answers() {
        let runtime = runtime();
        let scratch = Scratch::new("silent");
        // Bound and never accepting: the system takes the connection and
        // nothing ever answers on it.
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let started = std::time::Instant::now();
        let backend = resolve(
            &runtime,
            Options {
                config: Some(scratch.sample_config()),
                daemon: Some(format!("http://{}", silent.local_addr().unwrap())),
                ..Options::default()
            },
            scratch.env(),
        )
        .unwrap();
        let waited = started.elapsed();
        assert!(
            matches!(
                backend.source,
                Source::BuiltIn {
                    daemon_wanted: true,
                    ..
                }
            ),
            "{:?}",
            backend.source
        );
        assert!(
            waited < pending_client::REQUEST_TIMEOUT,
            "waited {waited:?}"
        );
    }

    /// Asked not to look for a daemon, the app does not, even with one
    /// answering, and the status line says so.
    #[test]
    fn runs_the_sources_here_when_told_not_to_look() {
        let runtime = runtime();
        let scratch = Scratch::new("no-daemon");
        let backend = resolve(
            &runtime,
            Options {
                config: Some(scratch.sample_config()),
                daemon: Some(serve(&runtime, sandbox_app())),
                no_daemon: true,
                ..Options::default()
            },
            scratch.env(),
        )
        .unwrap();
        assert!(
            matches!(
                backend.source,
                Source::BuiltIn {
                    daemon_wanted: false,
                    ..
                }
            ),
            "{:?}",
            backend.source
        );
        let line = backend.source.describe();
        assert!(line.starts_with("without a daemon, as asked; "), "{line}");
    }

    /// Something that answers at the address but is not the daemon is an
    /// error, not a reason to start the sources: the address was wrong.
    #[test]
    fn refuses_an_address_that_answers_wrongly() {
        let runtime = runtime();
        let scratch = Scratch::new("wrong");
        let url = serve(&runtime, Router::new());
        let error = resolve(
            &runtime,
            Options {
                config: Some(scratch.sample_config()),
                daemon: Some(url.clone()),
                ..Options::default()
            },
            scratch.env(),
        )
        .err()
        .expect("a 404 at the daemon address is an error");
        let text = error.to_string();
        assert!(
            text.contains("not usable") && text.contains("404"),
            "{text}"
        );
    }
}
