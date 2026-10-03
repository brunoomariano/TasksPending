//! Weather for the web dashboard, as Omarchy reports it.
//!
//! A browser cannot run commands, so the API runs Omarchy's
//! `omarchy-weather-status` and `omarchy-weather-icon` and serves the result
//! at `GET /api/v1/weather`. Anywhere they are missing or fail (not on
//! Omarchy, no location, offline) the answer is `{"available": false}`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tokio::time::Instant;
use tracing::debug;

/// Prints `<place>  ·  Temp <temperature>  ·  Wind <wind>`, or fails.
const STATUS_COMMAND: &str = "omarchy-weather-status";
/// Prints one Nerd Font glyph for the current condition, or fails.
const ICON_COMMAND: &str = "omarchy-weather-icon";

/// Longest wait for one command; each makes its own request to wttr.in.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(6);
/// How long a weather report is served before the commands run again.
const REPORT_TTL: Duration = Duration::from_secs(10 * 60);
/// How long a failure is served; short, so the widget shows up soon after the
/// network does.
const FAILURE_TTL: Duration = Duration::from_secs(60);

/// What a finished command printed, or `None` when it failed.
pub type CommandOutput = Pin<Box<dyn Future<Output = Option<String>> + Send>>;
/// Runs one of the weather commands, named by its program, with no arguments.
pub type CommandRunner = Arc<dyn Fn(&'static str) -> CommandOutput + Send + Sync>;

/// The body of `GET /api/v1/weather`: `{"available": false}`, or
/// `{"available": true}` with the fields of [`Report`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Weather {
    available: bool,
    #[serde(flatten)]
    report: Option<Report>,
}

impl Weather {
    const UNAVAILABLE: Self = Self {
        available: false,
        report: None,
    };

    fn available(report: Report) -> Self {
        Self {
            available: true,
            report: Some(report),
        }
    }
}

/// The texts are shown as Omarchy formats them, e.g. `29°C` and `←15km/h`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Report {
    place: String,
    temperature: String,
    wind: String,
    condition: Condition,
}

/// The conditions `omarchy-weather-icon` tells apart, as stable keywords for
/// the page to pick an icon from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Condition {
    ClearDay,
    ClearNight,
    PartlyCloudyDay,
    PartlyCloudyNight,
    Cloudy,
    Fog,
    Rain,
    Sleet,
    Snow,
    Thunder,
    /// The icon command failed or printed a glyph this version doesn't know.
    Unknown,
}

/// The condition behind a glyph printed by `omarchy-weather-icon` (Nerd Font
/// weather icons). Omarchy draws showers and light snow differently by day
/// and by night; here they are just rain and snow.
fn condition_from_glyph(output: &str) -> Condition {
    match output.trim() {
        "\u{e30d}" => Condition::ClearDay,
        "\u{e32b}" => Condition::ClearNight,
        "\u{e302}" => Condition::PartlyCloudyDay,
        "\u{e32e}" => Condition::PartlyCloudyNight,
        // Also what Omarchy prints for a weather code it doesn't know.
        "\u{e33d}" => Condition::Cloudy,
        "\u{e313}" => Condition::Fog,
        "\u{e308}" | "\u{e333}" | "\u{e318}" => Condition::Rain,
        "\u{e3ad}" => Condition::Sleet,
        "\u{e30a}" | "\u{e327}" | "\u{e31a}" => Condition::Snow,
        "\u{e31d}" => Condition::Thunder,
        _ => Condition::Unknown,
    }
}

/// What `omarchy-weather-status` puts between the parts of its line.
const STATUS_SEPARATOR: &str = "  ·  ";
/// Longest place name shown, in characters.
const MAX_PLACE_CHARS: usize = 80;

/// Reads `<place>  ·  Temp <temperature>  ·  Wind <wind>` into its three
/// values; anything else is `None`. The values come from a web service, so
/// each must look like what it claims to be.
fn parse_status(line: &str) -> Option<(String, String, String)> {
    let line = line.strip_suffix('\n').unwrap_or(line);
    if line.chars().any(char::is_control) {
        return None;
    }
    // From the right, so a place may contain the separator.
    let mut parts = line.rsplitn(3, STATUS_SEPARATOR);
    let wind = parts.next()?.strip_prefix("Wind ")?.trim();
    let temperature = parts.next()?.strip_prefix("Temp ")?.trim();
    let place = parts.next()?.trim();

    let valid = !place.is_empty()
        && place.chars().count() <= MAX_PLACE_CHARS
        && is_temperature(temperature)
        && is_wind(wind);
    valid.then(|| (place.to_owned(), temperature.to_owned(), wind.to_owned()))
}

/// `29°C`, `-3°C`, `84°F`.
fn is_temperature(text: &str) -> bool {
    let number = text.strip_prefix(['-', '+']).unwrap_or(text);
    let Some((digits, unit)) = number.split_once('°') else {
        return false;
    };
    is_number(digits) && matches!(unit, "C" | "F")
}

/// `←15km/h`, `↓9mph`, `0km/h`: an optional direction arrow, the speed and
/// its unit.
fn is_wind(text: &str) -> bool {
    let speed = text.trim_start_matches(|c: char| !c.is_ascii());
    if text.len() - speed.len() > '←'.len_utf8() {
        return false;
    }
    let unit = speed.trim_start_matches(|c: char| c.is_ascii_digit());
    is_number(&speed[..speed.len() - unit.len()])
        && (1..=8).contains(&unit.len())
        && unit.chars().all(|c| c.is_ascii_alphabetic() || c == '/')
}

/// One to three digits.
fn is_number(text: &str) -> bool {
    (1..=3).contains(&text.len()) && text.chars().all(|c| c.is_ascii_digit())
}

struct Cached {
    weather: Weather,
    expires: Instant,
}

/// Answers the weather route from memory, running the commands only when the
/// last answer has expired.
#[derive(Clone)]
pub struct WeatherService {
    runner: CommandRunner,
    /// Held while the commands run, so concurrent requests wait for one run
    /// instead of each starting their own.
    cache: Arc<tokio::sync::Mutex<Option<Cached>>>,
}

impl WeatherService {
    /// Weather from the given command runner; tests pass a fake one.
    pub fn new(runner: CommandRunner) -> Self {
        Self {
            runner,
            cache: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Weather from the Omarchy commands found on `PATH`.
    pub fn omarchy() -> Self {
        Self::new(Arc::new(|program| Box::pin(run_command(program))))
    }

    /// A fixed report for the sandbox: no command runs, no network.
    pub fn sample() -> Self {
        Self::new(Arc::new(|program| {
            let output = match program {
                STATUS_COMMAND => Some("Recife  ·  Temp 29°C  ·  Wind ←15km/h\n".to_owned()),
                ICON_COMMAND => Some("\u{e302}\n".to_owned()),
                _ => None,
            };
            Box::pin(async move { output })
        }))
    }

    /// The current weather, from memory while it is fresh.
    pub async fn current(&self) -> Weather {
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.as_ref()
            && Instant::now() < cached.expires
        {
            return cached.weather.clone();
        }

        let weather = self.fetch().await;
        let ttl = if weather.available {
            REPORT_TTL
        } else {
            FAILURE_TTL
        };
        *cache = Some(Cached {
            weather: weather.clone(),
            expires: Instant::now() + ttl,
        });
        weather
    }

    /// Runs both commands at once and reads what they printed.
    async fn fetch(&self) -> Weather {
        let (status, icon) = tokio::join!(self.run(STATUS_COMMAND), self.run(ICON_COMMAND));
        let Some(status) = status else {
            debug!("weather unavailable: {STATUS_COMMAND} failed, timed out or is not installed");
            return Weather::UNAVAILABLE;
        };
        let Some((place, temperature, wind)) = parse_status(&status) else {
            debug!("weather unavailable: unexpected output from {STATUS_COMMAND}");
            return Weather::UNAVAILABLE;
        };
        Weather::available(Report {
            place,
            temperature,
            wind,
            condition: icon
                .as_deref()
                .map_or(Condition::Unknown, condition_from_glyph),
        })
    }

    async fn run(&self, program: &'static str) -> Option<String> {
        tokio::time::timeout(COMMAND_TIMEOUT, (self.runner)(program))
            .await
            .ok()
            .flatten()
    }
}

/// Runs `program` with no arguments and returns what it printed, or `None`
/// when it is missing or exits with an error. It is spawned directly, never
/// through a shell.
async fn run_command(program: &'static str) -> Option<String> {
    let output = tokio::process::Command::new(program)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // Dropped on timeout: don't leave the script behind.
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// The weather route, to merge into the API router.
pub fn routes<S>(service: WeatherService) -> Router<S> {
    Router::new()
        .route("/api/v1/weather", get(weather))
        .with_state(service)
}

async fn weather(State(service): State<WeatherService>) -> Json<Weather> {
    Json(service.current().await)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use pending_runtime::Aggregator;
    use tower::ServiceExt;

    use super::*;

    const STATUS_LINE: &str = "Recife  ·  Temp 29°C  ·  Wind ←15km/h\n";

    /// A runner that answers from fixed outputs and records every program it
    /// was asked to run.
    fn fake(
        status: Option<&'static str>,
        icon: Option<&'static str>,
    ) -> (WeatherService, Arc<Mutex<Vec<&'static str>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let service = WeatherService::new(Arc::new(move |program| {
            seen.lock().unwrap().push(program);
            let output = match program {
                STATUS_COMMAND => status,
                ICON_COMMAND => icon,
                other => panic!("unexpected command {other}"),
            };
            Box::pin(async move { output.map(str::to_owned) })
        }));
        (service, calls)
    }

    fn api(service: WeatherService) -> Router {
        crate::app_with_weather(
            Aggregator::start(Vec::new(), Duration::from_secs(30)),
            None,
            service,
        )
    }

    async fn get_weather(app: Router) -> (StatusCode, serde_json::Value) {
        let response = app
            .oneshot(Request::get("/api/v1/weather").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    /// On Omarchy with weather working, the route serves the place,
    /// temperature and wind from the status line and the condition from the
    /// icon, running each command once with no arguments.
    #[tokio::test]
    async fn serves_the_omarchy_weather_as_json() {
        let (service, calls) = fake(Some(STATUS_LINE), Some("\u{e30d}\n"));

        let (status, body) = get_weather(api(service)).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            serde_json::json!({
                "available": true,
                "place": "Recife",
                "temperature": "29°C",
                "wind": "←15km/h",
                "condition": "clear-day",
            })
        );
        let mut calls = calls.lock().unwrap().clone();
        calls.sort_unstable();
        assert_eq!(calls, [ICON_COMMAND, STATUS_COMMAND]);
    }

    /// Off Omarchy, without a location or offline, the status command fails
    /// and the route says only that weather is unavailable.
    #[tokio::test]
    async fn a_failing_status_command_means_unavailable() {
        let (service, _) = fake(None, Some("\u{e30d}\n"));

        let (status, body) = get_weather(api(service)).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::json!({ "available": false }));
    }

    /// A status line that isn't in Omarchy's format (an error page from the
    /// weather service, missing parts, control characters, oversized text) is
    /// treated as no weather rather than shown.
    #[test]
    fn unexpected_status_lines_are_rejected() {
        for line in [
            "",
            "Weather unavailable",
            "Recife",
            "Recife  ·  Temp 29°C",
            "Recife  ·  29°C  ·  ←15km/h",
            "  ·  Temp 29°C  ·  Wind ←15km/h",
            "Recife  ·  Temp   ·  Wind ←15km/h",
            "Recife  ·  Temp 29°C  ·  Wind ",
            "Recife  ·  Temp Unknown location  ·  Wind ←15km/h",
            "Recife  ·  Temp 29°C  ·  Wind strong",
            "Recife\u{1b}[31m  ·  Temp 29°C  ·  Wind ←15km/h",
            "Recife  ·  Temp 29°C  ·  Wind ←15km/h\nsecond line",
        ] {
            assert_eq!(parse_status(line), None, "{line:?}");
        }
        let long_place = format!("{}  ·  Temp 29°C  ·  Wind ←15km/h", "x".repeat(200));
        assert_eq!(parse_status(&long_place), None);
    }

    /// The status line is read as Omarchy writes it: negative and Fahrenheit
    /// temperatures, mph winds and places with spaces or a middle dot.
    #[test]
    fn status_lines_are_parsed_as_omarchy_writes_them() {
        let parsed = |line| {
            let (place, temperature, wind) = parse_status(line).unwrap();
            [place, temperature, wind]
        };

        assert_eq!(
            parsed("Recife  ·  Temp 29°C  ·  Wind ←15km/h\n"),
            ["Recife", "29°C", "←15km/h"]
        );
        assert_eq!(
            parsed("São Paulo  ·  Temp -3°C  ·  Wind ↗4km/h"),
            ["São Paulo", "-3°C", "↗4km/h"]
        );
        assert_eq!(
            parsed("New York  ·  Temp 84°F  ·  Wind ↓9mph"),
            ["New York", "84°F", "↓9mph"]
        );
        assert_eq!(
            parsed("A · B  ·  Temp 0°C  ·  Wind 0km/h"),
            ["A · B", "0°C", "0km/h"]
        );
    }

    /// Each glyph Omarchy prints maps to one condition keyword; day and night
    /// variants of showers and light snow collapse into rain and snow.
    #[test]
    fn glyphs_map_to_condition_keywords() {
        for (glyph, condition) in [
            ("\u{e30d}", Condition::ClearDay),
            ("\u{e32b}", Condition::ClearNight),
            ("\u{e302}", Condition::PartlyCloudyDay),
            ("\u{e32e}", Condition::PartlyCloudyNight),
            ("\u{e33d}", Condition::Cloudy),
            ("\u{e313}", Condition::Fog),
            ("\u{e308}", Condition::Rain),
            ("\u{e333}", Condition::Rain),
            ("\u{e318}", Condition::Rain),
            ("\u{e3ad}", Condition::Sleet),
            ("\u{e30a}", Condition::Snow),
            ("\u{e327}", Condition::Snow),
            ("\u{e31a}", Condition::Snow),
            ("\u{e31d}", Condition::Thunder),
            ("?", Condition::Unknown),
            ("", Condition::Unknown),
        ] {
            assert_eq!(condition_from_glyph(glyph), condition, "{glyph:?}");
            assert_eq!(condition_from_glyph(&format!("{glyph}\n")), condition);
        }
        assert_eq!(
            serde_json::to_value(Condition::PartlyCloudyNight).unwrap(),
            "partly-cloudy-night"
        );
    }

    /// The icon is optional: when its command fails or prints something new,
    /// the weather is still served, with an unknown condition.
    #[tokio::test]
    async fn a_failing_icon_command_keeps_the_weather() {
        for icon in [None, Some("\u{f000}\n")] {
            let (service, _) = fake(Some(STATUS_LINE), icon);

            let (_, body) = get_weather(api(service)).await;

            assert_eq!(body["available"], true, "{icon:?}");
            assert_eq!(body["temperature"], "29°C");
            assert_eq!(body["condition"], "unknown");
        }
    }

    /// Page polls don't spawn processes: a report is served from memory for
    /// ten minutes, and only then do the commands run again.
    #[tokio::test(start_paused = true)]
    async fn a_report_is_cached_for_ten_minutes() {
        let (service, calls) = fake(Some(STATUS_LINE), Some("\u{e30d}\n"));
        let app = api(service);

        get_weather(app.clone()).await;
        tokio::time::advance(Duration::from_secs(9 * 60 + 59)).await;
        let (_, body) = get_weather(app.clone()).await;
        assert_eq!(body["available"], true);
        assert_eq!(calls.lock().unwrap().len(), 2);

        tokio::time::advance(Duration::from_secs(1)).await;
        get_weather(app).await;
        assert_eq!(calls.lock().unwrap().len(), 4);
    }

    /// A failure is remembered only for a minute, so the widget appears soon
    /// after the network comes back, without retrying on every request.
    #[tokio::test(start_paused = true)]
    async fn a_failure_is_cached_for_one_minute() {
        let (service, calls) = fake(None, None);
        let app = api(service);

        get_weather(app.clone()).await;
        tokio::time::advance(Duration::from_secs(59)).await;
        get_weather(app.clone()).await;
        assert_eq!(calls.lock().unwrap().len(), 2);

        tokio::time::advance(Duration::from_secs(1)).await;
        get_weather(app).await;
        assert_eq!(calls.lock().unwrap().len(), 4);
    }

    /// Requests arriving while the commands run wait for that run instead of
    /// starting their own.
    #[tokio::test(start_paused = true)]
    async fn concurrent_requests_share_one_run() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let service = WeatherService::new(Arc::new(move |program| {
            seen.lock().unwrap().push(program);
            Box::pin(async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                (program == STATUS_COMMAND).then(|| STATUS_LINE.to_owned())
            })
        }));

        let (first, second) = tokio::join!(service.current(), service.current());

        assert_eq!(first, second);
        assert!(first.available);
        assert_eq!(calls.lock().unwrap().len(), 2);
    }

    /// A command that hangs (a stalled network) is given up on after six
    /// seconds and the weather is unavailable.
    #[tokio::test(start_paused = true)]
    async fn a_hanging_command_times_out() {
        let service = WeatherService::new(Arc::new(|_| {
            Box::pin(std::future::pending::<Option<String>>())
        }));
        let started = Instant::now();

        let weather = service.current().await;

        assert_eq!(weather, Weather::UNAVAILABLE);
        assert_eq!(started.elapsed(), COMMAND_TIMEOUT);
    }

    /// The sandbox serves a fixed report, so screenshots and demos show the
    /// widget without Omarchy or network access.
    #[tokio::test]
    async fn the_sandbox_serves_a_fixed_sample() {
        let (_, body) = get_weather(api(WeatherService::sample())).await;

        assert_eq!(
            body,
            serde_json::json!({
                "available": true,
                "place": "Recife",
                "temperature": "29°C",
                "wind": "←15km/h",
                "condition": "partly-cloudy-day",
            })
        );
    }

    /// The weather route is local-only like the others: a site pointing its
    /// own domain at 127.0.0.1 (DNS rebinding) gets 421, not the user's city.
    #[tokio::test]
    async fn only_local_host_names_get_the_weather() {
        let (service, calls) = fake(Some(STATUS_LINE), None);
        let app = api(service);
        let status = |host: &'static str| {
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::get("/api/v1/weather")
                        .header("host", host)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
            }
        };

        assert_eq!(
            status("evil.example:61000").await,
            StatusCode::MISDIRECTED_REQUEST
        );
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(status("127.0.0.1:61000").await, StatusCode::OK);
    }

    /// A program that isn't installed (any system other than Omarchy) is a
    /// plain failure, not an error or a panic.
    #[tokio::test]
    async fn a_missing_program_is_a_failure() {
        assert_eq!(
            run_command("tasks-pending-test-no-such-weather-command").await,
            None
        );
    }
}
