//! Icons: the system's symbolic set for controls, and each source's logo
//! (an image URL from the config) fetched once and kept in memory.

use std::collections::HashMap;
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use pending_core::{Icon, WeatherCondition};

use super::Ctx;

/// Largest logo fetched; anything bigger is not a logo.
const MAX_ICON_BYTES: usize = 1024 * 1024;

/// Logos by URL: the bytes once fetched, `None` while they are being
/// fetched or when the fetch failed.
#[derive(Default)]
pub struct Cache {
    bytes: HashMap<String, Option<gtk::glib::Bytes>>,
    http: Option<reqwest::Client>,
}

impl Cache {
    fn client(&mut self) -> reqwest::Client {
        self.http
            .get_or_insert_with(|| {
                reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(15))
                    .build()
                    .unwrap_or_default()
            })
            .clone()
    }
}

/// A symbolic icon from the system theme, `size` pixels.
pub fn symbolic(name: &str, size: i32) -> gtk::Image {
    let image = gtk::Image::from_icon_name(name);
    image.set_pixel_size(size);
    image
}

/// The icon name for a weather condition.
pub fn weather_icon_name(condition: WeatherCondition) -> &'static str {
    match condition {
        WeatherCondition::ClearDay => "weather-clear-symbolic",
        WeatherCondition::ClearNight => "weather-clear-night-symbolic",
        WeatherCondition::PartlyCloudyDay => "weather-few-clouds-symbolic",
        WeatherCondition::PartlyCloudyNight => "weather-few-clouds-night-symbolic",
        WeatherCondition::Cloudy | WeatherCondition::Unknown => "weather-overcast-symbolic",
        WeatherCondition::Fog => "weather-fog-symbolic",
        WeatherCondition::Rain => "weather-showers-symbolic",
        WeatherCondition::Sleet => "weather-showers-scattered-symbolic",
        WeatherCondition::Snow => "weather-snow-symbolic",
        WeatherCondition::Thunder => "weather-storm-symbolic",
    }
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("https://") || value.starts_with("http://")
}

/// The source's logo at `size` pixels, or a generic icon without one. The
/// logo appears once fetched; until then (and on failure) the widget is an
/// empty box of the same size, so the layout does not move.
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
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Contain)
        .can_shrink(true)
        .width_request(size)
        .height_request(size)
        .build();
    let cached = ctx.model.borrow().icons.bytes.get(&url).cloned();
    match cached {
        Some(Some(bytes)) => show(&picture, &bytes),
        Some(None) => {}
        None => fetch(ctx, url, picture.clone()),
    }
    picture.upcast()
}

fn show(picture: &gtk::Picture, bytes: &gtk::glib::Bytes) {
    if let Ok(texture) = gtk::gdk::Texture::from_bytes(bytes) {
        picture.set_paintable(Some(&texture));
    }
}

/// Fetches the logo on the runtime and shows it on the GTK thread.
fn fetch(ctx: &Rc<Ctx>, url: String, picture: gtk::Picture) {
    let client = {
        let mut model = ctx.model.borrow_mut();
        model.icons.bytes.insert(url.clone(), None);
        model.icons.client()
    };
    let (sender, receiver) = tokio::sync::oneshot::channel::<Option<Vec<u8>>>();
    let request_url = url.clone();
    ctx.runtime_handle.spawn(async move {
        let bytes = async {
            let response = client.get(&request_url).send().await.ok()?;
            if !response.status().is_success() {
                return None;
            }
            let bytes = response.bytes().await.ok()?;
            (bytes.len() <= MAX_ICON_BYTES).then(|| bytes.to_vec())
        }
        .await;
        let _ = sender.send(bytes);
    });
    let ctx = ctx.clone();
    gtk::glib::spawn_future_local(async move {
        let Ok(Some(bytes)) = receiver.await else {
            return;
        };
        let bytes = gtk::glib::Bytes::from_owned(bytes);
        // Only an image the texture loader accepts is kept.
        if gtk::gdk::Texture::from_bytes(&bytes).is_err() {
            return;
        }
        show(&picture, &bytes);
        ctx.model.borrow_mut().icons.bytes.insert(url, Some(bytes));
    });
}
