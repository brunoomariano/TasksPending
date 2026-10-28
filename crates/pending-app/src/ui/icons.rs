//! Icons: the system's symbolic set for controls, and each source's logo
//! (an image URL from the config) fetched once and kept in memory.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use pending_core::{Icon, is_http_url};

use super::Ctx;

/// The icon sizes the window uses, in pixels, so that like things match.
pub mod size {
    /// Beside small text: a card's source, the update time, a toggle.
    pub const SMALL: i32 = 13;
    /// Beside body text: a stack's name, a section title, a status.
    pub const TEXT: i32 = 14;
    /// In a control button, and a logo in a Settings row.
    pub const CONTROL: i32 = 16;
    /// A source's logo heading its group or its row in Sources.
    pub const LOGO: i32 = 18;
    /// The weather beside the temperature.
    pub const WEATHER: i32 = 28;
}

/// Largest logo fetched; anything bigger is not a logo.
const MAX_ICON_BYTES: usize = 1024 * 1024;
/// Longest wait for a logo.
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
/// A logo that could not be fetched is tried again after this long, then
/// twice as long after each further failure, up to [`RETRY_AT_MOST`].
const RETRY_AFTER: Duration = Duration::from_secs(60);
const RETRY_AT_MOST: Duration = Duration::from_secs(60 * 60);

/// What is known about one logo.
enum Logo {
    Fetching,
    Ready(gtk::glib::Bytes),
    /// Not fetched, decoded or small enough, `attempts` times so far; tried
    /// again at `retry_at` for the pictures still waiting then, or when a
    /// page built after it wants the logo.
    Failed {
        attempts: u32,
        retry_at: Instant,
    },
}

/// Logos by URL, and the pictures waiting for one: being fetched, or
/// failed and due again (the page may be rebuilt meanwhile, so they are
/// held weakly and a picture no longer shown is let go).
#[derive(Default)]
pub struct Cache {
    logos: HashMap<String, Logo>,
    waiting: HashMap<String, Vec<gtk::glib::WeakRef<gtk::Image>>>,
    http: Option<reqwest::Client>,
}

impl Cache {
    /// The HTTP client for logos, built once; `None` when the HTTP stack
    /// cannot be set up at all, in which case no logo is fetched.
    fn client(&mut self) -> Option<reqwest::Client> {
        if self.http.is_none() {
            match reqwest::Client::builder().timeout(FETCH_TIMEOUT).build() {
                Ok(client) => self.http = Some(client),
                Err(error) => tracing::warn!(%error, "no HTTP client for logos"),
            }
        }
        self.http.clone()
    }
}

/// How long to wait before fetching a logo again after `attempts` failures.
fn retry_delay(attempts: u32) -> Duration {
    RETRY_AFTER
        .saturating_mul(1u32 << attempts.min(16))
        .min(RETRY_AT_MOST)
}

/// A symbolic icon from the system theme, `size` pixels.
pub fn symbolic(name: &str, size: i32) -> gtk::Image {
    let image = gtk::Image::from_icon_name(name);
    image.set_pixel_size(size);
    image
}

/// What to do for a picture that wants a logo.
enum Next {
    Show(gtk::glib::Bytes),
    /// Being fetched, or failed lately and not due yet: this picture stays
    /// empty and gets the logo with the others.
    Wait,
    Fetch {
        attempts: u32,
    },
}

/// The source's logo at `size` pixels, or a generic icon without one. The
/// logo appears once fetched; until then (and on failure) the widget is an
/// empty image of the same size, so the layout does not move. An image
/// with a pixel size measures exactly that, whatever the logo's own size;
/// a picture would ask for the logo's intrinsic size and widen its group.
pub fn source_icon(ctx: &Rc<Ctx>, icon: Option<&Icon>, size: i32) -> gtk::Widget {
    let url = icon.and_then(|icon| {
        let dark = adw::StyleManager::default().is_dark();
        let url = match (&icon.dark_url, dark) {
            (Some(dark_url), true) if is_http_url(dark_url) => dark_url.as_str(),
            _ => icon.url.as_str(),
        };
        is_http_url(url).then(|| url.to_owned())
    });
    let Some(url) = url else {
        let image = symbolic("view-list-symbolic", size);
        return image.upcast();
    };
    let picture = gtk::Image::new();
    picture.set_pixel_size(size);
    let next = match ctx.model.borrow().icons.logos.get(&url) {
        Some(Logo::Ready(bytes)) => Next::Show(bytes.clone()),
        Some(Logo::Fetching) => Next::Wait,
        Some(Logo::Failed { retry_at, .. }) if Instant::now() < *retry_at => Next::Wait,
        Some(Logo::Failed { attempts, .. }) => Next::Fetch {
            attempts: *attempts,
        },
        None => Next::Fetch { attempts: 0 },
    };
    if let Next::Show(bytes) = &next {
        show(&picture, bytes);
        return picture.upcast();
    }
    ctx.model
        .borrow_mut()
        .icons
        .waiting
        .entry(url.clone())
        .or_default()
        .push(picture.downgrade());
    if let Next::Fetch { attempts } = next {
        fetch(ctx, url, attempts);
    }
    picture.upcast()
}

fn show(image: &gtk::Image, bytes: &gtk::glib::Bytes) {
    if let Ok(texture) = gtk::gdk::Texture::from_bytes(bytes) {
        image.set_paintable(Some(&texture));
    }
}

/// Fetches the logo on the runtime and shows it on the GTK thread, on
/// every picture waiting for it. `attempts` is how many fetches of it
/// failed before; after another failure it is fetched again once the
/// retry is due, if a picture still waits for it.
fn fetch(ctx: &Rc<Ctx>, url: String, attempts: u32) {
    let client = {
        let mut model = ctx.model.borrow_mut();
        let Some(client) = model.icons.client() else {
            model.icons.logos.insert(url, failed(attempts));
            return;
        };
        model.icons.logos.insert(url.clone(), Logo::Fetching);
        client
    };
    let (sender, receiver) = tokio::sync::oneshot::channel::<Option<Vec<u8>>>();
    let request_url = url.clone();
    ctx.runtime_handle.spawn(async move {
        let _ = sender.send(download(&client, &request_url).await);
    });
    let ctx = ctx.clone();
    gtk::glib::spawn_future_local(async move {
        // Only an image the texture loader accepts is kept.
        let bytes = receiver.await.ok().flatten().and_then(|bytes| {
            let bytes = gtk::glib::Bytes::from_owned(bytes);
            match gtk::gdk::Texture::from_bytes(&bytes) {
                Ok(_) => Some(bytes),
                Err(error) => {
                    tracing::debug!(url, %error, "logo not decoded");
                    None
                }
            }
        });
        let mut model = ctx.model.borrow_mut();
        match bytes {
            Some(bytes) => {
                let waiting = model.icons.waiting.remove(&url).unwrap_or_default();
                model.icons.logos.insert(url, Logo::Ready(bytes.clone()));
                drop(model);
                for picture in waiting.iter().filter_map(|picture| picture.upgrade()) {
                    show(&picture, &bytes);
                }
            }
            None => {
                model.icons.logos.insert(url.clone(), failed(attempts));
                drop(model);
                retry_later(&ctx, url, retry_delay(attempts));
            }
        }
    });
}

/// Fetches `url` again after `delay`, when a picture still on screen waits
/// for it: a logo missed while the network was down shows up without the
/// page having to be rebuilt.
fn retry_later(ctx: &Rc<Ctx>, url: String, delay: Duration) {
    let ctx = Rc::downgrade(ctx);
    gtk::glib::timeout_add_local_once(delay, move || {
        let Some(ctx) = ctx.upgrade() else {
            return;
        };
        let attempts = {
            let mut model = ctx.model.borrow_mut();
            let Some(attempts) = retry_due(model.icons.logos.get(&url), Instant::now()) else {
                return;
            };
            let waiting = model.icons.waiting.entry(url.clone()).or_default();
            waiting.retain(|picture| picture.upgrade().is_some_and(|p| p.parent().is_some()));
            if waiting.is_empty() {
                return;
            }
            attempts
        };
        fetch(&ctx, url, attempts);
    });
}

/// The failed attempts so far when a fetch of this logo is due at `now`;
/// `None` when it is fetched, being fetched, or failed again since the
/// timer was set (that failure's own timer is the one due).
fn retry_due(logo: Option<&Logo>, now: Instant) -> Option<u32> {
    match logo {
        Some(Logo::Failed { attempts, retry_at }) if now >= *retry_at => Some(*attempts),
        _ => None,
    }
}

fn failed(attempts: u32) -> Logo {
    Logo::Failed {
        attempts: attempts + 1,
        retry_at: Instant::now() + retry_delay(attempts),
    }
}

/// The logo's bytes, or `None` when it cannot be had or is bigger than
/// [`MAX_ICON_BYTES`]: the size is checked before and while reading, so a
/// large or endless body is dropped rather than kept.
async fn download(client: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let mut response = match client.get(url).send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::debug!(url, %error, "logo not fetched");
            return None;
        }
    };
    if !response.status().is_success() {
        tracing::debug!(url, status = %response.status(), "logo not served");
        return None;
    }
    let too_big = || tracing::debug!(url, "logo bigger than {MAX_ICON_BYTES} bytes; not kept");
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ICON_BYTES as u64)
    {
        too_big();
        return None;
    }
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if bytes.len() + chunk.len() > MAX_ICON_BYTES {
                    too_big();
                    return None;
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => return Some(bytes),
            Err(error) => {
                tracing::debug!(url, %error, "logo cut short");
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: a logo failed while the network was down and its retry
    /// comes due. It is fetched again for the pictures waiting; a timer
    /// left from an earlier failure does nothing before the latest
    /// failure's retry is due, and nothing happens once the logo is in.
    #[test]
    fn retries_a_failed_logo_only_when_due() {
        let now = Instant::now();
        let failed_at = |retry_at| Logo::Failed {
            attempts: 2,
            retry_at,
        };
        assert_eq!(retry_due(Some(&failed_at(now)), now), Some(2));
        assert_eq!(
            retry_due(Some(&failed_at(now + Duration::from_secs(60))), now),
            None
        );
        assert_eq!(retry_due(Some(&Logo::Fetching), now), None);
        let ready = Logo::Ready(gtk::glib::Bytes::from_static(b"png"));
        assert_eq!(retry_due(Some(&ready), now), None);
        assert_eq!(retry_due(None, now), None);
    }

    /// A failed logo is tried again after a minute, then after twice as
    /// long each time, and never less often than once an hour.
    #[test]
    fn retries_later_each_time_up_to_an_hour() {
        assert_eq!(retry_delay(0), Duration::from_secs(60));
        assert_eq!(retry_delay(1), Duration::from_secs(120));
        assert_eq!(retry_delay(5), Duration::from_secs(32 * 60));
        assert_eq!(retry_delay(6), RETRY_AT_MOST);
        assert_eq!(retry_delay(40), RETRY_AT_MOST);
    }

    /// A logo server on a loopback port: each path is answered with the
    /// raw response `answer` gives for it.
    fn serve(answer: fn(&str) -> Vec<u8>) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut head = Vec::new();
                let mut byte = [0; 1];
                while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let path = head.split_whitespace().nth(1).unwrap_or("").to_owned();
                // The app may hang up once it has seen enough.
                let _ = stream.write_all(&answer(&path));
            }
        });
        url
    }

    /// A body of `size` bytes, saying how long it is when `declared`, and
    /// otherwise running until the connection closes.
    fn body(size: usize, declared: bool) -> Vec<u8> {
        let length = if declared {
            format!("content-length: {size}\r\n")
        } else {
            String::new()
        };
        let mut response =
            format!("HTTP/1.1 200 OK\r\n{length}connection: close\r\n\r\n").into_bytes();
        response.resize(response.len() + size, b'x');
        response
    }

    /// A logo is kept up to [`MAX_ICON_BYTES`]: one that says it is bigger
    /// is not read, and one that does not say is dropped once it grows past.
    #[test]
    fn keeps_no_logo_bigger_than_the_cap() {
        let url = serve(|path| match path {
            "/declared" => body(MAX_ICON_BYTES + 1, true),
            "/streamed" => body(MAX_ICON_BYTES + 1, false),
            "/limit" => body(MAX_ICON_BYTES, false),
            _ => body(5, true),
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        // Lengths, so a failure does not print a megabyte.
        let fetch = |path: &str| {
            runtime
                .block_on(download(&client, &format!("{url}{path}")))
                .map(|bytes| bytes.len())
        };
        assert_eq!(fetch("/declared"), None);
        assert_eq!(fetch("/streamed"), None);
        assert_eq!(fetch("/limit"), Some(MAX_ICON_BYTES));
        assert_eq!(fetch("/small"), Some(5));
    }
}
