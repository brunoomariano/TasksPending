//! Calendar events from an iCal feed (such as Google Calendar's secret iCal
//! address) as pending cards: what is happening now and what comes next.

use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Days, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use pending_core::{
    BoxFuture, CardSeverity, PendingCard, PendingSource, SourceBatch, SourceError, SourceItem,
};
use reqwest::Client;
use serde::Deserialize;

/// Environment variable holding the feed URL. The URL is a secret: anyone with
/// it can read the calendar.
pub const URL_ENV: &str = "TASKS_PENDING_ICAL_URL";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Events starting within this are shown as warnings.
const IMMINENT: chrono::Duration = chrono::Duration::hours(1);
/// Upper bound of occurrences expanded per recurring event.
const MAX_EXPANDED: u16 = 500;

/// Which events count as pending: those not yet over, starting within `days`,
/// at most `max_events` of them.
#[derive(Debug, Clone, Copy)]
pub struct Window {
    pub max_events: usize,
    pub days: u64,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            max_events: 25,
            days: 30,
        }
    }
}

/// A time bucket of the window; `occurrences` names its columns after these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum When {
    /// In progress.
    Now,
    Today,
    Tomorrow,
    /// Later within the window.
    Later,
}

impl When {
    const ALL: [When; 4] = [When::Now, When::Today, When::Tomorrow, When::Later];

    /// The column name `occurrences` uses for this bucket.
    fn default_name(self) -> &'static str {
        match self {
            When::Now => "Now",
            When::Today => "Today",
            When::Tomorrow => "Tomorrow",
            When::Later => "Next 30 days",
        }
    }
}

/// One column: the events falling in some of the time buckets.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IcalColumn {
    #[serde(skip)]
    pub name: String,
    pub when: Vec<When>,
}

/// Columns used when the configuration declares none: one per bucket.
pub fn default_columns() -> Vec<IcalColumn> {
    When::ALL
        .into_iter()
        .map(|when| IcalColumn {
            name: when.default_name().to_owned(),
            when: vec![when],
        })
        .collect()
}

/// Moves each occurrence from its time bucket (as named by [`occurrences`]) to
/// every column that lists that bucket.
pub fn assign_columns(columns: &[IcalColumn], batch: SourceBatch) -> SourceBatch {
    let items = batch
        .items
        .into_iter()
        .flat_map(|item| {
            let bucket = When::ALL
                .into_iter()
                .find(|when| when.default_name() == item.column);
            columns
                .iter()
                .filter(|column| bucket.is_some_and(|b| column.when.contains(&b)))
                .map(|column| SourceItem {
                    column: column.name.clone(),
                    card: item.card.clone(),
                })
                .collect::<Vec<_>>()
        })
        .collect();
    SourceBatch {
        items,
        warnings: batch.warnings,
    }
}

pub struct IcalSource {
    client: Client,
    url: Result<String, String>,
    window: Window,
    columns: Vec<IcalColumn>,
}

impl IcalSource {
    /// Reads the feed URL from [`URL_ENV`].
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Self {
        let url = env(URL_ENV)
            .map(|url| url.trim().to_owned())
            .filter(|url| !url.is_empty())
            .ok_or_else(|| {
                format!(
                    "calendar is not configured: set {URL_ENV} to the iCal address, then restart"
                )
            });
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            client,
            url,
            window: Window::default(),
            columns: default_columns(),
        }
    }

    /// Replaces the default columns; an empty list keeps the defaults.
    pub fn with_columns(mut self, columns: Vec<IcalColumn>) -> Self {
        if !columns.is_empty() {
            self.columns = columns;
        }
        self
    }

    async fn fetch(&self, url: &str) -> Result<SourceBatch, String> {
        let response = self
            .client
            .get(url)
            .header(
                "user-agent",
                concat!("tasks-pending/", env!("CARGO_PKG_VERSION")),
            )
            .send()
            .await
            .map_err(describe)?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("calendar feed returned {status}"));
        }
        let text = response.text().await.map_err(describe)?;
        occurrences(&text, Utc::now(), self.window, local_zone(), Some(url))
            .map(|batch| assign_columns(&self.columns, batch))
    }
}

impl PendingSource for IcalSource {
    fn columns(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }

    fn refresh(&self) -> BoxFuture<'_, Result<SourceBatch, SourceError>> {
        Box::pin(async move {
            match &self.url {
                Ok(url) => self.fetch(url).await.map_err(SourceError::new),
                Err(message) => Err(SourceError::new(message.clone())),
            }
        })
    }
}

/// The machine's time zone, for day boundaries and displayed times.
fn local_zone() -> Tz {
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|name| name.parse().ok())
        .unwrap_or(Tz::UTC)
}

/// Cards for the events of `ics` inside `window`, as seen at `now` in `zone`.
/// `feed_url` only decides the link of cards without their own URL (Google
/// feeds link to the day in Google Calendar); it is never shown.
pub fn occurrences(
    ics: &str,
    now: DateTime<Utc>,
    window: Window,
    zone: Tz,
    feed_url: Option<&str>,
) -> Result<SourceBatch, String> {
    if !ics.trim_start().starts_with("BEGIN:VCALENDAR") {
        return Err("calendar feed is not an iCal file".to_owned());
    }
    let events = parse_events(ics);
    let end = now + chrono::Duration::days(window.days as i64);
    let google = feed_url.is_some_and(|url| url.contains("calendar.google.com"));

    let mut unreadable = 0usize;
    // Instances replaced by a RECURRENCE-ID override (moved or cancelled).
    let mut overridden: HashMap<(String, DateTime<Utc>), ()> = HashMap::new();
    for event in &events {
        if let Some(recurrence_id) = &event.recurrence_id {
            match parse_time(recurrence_id, zone) {
                Some((at, _)) => {
                    overridden.insert((event.uid.clone(), at), ());
                }
                None => unreadable += 1,
            }
        }
    }

    let mut found = Vec::new();
    for event in &events {
        if event.cancelled() {
            continue;
        }
        // Counted above; showing it too would duplicate the original instance.
        if event
            .recurrence_id
            .as_ref()
            .is_some_and(|id| parse_time(id, zone).is_none())
        {
            continue;
        }
        let Some((start, all_day)) = parse_time(event.start_prop(), zone) else {
            unreadable += 1;
            continue;
        };
        let duration = event.duration(start, all_day, zone);
        // All-day events span whole local days, whatever their length in hours.
        let days = ((duration.num_minutes() as f64) / (24.0 * 60.0))
            .round()
            .max(1.0) as u64;

        let starts = if event.recurrence_id.is_none() && event.recurs() {
            // A day of slack: a day longer than `duration` (DST) must still
            // be expanded; finished occurrences are dropped below.
            match expand(event, zone, now - duration - chrono::Duration::days(1), end) {
                Some(starts) => starts
                    .into_iter()
                    .filter(|at| !overridden.contains_key(&(event.uid.clone(), *at)))
                    .collect(),
                None => {
                    unreadable += 1;
                    continue;
                }
            }
        } else {
            vec![start]
        };

        for start in starts {
            let finish = if all_day {
                start
                    .with_timezone(&zone)
                    .date_naive()
                    .checked_add_days(Days::new(days))
                    .and_then(|day| local_midnight(day, zone))
                    .unwrap_or(start + duration)
            } else {
                start + duration
            };
            let not_over = finish > now || (duration.is_zero() && start >= now);
            if not_over && start < end {
                found.push((start, finish, all_day, event));
            }
        }
    }

    found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.3.uid.cmp(&b.3.uid)));
    found.truncate(window.max_events);

    let today = now.with_timezone(&zone).date_naive();
    let items = found
        .into_iter()
        .map(|(start, finish, all_day, event)| {
            card(event, start, finish, all_day, now, today, zone, google)
        })
        .collect();

    let mut warnings = Vec::new();
    if unreadable > 0 {
        warnings.push(format!(
            "{unreadable} calendar entries could not be read (unknown time zone or recurrence)"
        ));
    }
    Ok(SourceBatch { items, warnings })
}

#[allow(clippy::too_many_arguments)]
fn card(
    event: &Event,
    start: DateTime<Utc>,
    finish: DateTime<Utc>,
    all_day: bool,
    now: DateTime<Utc>,
    today: NaiveDate,
    zone: Tz,
    google: bool,
) -> SourceItem {
    let local_start = start.with_timezone(&zone);
    let day = local_start.date_naive();
    let section = if all_day {
        if day <= today {
            "Today"
        } else {
            day_section(day, today)
        }
    } else if start <= now {
        "Now"
    } else {
        day_section(day, today)
    };

    let mut body = if all_day {
        format!("{} · all day", local_start.format("%a %d %b"))
    } else if finish == start {
        local_start.format("%a %d %b %H:%M").to_string()
    } else {
        format!(
            "{}–{}",
            local_start.format("%a %d %b %H:%M"),
            finish.with_timezone(&zone).format("%H:%M")
        )
    };
    if let Some(location) = event.location.as_deref().filter(|l| !l.is_empty()) {
        body.push_str(&format!(" · {location}"));
    }

    let url = event
        .url
        .clone()
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
        .or_else(|| {
            google.then(|| {
                format!(
                    "https://calendar.google.com/calendar/r/day/{}",
                    day.format("%Y/%-m/%-d")
                )
            })
        });

    SourceItem {
        column: section.to_owned(),
        card: PendingCard {
            id: format!("ical:{}:{}", event.uid, start.to_rfc3339()),
            title: event
                .summary
                .clone()
                .unwrap_or_else(|| "(no title)".to_owned()),
            body,
            source: "calendar".to_owned(),
            url,
            due_at: Some(start),
            severity: if !all_day && start <= now + IMMINENT {
                CardSeverity::Warning
            } else {
                CardSeverity::Info
            },
            updated_at: event
                .last_modified
                .as_ref()
                .and_then(|prop| parse_time(prop, zone))
                .map_or(now, |(at, _)| at),
        },
    }
}

fn day_section(day: NaiveDate, today: NaiveDate) -> &'static str {
    if day == today {
        "Today"
    } else if Some(day) == today.checked_add_days(Days::new(1)) {
        "Tomorrow"
    } else {
        "Next 30 days"
    }
}

/// One content line, after unfolding.
#[derive(Debug, Clone)]
struct Prop {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Prop {
    fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn text(&self) -> String {
        let mut out = String::new();
        let mut chars = self.value.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n' | 'N') => out.push('\n'),
                    Some(other) => out.push(other),
                    None => {}
                }
            } else {
                out.push(c);
            }
        }
        out
    }
}

#[derive(Debug, Default)]
struct Event {
    uid: String,
    summary: Option<String>,
    location: Option<String>,
    url: Option<String>,
    status: Option<String>,
    dtstart: Option<Prop>,
    dtend: Option<Prop>,
    duration: Option<String>,
    recurrence_id: Option<Prop>,
    last_modified: Option<Prop>,
    /// RRULE, RDATE and EXDATE lines, kept for expansion.
    recurrence: Vec<Prop>,
}

impl Event {
    fn cancelled(&self) -> bool {
        self.status
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("CANCELLED"))
    }

    fn recurs(&self) -> bool {
        self.recurrence
            .iter()
            .any(|p| p.name == "RRULE" || p.name == "RDATE")
    }

    fn duration(&self, start: DateTime<Utc>, all_day: bool, zone: Tz) -> chrono::Duration {
        if let Some((end, _)) = self.dtend.as_ref().and_then(|p| parse_time(p, zone)) {
            return (end - start).max(chrono::Duration::zero());
        }
        if let Some(duration) = self.duration.as_deref().and_then(parse_duration) {
            return duration;
        }
        if all_day {
            chrono::Duration::days(1)
        } else {
            chrono::Duration::zero()
        }
    }
}

fn parse_events(ics: &str) -> Vec<Event> {
    let mut events = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut current: Option<Event> = None;

    for line in unfold(ics) {
        let Some(prop) = parse_prop(&line) else {
            continue;
        };
        match prop.name.as_str() {
            "BEGIN" => {
                if prop.value.eq_ignore_ascii_case("VEVENT")
                    && stack.last().is_some_and(|c| c == "VCALENDAR")
                {
                    current = Some(Event::default());
                }
                stack.push(prop.value.to_ascii_uppercase());
                continue;
            }
            "END" => {
                stack.pop();
                if prop.value.eq_ignore_ascii_case("VEVENT")
                    && let Some(event) = current.take().filter(|e| e.dtstart.is_some())
                {
                    events.push(event);
                }
                continue;
            }
            _ => {}
        }
        // Only properties of the event itself, not of nested VALARMs.
        if stack.last().map(String::as_str) != Some("VEVENT") {
            continue;
        }
        let Some(event) = current.as_mut() else {
            continue;
        };
        match prop.name.as_str() {
            "UID" => event.uid = prop.value.clone(),
            "SUMMARY" => event.summary = Some(prop.text()),
            "LOCATION" => event.location = Some(prop.text()),
            "URL" => event.url = Some(prop.value.clone()),
            "STATUS" => event.status = Some(prop.value.clone()),
            "DTSTART" => event.dtstart = Some(prop),
            "DTEND" => event.dtend = Some(prop),
            "DURATION" => event.duration = Some(prop.value.clone()),
            "RECURRENCE-ID" => event.recurrence_id = Some(prop),
            "LAST-MODIFIED" => event.last_modified = Some(prop),
            "RRULE" | "RDATE" | "EXDATE" => event.recurrence.push(prop),
            _ => {}
        }
    }
    events
}

impl Event {
    fn start_prop(&self) -> &Prop {
        self.dtstart
            .as_ref()
            .expect("parse_events keeps only events with DTSTART")
    }
}

/// Joins continuation lines (starting with a space or tab) to the line above.
fn unfold(ics: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in ics.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        match raw.chars().next() {
            Some(' ' | '\t') if !lines.is_empty() => {
                lines.last_mut().expect("checked").push_str(&raw[1..]);
            }
            _ if raw.is_empty() => {}
            _ => lines.push(raw.to_owned()),
        }
    }
    lines
}

/// `NAME;PARAM=value;PARAM="quoted:value":VALUE`.
fn parse_prop(line: &str) -> Option<Prop> {
    let mut in_quotes = false;
    let colon = line.char_indices().find_map(|(i, c)| match c {
        '"' => {
            in_quotes = !in_quotes;
            None
        }
        ':' if !in_quotes => Some(i),
        _ => None,
    })?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.trim().to_ascii_uppercase();
    let params = parts
        .filter_map(|param| {
            let (key, value) = param.split_once('=')?;
            Some((key.to_ascii_uppercase(), value.trim_matches('"').to_owned()))
        })
        .collect();
    Some(Prop {
        name,
        params,
        value: value.to_owned(),
    })
}

/// A DATE or DATE-TIME property as an instant; `true` for all-day dates.
/// Floating times and dates use `zone`.
fn parse_time(prop: &Prop, zone: Tz) -> Option<(DateTime<Utc>, bool)> {
    let value = prop.value.trim();
    if prop
        .param("VALUE")
        .is_some_and(|v| v.eq_ignore_ascii_case("DATE"))
        || value.len() == 8
    {
        let date = NaiveDate::parse_from_str(value, "%Y%m%d").ok()?;
        return Some((local_midnight(date, zone)?, true));
    }
    if let Some(utc) = value.strip_suffix('Z') {
        let naive = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        return Some((naive.and_utc(), false));
    }
    let naive = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()?;
    let tz = match prop.param("TZID") {
        Some(name) => Tz::from_str(name).ok()?,
        None => zone,
    };
    let local = tz.from_local_datetime(&naive).earliest()?;
    Some((local.with_timezone(&Utc), false))
}

/// The first instant of `date` in `zone`. On days whose midnight does not
/// exist (clocks jump forward at 00:00), the first minute that does.
fn local_midnight(date: NaiveDate, zone: Tz) -> Option<DateTime<Utc>> {
    first_valid(date.and_hms_opt(0, 0, 0)?, zone)
}

/// `local` in `zone`, or the first minute after it that exists when clocks
/// jump forward over it.
fn first_valid(local: NaiveDateTime, zone: Tz) -> Option<DateTime<Utc>> {
    (0..=120).find_map(|minute| {
        zone.from_local_datetime(&(local + chrono::Duration::minutes(minute)))
            .earliest()
            .map(|at| at.with_timezone(&Utc))
    })
}

/// `P1D`, `PT1H30M`, `P1W`, `-PT15M` (negative durations count as zero).
fn parse_duration(value: &str) -> Option<chrono::Duration> {
    let value = value.trim();
    if value.starts_with('-') {
        return Some(chrono::Duration::zero());
    }
    let rest = value.strip_prefix('+').unwrap_or(value).strip_prefix('P')?;
    let mut total = chrono::Duration::zero();
    let mut number = String::new();
    let mut in_time = false;
    for c in rest.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => number.push(c),
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                total += match (unit, in_time) {
                    ('W', false) => chrono::Duration::weeks(n),
                    ('D', false) => chrono::Duration::days(n),
                    ('H', true) => chrono::Duration::hours(n),
                    ('M', true) => chrono::Duration::minutes(n),
                    ('S', true) => chrono::Duration::seconds(n),
                    _ => return None,
                };
            }
        }
    }
    Some(total)
}

/// Starts of a recurring event between `after` and `before`, or `None` when
/// its rules cannot be read.
fn expand(
    event: &Event,
    zone: Tz,
    after: DateTime<Utc>,
    before: DateTime<Utc>,
) -> Option<Vec<DateTime<Utc>>> {
    let mut block = vec![rule_line(event.start_prop(), zone)?];
    let start_tz = match event.start_prop().param("TZID") {
        Some(name) => Tz::from_str(name).ok()?,
        None => zone,
    };
    for prop in &event.recurrence {
        block.push(match prop.name.as_str() {
            "RRULE" => format!("RRULE:{}", utc_until(&prop.value, start_tz)?),
            // Several comma-separated values share the same parameters.
            _ => prop
                .value
                .split(',')
                .map(|value| {
                    rule_line(
                        &Prop {
                            value: value.to_owned(),
                            ..prop.clone()
                        },
                        zone,
                    )
                })
                .collect::<Option<Vec<_>>>()?
                .join("\n"),
        });
    }
    let set = rrule::RRuleSet::from_str(&block.join("\n")).ok()?;
    let utc = rrule::Tz::UTC;
    let result = set
        .after((after - chrono::Duration::seconds(1)).with_timezone(&utc))
        .before(before.with_timezone(&utc))
        .all(MAX_EXPANDED);
    Some(
        result
            .dates
            .iter()
            .map(|at| at.with_timezone(&Utc))
            .collect(),
    )
}

/// The RRULE with `UNTIL` in UTC, which `rrule` requires once DTSTART has a
/// zone. A DATE `UNTIL` (all-day series) means the end of that local day; a
/// floating one is read in the series' zone.
fn utc_until(rule: &str, zone: Tz) -> Option<String> {
    rule.split(';')
        .map(|part| match part.split_once('=') {
            Some((key, value)) if key.eq_ignore_ascii_case("UNTIL") => {
                if let Some(utc) = value.strip_suffix(['Z', 'z']) {
                    return Some(format!("UNTIL={utc}Z"));
                }
                let until = if value.len() == 8 {
                    let day = NaiveDate::parse_from_str(value, "%Y%m%d").ok()?;
                    local_midnight(day.checked_add_days(Days::new(1))?, zone)?
                        - chrono::Duration::seconds(1)
                } else {
                    let naive = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()?;
                    first_valid(naive, zone)?
                };
                Some(format!("UNTIL={}", until.format("%Y%m%dT%H%M%SZ")))
            }
            _ => Some(part.to_owned()),
        })
        .collect::<Option<Vec<_>>>()
        .map(|parts| parts.join(";"))
}

/// A DTSTART/EXDATE/RDATE line in a form `rrule` accepts: dates and floating
/// times get an explicit TZID of `zone`.
fn rule_line(prop: &Prop, zone: Tz) -> Option<String> {
    let value = prop.value.trim();
    let is_date = prop
        .param("VALUE")
        .is_some_and(|v| v.eq_ignore_ascii_case("DATE"))
        || value.len() == 8;
    if is_date {
        return Some(format!("{};TZID={}:{value}T000000", prop.name, zone.name()));
    }
    if value.ends_with('Z') {
        return Some(format!("{}:{value}", prop.name));
    }
    let tz = match prop.param("TZID") {
        Some(name) => Tz::from_str(name).ok()?.name().to_owned(),
        None => zone.name().to_owned(),
    };
    Some(format!("{};TZID={tz}:{value}", prop.name))
}

fn describe(error: reqwest::Error) -> String {
    pending_http::describe_error(error, CONNECT_TIMEOUT, REQUEST_TIMEOUT)
}
