//! Google Calendar as a pending-work source, through the Calendar API with an
//! access token from GNOME Online Accounts (GOA). No OAuth client of our own
//! and no token on disk: GOA owns the account and refreshes its tokens.
//!
//! Events of the calendars visible in Google Calendar are read with
//! `singleEvents=true`, so Google expands recurrences; cards and time buckets
//! are shared with the iCal source.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use futures_util::future::join_all;
use pending_core::{BoxFuture, PendingSource, SourceBatch, SourceError, SourceItem};
use pending_ical::{Occurrence, When, Window, local_zone, occurrence_items};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::Value;

pub const DEFAULT_API_URL: &str = "https://www.googleapis.com/calendar/v3";
/// Optional: the e-mail of the GOA account to use when there are several.
pub const ACCOUNT_ENV: &str = "TASKS_PENDING_GOOGLE_ACCOUNT";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Access tokens for the Calendar API.
pub trait TokenProvider: Send + Sync {
    /// A current token; `refresh` asks for new credentials first (after the
    /// API rejected the previous token).
    fn token(&self, refresh: bool) -> BoxFuture<'_, Result<String, String>>;
}

/// One column: time buckets and, optionally, calendars (by name).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoogleColumn {
    #[serde(skip)]
    pub name: String,
    /// Empty means every bucket.
    #[serde(default)]
    pub when: Vec<When>,
    /// Calendar names as shown in Google Calendar (case-insensitive); empty
    /// means every visible calendar.
    #[serde(default)]
    pub calendar: Vec<String>,
}

/// Columns used when the configuration declares none: one per time bucket.
pub fn default_columns() -> Vec<GoogleColumn> {
    When::ALL
        .into_iter()
        .map(|when| GoogleColumn {
            name: when.default_name().to_owned(),
            when: vec![when],
            calendar: Vec::new(),
        })
        .collect()
}

pub struct GoogleSource {
    client: Client,
    api_url: String,
    tokens: Arc<dyn TokenProvider>,
    columns: Vec<GoogleColumn>,
    window: Window,
    now: fn() -> DateTime<Utc>,
    zone: Tz,
}

impl GoogleSource {
    pub fn new(tokens: Arc<dyn TokenProvider>) -> Self {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            client,
            api_url: DEFAULT_API_URL.to_owned(),
            tokens,
            columns: default_columns(),
            window: Window::default(),
            now: Utc::now,
            zone: local_zone(),
        }
    }

    pub fn with_api_url(mut self, api_url: impl Into<String>) -> Self {
        self.api_url = api_url.into().trim_end_matches('/').to_owned();
        self
    }

    /// Replaces the default columns; an empty list keeps the defaults.
    pub fn with_columns(mut self, columns: Vec<GoogleColumn>) -> Self {
        if !columns.is_empty() {
            self.columns = columns;
        }
        self
    }

    /// The clock and zone deciding "now" and day boundaries (tests).
    pub fn with_clock(mut self, now: fn() -> DateTime<Utc>, zone: Tz) -> Self {
        self.now = now;
        self.zone = zone;
        self
    }

    async fn refresh_now(&self) -> Result<SourceBatch, String> {
        let mut token = self.tokens.token(false).await?;
        let calendars = match self.calendars(&token).await {
            Err(Failure::Auth) => {
                // GOA hands out short-lived tokens; renew once.
                token = self.tokens.token(true).await?;
                self.calendars(&token).await
            }
            other => other,
        }
        .map_err(|failure| failure.message())?;

        let now = (self.now)();
        let until = now + chrono::Duration::days(self.window.days as i64);
        let results = join_all(
            calendars
                .iter()
                .map(|calendar| self.events(&token, &calendar.id, now, until)),
        )
        .await;

        let mut batch = SourceBatch::default();
        let mut failures = Vec::new();
        let mut occurrences = Vec::new();
        let mut calendar_of: HashMap<String, String> = HashMap::new();
        // The same meeting shows in several calendars you can see.
        let mut seen: HashSet<(String, DateTime<Utc>)> = HashSet::new();
        for (calendar, result) in calendars.iter().zip(results) {
            match result {
                Ok(events) => {
                    for event in events {
                        let Some((key, occurrence)) = self.occurrence(&calendar.id, event) else {
                            continue;
                        };
                        if seen.insert((key, occurrence.start)) {
                            calendar_of.insert(occurrence.id.clone(), calendar.summary.clone());
                            occurrences.push(occurrence);
                        }
                    }
                }
                Err(failure) => {
                    failures.push(format!("{}: {}", calendar.summary, failure.message()))
                }
            }
        }
        if !calendars.is_empty() && failures.len() == calendars.len() {
            return Err(format!(
                "all Google calendars failed; {}",
                failures.join("; ")
            ));
        }

        for mut item in occurrence_items(occurrences, now, self.window, self.zone) {
            let calendar = calendar_of.get(&item.card.id).cloned().unwrap_or_default();
            item.card.body.push_str(&format!(" · {calendar}"));
            let bucket = When::ALL
                .into_iter()
                .find(|when| when.default_name() == item.column);
            for column in &self.columns {
                let when_ok =
                    column.when.is_empty() || bucket.is_some_and(|b| column.when.contains(&b));
                let calendar_ok = column.calendar.is_empty()
                    || column
                        .calendar
                        .iter()
                        .any(|name| name.to_lowercase() == calendar.to_lowercase());
                if when_ok && calendar_ok {
                    batch.items.push(SourceItem {
                        column: column.name.clone(),
                        card: item.card.clone(),
                    });
                }
            }
        }
        batch.warnings.extend(failures);
        Ok(batch)
    }

    /// Calendars visible in Google Calendar (selected, or the primary one),
    /// primary first.
    async fn calendars(&self, token: &str) -> Result<Vec<Calendar>, Failure> {
        let mut calendars = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut query = vec![("minAccessRole", "reader"), ("maxResults", "250")];
            if let Some(page) = &page {
                query.push(("pageToken", page.as_str()));
            }
            let list: CalendarList = self.get(token, "/users/me/calendarList", &query).await?;
            calendars.extend(list.items.into_iter().filter(|c| c.selected || c.primary));
            match list.next_page_token {
                Some(next) if page.as_ref() != Some(&next) => page = Some(next),
                _ => break,
            }
        }
        calendars.sort_by_key(|c| !c.primary);
        Ok(calendars)
    }

    async fn events(
        &self,
        token: &str,
        calendar: &str,
        from: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<Value>, Failure> {
        let path = format!("/calendars/{}/events", percent_encode(calendar));
        let (from, until) = (from.to_rfc3339(), until.to_rfc3339());
        let max = (self.window.max_events * 4).to_string();
        let list: EventList = self
            .get(
                token,
                &path,
                &[
                    ("singleEvents", "true"),
                    ("orderBy", "startTime"),
                    ("showDeleted", "false"),
                    ("timeMin", from.as_str()),
                    ("timeMax", until.as_str()),
                    ("maxResults", max.as_str()),
                ],
            )
            .await?;
        Ok(list.items)
    }

    /// An event instance as an occurrence, plus a key identifying the same
    /// meeting across calendars. `None` for cancelled or declined events.
    fn occurrence(&self, calendar: &str, event: Value) -> Option<(String, Occurrence)> {
        if event.get("status").and_then(Value::as_str) == Some("cancelled") {
            return None;
        }
        let declined = event
            .get("attendees")
            .and_then(Value::as_array)
            .is_some_and(|attendees| {
                attendees.iter().any(|a| {
                    a.get("self").and_then(Value::as_bool) == Some(true)
                        && a.get("responseStatus").and_then(Value::as_str) == Some("declined")
                })
            });
        if declined {
            return None;
        }

        let (start, all_day) = self.when(event.get("start")?)?;
        let (finish, _) = self.when(event.get("end")?).unwrap_or((start, all_day));
        let text = |key: &str| event.get(key).and_then(Value::as_str).map(str::to_owned);
        let id = text("id")?;
        let key = text("iCalUID").unwrap_or_else(|| id.clone());
        Some((
            key,
            Occurrence {
                id: format!("google:{calendar}:{id}"),
                title: text("summary").unwrap_or_default(),
                location: text("location"),
                url: text("htmlLink").filter(|url| url.starts_with("https://")),
                start,
                finish,
                all_day,
                updated_at: text("updated")
                    .and_then(|at| DateTime::parse_from_rfc3339(&at).ok())
                    .map(|at| at.with_timezone(&Utc)),
            },
        ))
    }

    /// `{ "dateTime": … }` or an all-day `{ "date": … }` (local midnight).
    fn when(&self, value: &Value) -> Option<(DateTime<Utc>, bool)> {
        if let Some(at) = value.get("dateTime").and_then(Value::as_str) {
            return Some((
                DateTime::parse_from_rfc3339(at).ok()?.with_timezone(&Utc),
                false,
            ));
        }
        let date = NaiveDate::parse_from_str(value.get("date")?.as_str()?, "%Y-%m-%d").ok()?;
        Some((local_midnight(date, self.zone)?, true))
    }

    /// GET `{api}{path}`. Errors never contain the token or the URL.
    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        token: &str,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, Failure> {
        let response = self
            .client
            .get(format!("{}{path}", self.api_url))
            .query(query)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| Failure::Other(describe(error)))?;
        let status = response.status();
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(Failure::Auth);
        }
        if !status.is_success() {
            return Err(Failure::Other(format!(
                "Google Calendar API returned {status}"
            )));
        }
        response.json::<T>().await.map_err(|error| {
            Failure::Other(format!("unexpected Google response: {}", describe(error)))
        })
    }
}

impl PendingSource for GoogleSource {
    fn columns(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }

    fn refresh(&self) -> BoxFuture<'_, Result<SourceBatch, SourceError>> {
        Box::pin(async move { self.refresh_now().await.map_err(SourceError::new) })
    }
}

enum Failure {
    /// 401/403: the token was rejected.
    Auth,
    Other(String),
}

impl Failure {
    fn message(self) -> String {
        match self {
            Failure::Auth => "Google rejected the access token; check the account in GNOME \
                              Online Accounts (Calendar enabled, no pending sign-in)"
                .to_owned(),
            Failure::Other(message) => message,
        }
    }
}

#[derive(Deserialize)]
struct CalendarList {
    #[serde(default)]
    items: Vec<Calendar>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct Calendar {
    id: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    primary: bool,
    #[serde(default)]
    selected: bool,
}

#[derive(Deserialize)]
struct EventList {
    #[serde(default)]
    items: Vec<Value>,
}

fn local_midnight(date: NaiveDate, zone: Tz) -> Option<DateTime<Utc>> {
    (0..=120).find_map(|minute| {
        let time = date.and_hms_opt(0, 0, 0)? + chrono::Duration::minutes(minute);
        zone.from_local_datetime(&time)
            .earliest()
            .map(|at| at.with_timezone(&Utc))
    })
}

/// Calendar ids are e-mail-like (`a@b.com`, `…#holiday@group.v.calendar…`).
fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn describe(error: reqwest::Error) -> String {
    pending_http::describe_error(error, CONNECT_TIMEOUT, REQUEST_TIMEOUT)
}

/// Tokens from GNOME Online Accounts over the session D-Bus.
pub struct GoaTokens {
    /// E-mail of the account to use; the first Google account with Calendar
    /// enabled when `None`.
    account: Option<String>,
}

impl GoaTokens {
    pub fn new(account: Option<String>) -> Self {
        Self { account }
    }

    /// Reads the optional account from [`ACCOUNT_ENV`].
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Self {
        Self::new(
            env(ACCOUNT_ENV)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty()),
        )
    }

    async fn fetch(&self, refresh: bool) -> Result<String, String> {
        const SERVICE: &str = "org.gnome.OnlineAccounts";
        let unavailable = |error: zbus::Error| {
            format!(
                "GNOME Online Accounts is not available ({error}); install gnome-online-accounts \
                 and add the Google account with Calendar enabled"
            )
        };
        let connection = zbus::Connection::session().await.map_err(unavailable)?;
        let manager = zbus::fdo::ObjectManagerProxy::builder(&connection)
            .destination(SERVICE)
            .and_then(|b| b.path("/org/gnome/OnlineAccounts"))
            .map_err(unavailable)?
            .build()
            .await
            .map_err(unavailable)?;
        let objects = manager
            .get_managed_objects()
            .await
            .map_err(|e| unavailable(e.into()))?;

        let account = objects
            .iter()
            .filter_map(|(path, interfaces)| {
                let props = interfaces
                    .iter()
                    .find(|(name, _)| name.as_str() == "org.gnome.OnlineAccounts.Account")?
                    .1;
                let oauth2 = interfaces
                    .keys()
                    .any(|name| name.as_str() == "org.gnome.OnlineAccounts.OAuth2Based");
                let text = |key: &str| {
                    props
                        .get(key)
                        .and_then(|v| <&str>::try_from(&**v).ok())
                        .map(str::to_owned)
                };
                let flag = |key: &str| {
                    props
                        .get(key)
                        .and_then(|v| bool::try_from(&**v).ok())
                        .unwrap_or(false)
                };
                let usable = oauth2
                    && text("ProviderType").as_deref() == Some("google")
                    && !flag("CalendarDisabled");
                let identity = text("PresentationIdentity").or_else(|| text("Identity"));
                let wanted = match &self.account {
                    Some(email) => identity
                        .as_deref()
                        .is_some_and(|id| id.eq_ignore_ascii_case(email)),
                    None => true,
                };
                (usable && wanted).then(|| (path.clone(), flag("AttentionNeeded")))
            })
            .min_by_key(|(path, _)| path.to_string())
            .ok_or_else(|| match &self.account {
                Some(email) => format!(
                    "no Google account {email} with Calendar enabled in GNOME Online Accounts"
                ),
                None => "no Google account with Calendar enabled in GNOME Online Accounts; add \
                         one with gnome-online-accounts-gtk"
                    .to_owned(),
            })?;
        let (path, attention_needed) = account;
        if attention_needed && !refresh {
            return Err(
                "the Google account in GNOME Online Accounts needs attention (sign in \
                        again in gnome-online-accounts-gtk)"
                    .to_owned(),
            );
        }

        let proxy = |interface: &'static str| {
            zbus::Proxy::new(&connection, SERVICE, path.clone(), interface)
        };
        if refresh {
            let account = proxy("org.gnome.OnlineAccounts.Account")
                .await
                .map_err(unavailable)?;
            let _: i32 = account
                .call("EnsureCredentials", &())
                .await
                .map_err(|e| format!("GNOME Online Accounts could not renew credentials: {e}"))?;
        }
        let oauth2 = proxy("org.gnome.OnlineAccounts.OAuth2Based")
            .await
            .map_err(unavailable)?;
        let (token, _expires_in): (String, i32) = oauth2
            .call("GetAccessToken", &())
            .await
            .map_err(|e| format!("GNOME Online Accounts gave no access token: {e}"))?;
        Ok(token)
    }
}

impl TokenProvider for GoaTokens {
    fn token(&self, refresh: bool) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(self.fetch(refresh))
    }
}
