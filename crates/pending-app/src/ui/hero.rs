//! The strip above the board: the weather, the big clock and the controls.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use chrono::{Local, Timelike};
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;

use super::dialogs;
use super::icons::symbolic;
use super::{Ctx, Widgets};
use crate::backend::Command;
use crate::clock::{clock_text, date_text, drawing};
use crate::present::{Page, refresh_button, update_line, weather_icon_name};
use crate::view::View;

/// Widest the clock gets.
const CLOCK_MAX_WIDTH: i32 = 880;
/// The weather widget is shown only in windows at least this wide.
pub const WEATHER_MIN_WIDTH: f64 = 1440.0;
/// The widest window that hides the weather. A breakpoint's `max-width`
/// counts its own width in, so it ends a pixel below
/// [`WEATHER_MIN_WIDTH`].
pub const NARROW_MAX_WIDTH: f64 = WEATHER_MIN_WIDTH - 1.0;
/// The room kept for the weather, so the clock stays centred whether or
/// not a report is in.
const WEATHER_SLOT_WIDTH: i32 = 180;
/// Longest the line saying where the dashboard comes from gets before it
/// is cut; the whole line is in the tooltip.
const SOURCE_LINE_CHARS: i32 = 34;
/// How long a note under the controls stays.
const NOTE_FOR: Duration = Duration::from_secs(6);

pub struct Hero {
    pub root: gtk::CenterBox,
    /// Hidden in narrow windows, whatever the weather.
    pub weather_slot: gtk::Box,
    weather: gtk::Box,
    weather_icon: gtk::Image,
    weather_temperature: gtk::Label,
    weather_place: gtk::Label,
    weather_wind: gtk::Label,
    sources_icon: gtk::Image,
    refresh_button: gtk::Button,
    refresh_label: gtk::Label,
    settings_button: gtk::Button,
    filter_dot: gtk::Box,
    updated_line: gtk::Box,
    updated: gtk::Label,
    version: gtk::Label,
    snoozed_button: gtk::Button,
    update: gtk::Label,
    note: gtk::Label,
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(0.0).build();
    for class in classes {
        label.add_css_class(class);
    }
    label
}

fn control_button(
    icon: &str,
    text: &str,
    classes: &[&str],
) -> (gtk::Button, gtk::Image, gtk::Label) {
    let image = symbolic(icon, 16);
    let label = gtk::Label::new(Some(text));
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.append(&image);
    content.append(&label);
    let button = gtk::Button::builder().child(&content).build();
    button.add_css_class("control");
    for class in classes {
        button.add_css_class(class);
    }
    (button, image, label)
}

pub fn build(ctx: &Rc<Ctx>) -> Hero {
    // Weather: icon and temperature, then place and wind.
    let weather_icon = symbolic("weather-overcast-symbolic", 28);
    let weather_temperature = label("", &["weather-temperature"]);
    let now = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    now.append(&weather_icon);
    now.append(&weather_temperature);
    let weather_place = label("", &["weather-details"]);
    let weather_wind = label("", &["weather-details"]);
    let weather = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    weather.add_css_class("weather");
    weather.append(&now);
    weather.append(&weather_place);
    weather.append(&weather_wind);
    let weather_slot = gtk::Box::builder()
        .valign(gtk::Align::Center)
        .width_request(WEATHER_SLOT_WIDTH)
        .build();
    weather_slot.append(&weather);

    // The clock, scaling with the window up to a width.
    let date = label("", &["clock-date"]);
    date.set_halign(gtk::Align::Center);
    let attributes = gtk::pango::AttrList::new();
    attributes.insert(gtk::pango::AttrInt::new_letter_spacing(
        super::board::TRACKING / 2,
    ));
    date.set_attributes(Some(&attributes));
    let digits = gtk::DrawingArea::builder().hexpand(true).build();
    digits.add_css_class("clock-digits");
    // The frame keeps the shape of the clock's text, so the digits fill it.
    let shape = drawing("00:00:00");
    let frame = gtk::AspectFrame::builder()
        .ratio(shape.width as f32 / shape.height as f32)
        .obey_child(false)
        .child(&digits)
        .build();
    let clock = gtk::Box::new(gtk::Orientation::Vertical, 14);
    clock.append(&date);
    clock.append(&frame);
    let clamp = adw::Clamp::builder()
        .maximum_size(CLOCK_MAX_WIDTH)
        .tightening_threshold(CLOCK_MAX_WIDTH)
        .child(&clock)
        .hexpand(true)
        .build();
    start_clock(date, digits);

    // The controls, with the last update below them.
    let (sources_button, sources_icon, _) = control_button("view-list-symbolic", "Sources", &[]);
    let (refresh_button, _, refresh_label) =
        control_button("view-refresh-symbolic", "Refresh", &["primary"]);
    let filter_dot = gtk::Box::builder()
        .valign(gtk::Align::Start)
        .halign(gtk::Align::End)
        .visible(false)
        .build();
    filter_dot.add_css_class("filter-dot");
    let settings_icon = symbolic("emblem-system-symbolic", 16);
    let settings_overlay = gtk::Overlay::builder().child(&settings_icon).build();
    settings_overlay.add_overlay(&filter_dot);
    let settings_button = gtk::Button::builder().child(&settings_overlay).build();
    settings_button.add_css_class("control");
    settings_button.add_css_class("icon-only");
    settings_button.set_tooltip_text(Some("Settings"));
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.append(&sources_button);
    actions.append(&refresh_button);
    actions.append(&settings_button);

    let updated = label("", &[]);
    updated.set_single_line_mode(true);
    let version = label("", &[]);
    let snoozed_button = gtk::Button::builder().visible(false).build();
    snoozed_button.add_css_class("quiet-link");
    let updated_line = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .halign(gtk::Align::Center)
        .visible(false)
        .build();
    updated_line.add_css_class("updated");
    updated_line.append(&symbolic("alarm-symbolic", 13));
    updated_line.append(&updated);
    updated_line.append(&version);
    updated_line.append(&snoozed_button);
    let update = label("", &[]);
    update.set_visible(false);
    update.set_tooltip_text(Some(
        "A newer version is published. Run: tasks-pending upgrade",
    ));
    updated_line.append(&update);
    let note = label("", &["updated"]);
    note.set_halign(gtk::Align::Center);
    note.set_visible(false);
    // Where the dashboard comes from, kept short so the clock keeps its
    // room; the whole line is in the tooltip.
    let source_text = ctx.model.borrow().source_line.clone();
    let source_line = label(&source_text, &["updated"]);
    source_line.set_halign(gtk::Align::Center);
    source_line.set_single_line_mode(true);
    source_line.set_ellipsize(gtk::pango::EllipsizeMode::End);
    source_line.set_max_width_chars(SOURCE_LINE_CHARS);
    source_line.set_tooltip_text(Some(&source_text));

    let controls = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .build();
    controls.add_css_class("controls");
    controls.append(&actions);
    controls.append(&updated_line);
    controls.append(&note);
    controls.append(&source_line);

    let root = gtk::CenterBox::builder().build();
    root.add_css_class("hero");
    root.set_start_widget(Some(&weather_slot));
    root.set_center_widget(Some(&clamp));
    root.set_end_widget(Some(&controls));

    {
        let ctx = ctx.clone();
        sources_button.connect_clicked(move |_| dialogs::show_sources(&ctx));
    }
    {
        let ctx = ctx.clone();
        settings_button.connect_clicked(move |_| dialogs::show_settings(&ctx));
    }
    {
        let ctx = ctx.clone();
        snoozed_button.connect_clicked(move |_| dialogs::show_snoozed(&ctx));
    }
    {
        let ctx = ctx.clone();
        refresh_button.connect_clicked(move |button| {
            button.set_sensitive(false);
            ctx.send(Command::Refresh { manual: true });
        });
    }

    Hero {
        root,
        weather_slot,
        weather,
        weather_icon,
        weather_temperature,
        weather_place,
        weather_wind,
        sources_icon,
        refresh_button,
        refresh_label,
        settings_button,
        filter_dot,
        updated_line,
        updated,
        version,
        snoozed_button,
        update,
        note,
    }
}

/// Keeps the date and the clock current, redrawing on each new second.
fn start_clock(date: gtk::Label, digits: gtk::DrawingArea) {
    let shown = Rc::new(RefCell::new(String::new()));
    {
        let shown = shown.clone();
        digits.set_draw_func(move |area, cr, width, height| {
            let drawing = drawing(&shown.borrow());
            if drawing.width == 0 {
                return;
            }
            let unit = (f64::from(width) / f64::from(drawing.width))
                .min(f64::from(height) / f64::from(drawing.height));
            let left = (f64::from(width) - unit * f64::from(drawing.width)) / 2.0;
            let color = area.color();
            cr.set_source_rgba(
                f64::from(color.red()),
                f64::from(color.green()),
                f64::from(color.blue()),
                f64::from(color.alpha()),
            );
            for block in &drawing.blocks {
                // Whole pixels: crisp edges, no seams between blocks.
                let x = (left + f64::from(block.x) * unit).round();
                let y = (f64::from(block.y) * unit).round();
                let right = (left + f64::from(block.x + block.width) * unit).round();
                let bottom = (f64::from(block.y + block.height) * unit).round();
                cr.rectangle(x, y, right - x, bottom - y);
            }
            let _ = cr.fill();
        });
    }
    fn tick(date: gtk::Label, digits: gtk::DrawingArea, shown: Rc<RefCell<String>>) {
        let now = Local::now();
        let text = clock_text(&now);
        if *shown.borrow() != text {
            *shown.borrow_mut() = text;
            date.set_label(&date_text(&now));
            digits.queue_draw();
        }
        let wait = 1000 - u64::from(now.nanosecond() / 1_000_000).min(999);
        gtk::glib::timeout_add_local_once(Duration::from_millis(wait), move || {
            tick(date, digits, shown)
        });
    }
    tick(date, digits, shown);
}

/// The controls for the current page: the refresh hold, the settings dot,
/// attention on Sources, the update line. The refresh button is settled
/// whether or not there is a page: a refresh refused while the dashboard
/// is unavailable must not leave it off.
pub fn update_controls(ctx: &Rc<Ctx>, widgets: &Widgets, page: Option<&Page>, view: &View) {
    let hero = &widgets.hero;
    let (label, enabled) = refresh_button(ctx.model.borrow().refresh_hold.as_ref(), Instant::now());
    hero.refresh_button.set_sensitive(enabled);
    hero.refresh_label.set_label(&label);
    hero.settings_button
        .set_tooltip_text(Some(&view.settings_label()));
    hero.filter_dot.set_visible(view.is_filtering());
    let Some(page) = page else {
        hero.updated_line.set_visible(false);
        return;
    };
    hero.sources_icon.set_icon_name(Some(if page.attention {
        "dialog-warning-symbolic"
    } else {
        "view-list-symbolic"
    }));
    if page.attention {
        hero.sources_icon.add_css_class("attention");
    } else {
        hero.sources_icon.remove_css_class("attention");
    }
    hero.updated.set_label(&page.updated);
    hero.version
        .set_label(page.version_line.as_deref().unwrap_or_default());
    hero.version.set_visible(page.version_line.is_some());
    hero.snoozed_button
        .set_label(page.snoozed_line.as_deref().unwrap_or_default());
    hero.snoozed_button.set_visible(page.snoozed_line.is_some());
    let update = ctx.model.borrow().update.clone();
    match update {
        Some(version) => {
            hero.update.set_label(&update_line(&version));
            hero.update.set_visible(true);
        }
        None => hero.update.set_visible(false),
    }
    hero.updated_line.set_visible(true);
}

/// Shows or hides the weather widget for the report in the model.
pub fn update_weather(ctx: &Rc<Ctx>, widgets: &Widgets) {
    let hero = &widgets.hero;
    let report = ctx.model.borrow().weather.clone();
    let Some(report) = report else {
        hero.weather.set_visible(false);
        return;
    };
    hero.weather_icon
        .set_icon_name(Some(weather_icon_name(report.condition)));
    hero.weather_icon
        .set_tooltip_text(Some(report.condition.label()));
    hero.weather_temperature.set_label(&report.temperature);
    hero.weather_place.set_label(&report.place);
    hero.weather_wind
        .set_label(&format!("Wind {}", report.wind));
    hero.weather.set_visible(true);
}

/// A short note under the controls, e.g. why a change was refused.
pub fn show_note(widgets: &Widgets, text: &str) {
    let note = widgets.hero.note.clone();
    note.set_label(text);
    note.set_visible(true);
    gtk::glib::timeout_add_local_once(NOTE_FOR, move || note.set_visible(false));
}
