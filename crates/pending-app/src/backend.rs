//! Where the dashboard comes from: the daemon when one is running, the
//! same API started inside the app when none is, or the sandbox. Whatever
//! the origin, the window talks to it the same way, over the loopback
//! client, from a worker on the async runtime.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use pending_api::{LiveServer, Router, sandbox_app, serve_router, start_live};
use pending_client::{Client, DEFAULT_DAEMON_URL, Error};
use pending_core::{DashboardSnapshot, WeatherReport};
use pending_runtime::config::Origin;
use pending_runtime::live::Watch;
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::info;

/// Sources answer a refresh in the background; read the result shortly
/// after.
const AFTER_REFRESH: Duration = Duration::from_secs(2);

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
    /// No daemon answered: the sources run inside the app, from `origin`.
    BuiltIn { origin: Origin, address: SocketAddr },
    /// Simulated cards.
    Sandbox { address: SocketAddr },
}

impl Source {
    /// One line for the window's status.
    pub fn describe(&self) -> String {
        match self {
            Self::Daemon { url, version } => format!("daemon {version} at {url}"),
            Self::BuiltIn {
                origin: Origin::File(path),
                address,
            } => format!(
                "no daemon running; sources from {} (built-in API at {address})",
                path.display()
            ),
            Self::BuiltIn {
                origin: Origin::SampleDefault { .. },
                address,
            } => format!(
                "no daemon running and no config; sample source (built-in API at {address})"
            ),
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
/// sources started here. Blocks on `runtime` while doing so.
pub fn resolve(runtime: &Runtime, options: Options) -> anyhow::Result<Backend> {
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
            let client = Client::new(&url).with_context(|| format!("daemon address {url}"))?;
            match client.health().await {
                Ok(health) => {
                    info!(url, version = health.version, "using the daemon");
                    return Ok(Backend {
                        client,
                        source: Source::Daemon {
                            url,
                            version: health.version,
                        },
                        _watch: None,
                    });
                }
                Err(Error::Transport(error)) => {
                    info!(url, error, "no daemon answered; starting the sources here")
                }
                Err(error) => anyhow::bail!("the daemon at {url} is not usable: {error}"),
            }
        }
        let LiveServer {
            router,
            origin,
            watch,
        } = start_live(options.config)?;
        let (client, address) = serve_here(router).await?;
        Ok(Backend {
            client,
            source: Source::BuiltIn { origin, address },
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

/// Runs the window's commands against `client`, one at a time, on the
/// runtime, and reports back through the returned receiver.
pub fn spawn_worker(
    runtime: &Runtime,
    client: Client,
) -> (UnboundedSender<Command>, UnboundedReceiver<Event>) {
    let (commands, mut inbox) = unbounded_channel::<Command>();
    let (events, outbox) = unbounded_channel::<Event>();
    let later = commands.clone();
    runtime.spawn(async move {
        while let Some(command) = inbox.recv().await {
            let event = match command {
                Command::Poll => Event::Snapshot(client.snapshot().await.map_err(text)),
                Command::Look => {
                    if client.look().await.is_err() {
                        continue;
                    }
                    Event::Snapshot(client.snapshot().await.map_err(text))
                }
                Command::Weather => Event::Weather(client.weather().await.ok().flatten()),
                Command::Version => {
                    Event::Update(client.version().await.ok().and_then(|v| v.available))
                }
                Command::Refresh => {
                    let result = client.refresh().await.map_err(text);
                    if result.is_ok() {
                        let later = later.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(AFTER_REFRESH).await;
                            let _ = later.send(Command::Poll);
                        });
                    }
                    Event::Refreshed(result)
                }
                Command::SetMark { id, marked } => {
                    acted(&later, client.set_mark(&id, marked).await)
                }
                Command::Snooze { id, until } => acted(&later, client.snooze(&id, until).await),
                Command::Wake { id } => acted(&later, client.wake(&id).await),
            };
            if events.send(event).is_err() {
                break;
            }
        }
    });
    (commands, outbox)
}

/// After a change went through, the snapshot holds the result: read it.
fn acted(later: &UnboundedSender<Command>, result: Result<(), Error>) -> Event {
    if result.is_ok() {
        let _ = later.send(Command::Poll);
    }
    Event::Acted(result.map_err(text))
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

    /// The sandbox serves on a port of its own, and the worker answers a
    /// poll with its cards and a mark with the snapshot that shows it.
    #[test]
    fn the_sandbox_backend_answers_through_the_worker() {
        let runtime = runtime();
        let backend = resolve(
            &runtime,
            Options {
                sandbox: true,
                ..Options::default()
            },
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
        assert_eq!(snapshot.marked, vec![id]);

        commands.send(Command::Weather).unwrap();
        let Some(Event::Weather(Some(report))) = runtime.block_on(events.recv()) else {
            panic!("expected the sample weather");
        };
        assert_eq!(report.place, "Recife");
    }

    /// With nothing listening at the daemon address, the app runs the
    /// sources itself, from the config it is given, and says so.
    #[test]
    fn falls_back_to_the_built_in_sources() {
        let runtime = runtime();
        let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
        let free = listener.local_addr().unwrap();
        drop(listener);
        let config = std::env::temp_dir().join(format!(
            "tasks-pending-app-{}-config.toml",
            std::process::id()
        ));
        std::fs::write(
            &config,
            "[[sources]]\nname = \"sample\"\nkind = \"sample\"\n",
        )
        .unwrap();
        let backend = resolve(
            &runtime,
            Options {
                config: Some(config.clone()),
                daemon: Some(format!("http://{free}")),
                ..Options::default()
            },
        )
        .unwrap();
        let _ = std::fs::remove_file(&config);
        assert!(
            matches!(backend.source, Source::BuiltIn { .. }),
            "{:?}",
            backend.source
        );
        assert!(backend.source.describe().contains("no daemon"));
        let snapshot = runtime.block_on(backend.client.snapshot()).unwrap();
        assert!(!snapshot.sources.is_empty());
    }
}
