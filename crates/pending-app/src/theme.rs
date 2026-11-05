//! The app's look: its own palette (dark by default, light when the system
//! asks), or one mixed from the theme Omarchy has applied, written into the
//! stylesheet GTK loads. GTK CSS has no variables on every version the app
//! runs on, so the tokens are filled in here.

use std::borrow::Cow;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use libadwaita::ColorScheme;

use crate::Scene;

/// The colours of one scheme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub bg: Cow<'static, str>,
    pub surface: Cow<'static, str>,
    pub surface_2: Cow<'static, str>,
    pub border: Cow<'static, str>,
    pub border_strong: Cow<'static, str>,
    pub text: Cow<'static, str>,
    pub text_2: Cow<'static, str>,
    pub text_3: Cow<'static, str>,
    pub accent: Cow<'static, str>,
    /// The accent as text: the accent itself, unless a theme's is too
    /// faint to read on the window's grounds.
    pub accent_ink: Cow<'static, str>,
    /// The count on a checked tab: the accent as text, deepened further
    /// where the count's own tint needs it.
    pub checked_count: Cow<'static, str>,
    /// The accent, lightened for a hover.
    pub accent_hover: Cow<'static, str>,
    /// The accent at low alpha, for tinted surfaces.
    pub accent_soft: Cow<'static, str>,
    /// Text on the accent.
    pub on_accent: Cow<'static, str>,
    pub ok: Cow<'static, str>,
    pub warn: Cow<'static, str>,
    pub danger: Cow<'static, str>,
    /// A card under the pointer.
    pub card_hover: Cow<'static, str>,
    /// Dialog shadow.
    pub shadow: Cow<'static, str>,
}

impl Palette {
    /// Each colour under the name the stylesheet gives it.
    pub fn tokens(&self) -> [(&'static str, &str); 19] {
        [
            ("bg", &self.bg),
            ("surface", &self.surface),
            ("surface-2", &self.surface_2),
            ("border", &self.border),
            ("border-strong", &self.border_strong),
            ("text", &self.text),
            ("text-2", &self.text_2),
            ("text-3", &self.text_3),
            ("accent", &self.accent),
            ("accent-ink", &self.accent_ink),
            ("checked-count", &self.checked_count),
            ("accent-hover", &self.accent_hover),
            ("accent-soft", &self.accent_soft),
            ("on-accent", &self.on_accent),
            ("ok", &self.ok),
            ("warn", &self.warn),
            ("danger", &self.danger),
            ("card-hover", &self.card_hover),
            ("shadow", &self.shadow),
        ]
    }
}

const fn fixed(colour: &'static str) -> Cow<'static, str> {
    Cow::Borrowed(colour)
}

pub const DARK: Palette = Palette {
    bg: fixed("#0e0f12"),
    surface: fixed("#16181d"),
    surface_2: fixed("#1c1f25"),
    border: fixed("#262a31"),
    border_strong: fixed("#343a43"),
    text: fixed("#e7e9ee"),
    text_2: fixed("#babdc5"),
    text_3: fixed("#9196a0"),
    accent: fixed("#7c8cff"),
    accent_ink: fixed("#7c8cff"),
    checked_count: fixed("#8b9aff"),
    accent_hover: fixed("#8c9aff"),
    accent_soft: fixed("rgba(124, 140, 255, 0.14)"),
    on_accent: fixed("#0e0f12"),
    ok: fixed("#4ccb8f"),
    warn: fixed("#f2c14e"),
    danger: fixed("#f2667a"),
    card_hover: fixed("#23272e"),
    shadow: fixed("rgba(0, 0, 0, 0.45)"),
};

pub const LIGHT: Palette = Palette {
    bg: fixed("#f6f7f9"),
    surface: fixed("#ffffff"),
    surface_2: fixed("#f1f3f6"),
    border: fixed("#e4e7ec"),
    border_strong: fixed("#d3d8e0"),
    text: fixed("#15181e"),
    text_2: fixed("#40444d"),
    text_3: fixed("#626671"),
    accent: fixed("#4f5ee8"),
    accent_ink: fixed("#4c5ade"),
    checked_count: fixed("#404cbb"),
    accent_hover: fixed("#6170ee"),
    accent_soft: fixed("rgba(79, 94, 232, 0.10)"),
    on_accent: fixed("#ffffff"),
    ok: fixed("#10774e"),
    warn: fixed("#8d6004"),
    danger: fixed("#c03045"),
    card_hover: fixed("#e8ebf0"),
    shadow: fixed("rgba(16, 24, 40, 0.18)"),
};

/// The scheme the window asks for. With an Omarchy theme, the theme's:
/// dark or light as it says. Otherwise dark unless the system prefers
/// light (libadwaita's own default is the other way round), and light for
/// the light screenshot whatever the system says.
pub fn color_scheme(screenshot: Option<Scene>, omarchy: Option<&OmarchyTheme>) -> ColorScheme {
    match (omarchy, screenshot) {
        (Some(theme), _) if theme.dark => ColorScheme::ForceDark,
        (Some(_), _) => ColorScheme::ForceLight,
        (None, Some(Scene::Light)) => ColorScheme::ForceLight,
        (None, _) => ColorScheme::PreferDark,
    }
}

/// The palette drawn: the Omarchy theme's, or the window's own for a dark
/// or a light scheme.
pub fn palette(dark: bool, omarchy: Option<&OmarchyTheme>) -> Palette {
    match omarchy {
        Some(theme) => theme.palette.clone(),
        None if dark => DARK,
        None => LIGHT,
    }
}

const TEMPLATE: &str = include_str!("style.css");

/// The stylesheet for `palette`.
pub fn stylesheet(palette: &Palette) -> String {
    let mut css = TEMPLATE.to_owned();
    for (name, value) in palette.tokens() {
        css = css.replace(&format!("{{{{{name}}}}}"), value);
    }
    css
}

/// The theme Omarchy has applied, as the window draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OmarchyTheme {
    /// Its name as Omarchy shows it ("Osaka Jade"), when Omarchy says.
    pub name: Option<String>,
    pub dark: bool,
    pub palette: Palette,
}

/// An Omarchy theme for the screenshot of one: made up for the README,
/// not one of Omarchy's own.
pub const SAMPLE: &str = include_str!("omarchy-sample.toml");

/// Where Omarchy keeps the theme it applied, under `home`: Omarchy 4 in
/// the state directory, Omarchy 3 in the config one. Each holds the
/// theme's files in `theme/` and its name in `theme.name`.
pub fn omarchy_dirs(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".local/state/omarchy/current"),
        home.join(".config/omarchy/current"),
    ]
}

/// Reads the theme Omarchy applied: the first of `dirs` with a palette
/// (`theme/colors.toml`). Says why when there is none, or it cannot be
/// drawn.
pub fn read_omarchy(dirs: &[PathBuf]) -> Result<OmarchyTheme, String> {
    for dir in dirs {
        let path = dir.join("theme/colors.toml");
        let Some(colors) = read_theme_file(&path)? else {
            continue;
        };
        let mut theme =
            parse_omarchy(&colors).map_err(|error| format!("{}: {error}", path.display()))?;
        theme.name = read_theme_file(&dir.join("theme.name"))
            .ok()
            .flatten()
            .map(|slug| title(slug.trim()))
            .filter(|name| !name.is_empty());
        return Ok(theme);
    }
    Err(match dirs.iter().find(|dir| dir.join("theme").is_dir()) {
        Some(dir) => format!("no colors.toml in {}", dir.join("theme").display()),
        None => match dirs.first() {
            Some(dir) => format!("no Omarchy theme in {}", dir.join("theme").display()),
            None => "no Omarchy theme: no home directory".to_owned(),
        },
    })
}

/// The most of a theme file that is read: a palette is a few hundred
/// bytes, and a file far bigger is not one.
const MAX_THEME_FILE: u64 = 64 * 1024;

/// The text of the theme file at `path`, `None` when there is none. Only
/// a regular file (or a link to one) is read, and only up to
/// [`MAX_THEME_FILE`]: a theme from someone else's repository must not
/// stall the window.
fn read_theme_file(path: &Path) -> Result<Option<String>, String> {
    let failed = |why: &dyn std::fmt::Display| format!("{}: {why}", path.display());
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(failed(&error)),
    };
    if !metadata.is_file() {
        return Err(failed(&"not a file"));
    }
    let mut text = String::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_THEME_FILE + 1).read_to_string(&mut text))
        .map_err(|error| failed(&error))?;
    if text.len() as u64 > MAX_THEME_FILE {
        return Err(failed(&format!(
            "larger than {} KiB",
            MAX_THEME_FILE / 1024
        )));
    }
    Ok(Some(text))
}

/// The folder to watch for Omarchy switching themes, among `dirs`: the
/// one read (holding `theme/colors.toml`), else the first there is. Each
/// switch replaces `theme/` there, or the link of that name on Omarchy 3,
/// and rewrites `theme.name`. `None` without Omarchy.
pub fn omarchy_watch_dir(dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter()
        .find(|dir| dir.join("theme/colors.toml").is_file())
        .or_else(|| dirs.iter().find(|dir| dir.is_dir()))
        .cloned()
}

/// A theme's slug as Omarchy shows it: "osaka-jade" is "Osaka Jade".
fn title(slug: &str) -> String {
    slug.split(['-', '_', ' '])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// How far each colour sits from the one it is mixed from in the window's
/// own palette, so a theme is drawn with the same structure in its own
/// colours: the surfaces and borders from the background towards the text
/// (a light scheme's cards towards white), the quieter texts from the text
/// towards the background, the hover from the accent towards white.
struct Mix {
    surface: f64,
    surface_2: f64,
    border: f64,
    border_strong: f64,
    card_hover: f64,
    text_2: f64,
    text_3: f64,
    accent_hover: f64,
    accent_soft_alpha: f64,
    shadow: &'static str,
    /// The states a theme leaves out.
    own: Palette,
}

static DARK_MIX: Mix = Mix {
    surface: 0.039,
    surface_2: 0.076,
    border: 0.125,
    border_strong: 0.198,
    card_hover: 0.112,
    text_2: 0.275,
    text_3: 0.524,
    accent_hover: 0.119,
    accent_soft_alpha: 0.14,
    shadow: "rgba(0, 0, 0, 0.45)",
    own: DARK,
};

static LIGHT_MIX: Mix = Mix {
    surface: 0.945,
    surface_2: 0.016,
    border: 0.069,
    border_strong: 0.132,
    card_hover: 0.052,
    text_2: 0.313,
    text_3: 0.555,
    accent_hover: 0.109,
    accent_soft_alpha: 0.10,
    shadow: "rgba(16, 24, 40, 0.18)",
    own: LIGHT,
};

/// Reads Omarchy's `colors.toml`: its background, text and accent are
/// needed; green, yellow and red are the states, the window's own when it
/// leaves them out. It is dark or light as its `mode` says, else as its
/// background looks.
pub fn parse_omarchy(colors: &str) -> Result<OmarchyTheme, String> {
    let table: toml::Table = colors
        .parse()
        .map_err(|error: toml::de::Error| error.message().to_owned())?;
    let colour = |key: &str| -> Result<Option<Rgb>, String> {
        match table.get(key) {
            None => Ok(None),
            Some(value) => value
                .as_str()
                .and_then(Rgb::parse)
                .map(Some)
                .ok_or_else(|| format!("{key} is not a colour")),
        }
    };
    let needed = |key: &str| colour(key)?.ok_or_else(|| format!("no {key} colour"));
    let bg = needed("background")?;
    let text = needed("foreground")?;
    let accent = needed("accent")?;
    let dark = match table.get("mode").and_then(toml::Value::as_str) {
        Some("dark") => true,
        Some("light") => false,
        _ => !bg.is_light(),
    };
    let mix = if dark { &DARK_MIX } else { &LIGHT_MIX };
    // A state without hue cannot say what it means, and a grey one reads
    // as switched off: it is drawn in the text colour instead.
    let state = |key: &str, own: &str| -> Result<Rgb, String> {
        Ok(match colour(key)? {
            Some(state) if state.saturation() > HUE_FLOOR => state,
            Some(_) => text,
            None => own_colour(own),
        })
    };
    let states = [
        state("green", &mix.own.ok)?,
        state("yellow", &mix.own.warn)?,
        state("red", &mix.own.danger)?,
    ];
    Ok(OmarchyTheme {
        name: None,
        dark,
        palette: Mixed::new(dark, bg, text, accent, states)
            .readable()
            .palette(),
    })
}

/// The contrast every colour written as text keeps with what it is
/// written on: WCAG AA for text.
const READS: f64 = 4.5;

/// The saturation under which a theme's state colour is taken for a grey.
const HUE_FLOOR: f64 = 0.2;

/// How much of a state its chip is tinted with.
const CHIP_TINT: f64 = 0.10;

/// How much of its yellow or red a stale or unavailable notice is tinted
/// with.
const NOTICE_TINT: f64 = 0.08;

/// How much of the accent the count on a checked tab is tinted with, over
/// the tab's own tint (`button.tab:checked .count` in the stylesheet).
const COUNT_TINT: f64 = 0.2;

/// `ink` walked towards `toward` only as far as it takes to `read`;
/// walking towards white or black keeps the hue. `toward` itself when
/// even that does not read.
fn walked(ink: Rgb, toward: Rgb, read: impl Fn(Rgb) -> bool) -> Rgb {
    if read(ink) {
        return ink;
    }
    let (mut short, mut far) = (0.0, 1.0);
    for _ in 0..16 {
        let half = (short + far) / 2.0;
        if read(ink.mix(toward, half)) {
            far = half;
        } else {
            short = half;
        }
    }
    ink.mix(toward, far)
}

/// Whether `ink` reads on every one of `grounds`.
fn reads_on(ink: Rgb, grounds: &[Rgb]) -> bool {
    grounds.iter().all(|ground| ink.contrast(*ground) >= READS)
}

/// One of the window's own colours, which are all written `#rrggbb`.
fn own_colour(colour: &str) -> Rgb {
    Rgb::parse(colour).expect("the window's own colours are colours")
}

/// A theme's colours in the window's own proportions.
#[derive(Clone, Copy, Debug)]
struct Mixed {
    dark: bool,
    bg: Rgb,
    surface: Rgb,
    surface_2: Rgb,
    border: Rgb,
    border_strong: Rgb,
    card_hover: Rgb,
    text: Rgb,
    text_2: Rgb,
    text_3: Rgb,
    accent: Rgb,
    accent_ink: Rgb,
    checked_count: Rgb,
    on_accent: Rgb,
    ok: Rgb,
    warn: Rgb,
    danger: Rgb,
}

impl Mixed {
    /// The palette mixed from a theme's background, text and accent, with
    /// its green, yellow and red as the states.
    fn new(dark: bool, bg: Rgb, text: Rgb, accent: Rgb, [ok, warn, danger]: [Rgb; 3]) -> Mixed {
        let mix = Mixed::proportions(dark);
        let surface_to = if dark { text } else { Rgb::WHITE };
        Mixed {
            dark,
            bg,
            surface: bg.mix(surface_to, mix.surface),
            surface_2: bg.mix(text, mix.surface_2),
            border: bg.mix(text, mix.border),
            border_strong: bg.mix(text, mix.border_strong),
            card_hover: bg.mix(text, mix.card_hover),
            text,
            text_2: text.mix(bg, mix.text_2),
            text_3: text.mix(bg, mix.text_3),
            accent,
            accent_ink: accent,
            checked_count: accent,
            on_accent: on(accent, if dark { bg } else { text }),
            ok,
            warn,
            danger,
        }
    }

    /// The same palette with every colour written as text made to read
    /// on the grounds the stylesheet writes it on: the states first, since
    /// notices are tinted with them, then the accent as text (and the
    /// count on a checked tab), the text on the accent and the texts. The second text keeps its place between
    /// the text and the third, so the three keep their order.
    fn readable(self) -> Mixed {
        // Past this luminance, black stands out more on the background
        // than white does: the way a colour can be made to read, whatever
        // the theme says it is.
        let toward = if self.bg.luminance() > 0.179 {
            Rgb::BLACK
        } else {
            Rgb::WHITE
        };
        let soft = Mixed::proportions(self.dark).accent_soft_alpha;
        // The page, a group, a card and a card under the pointer.
        let plain = [self.bg, self.surface, self.surface_2, self.card_hover];
        // A state is also written on its chip, on the page.
        let state = |state: Rgb| {
            walked(state, toward, |ink| {
                reads_on(ink, &plain) && reads_on(ink, &[self.bg.mix(ink, CHIP_TINT)])
            })
        };
        let [ok, warn, danger] = [self.ok, self.warn, self.danger].map(state);
        // The accent is also written on a checked tab or filter.
        let checked = self.bg.mix(self.accent, soft);
        let accent_ink = walked(self.accent, toward, |ink| {
            reads_on(ink, &plain) && reads_on(ink, &[checked])
        });
        // The count on a checked tab sits on more of the accent still, so
        // it goes further only there.
        let count = checked.mix(self.accent, COUNT_TINT);
        let checked_count = walked(accent_ink, toward, |ink| reads_on(ink, &[count]));
        let on_accent = if self.on_accent.contrast(self.accent) >= READS {
            self.on_accent
        } else {
            let dark = if self.dark { self.bg } else { self.text };
            let deeper = walked(dark, Rgb::BLACK, |ink| reads_on(ink, &[self.accent]));
            if self.accent.contrast(Rgb::WHITE) >= self.accent.contrast(deeper) {
                Rgb::WHITE
            } else {
                deeper
            }
        };
        // The texts are also written on a marked card, on a count and on
        // a stale or unavailable notice.
        let mut texts = plain.to_vec();
        texts.extend([
            self.surface_2.mix(self.accent, soft),
            self.border,
            self.surface.mix(warn, NOTICE_TINT),
            self.surface.mix(danger, NOTICE_TINT),
        ]);
        let text_ink = |ink: Rgb| walked(ink, toward, |ink| reads_on(ink, &texts));
        let text = text_ink(self.text);
        let text_3 = text_ink(self.text_3);
        let mix = Mixed::proportions(self.dark);
        let text_2 = text_ink(text.mix(text_3, mix.text_2 / mix.text_3));
        Mixed {
            text,
            text_2,
            text_3,
            accent_ink,
            checked_count,
            on_accent,
            ok,
            warn,
            danger,
            ..self
        }
    }

    fn proportions(dark: bool) -> &'static Mix {
        if dark { &DARK_MIX } else { &LIGHT_MIX }
    }

    fn palette(&self) -> Palette {
        let mix = Mixed::proportions(self.dark);
        let hex = |colour: Rgb| -> Cow<'static, str> { colour.hex().into() };
        Palette {
            bg: hex(self.bg),
            surface: hex(self.surface),
            surface_2: hex(self.surface_2),
            border: hex(self.border),
            border_strong: hex(self.border_strong),
            text: hex(self.text),
            text_2: hex(self.text_2),
            text_3: hex(self.text_3),
            accent: hex(self.accent),
            accent_ink: hex(self.accent_ink),
            checked_count: hex(self.checked_count),
            accent_hover: hex(self.accent.mix(Rgb::WHITE, mix.accent_hover)),
            accent_soft: self.accent.rgba(mix.accent_soft_alpha).into(),
            on_accent: hex(self.on_accent),
            ok: hex(self.ok),
            warn: hex(self.warn),
            danger: hex(self.danger),
            card_hover: hex(self.card_hover),
            shadow: fixed(mix.shadow),
        }
    }
}

/// The text drawn on `accent`: white or the theme's `dark` colour,
/// whichever stands out more on it (many themes have a pale accent).
fn on(accent: Rgb, dark: Rgb) -> Rgb {
    if accent.contrast(dark) > accent.contrast(Rgb::WHITE) {
        dark
    } else {
        Rgb::WHITE
    }
}

/// A colour as Omarchy writes one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rgb([u8; 3]);

impl Rgb {
    const WHITE: Rgb = Rgb([255; 3]);
    const BLACK: Rgb = Rgb([0; 3]);

    /// `#rgb` or `#rrggbb`, or `#rrggbbaa` with its alpha left out.
    fn parse(text: &str) -> Option<Rgb> {
        let hex = text.trim().strip_prefix('#')?;
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let channel = |at: usize, len: usize| u8::from_str_radix(&hex[at..at + len], 16).ok();
        match hex.len() {
            3 => Some(Rgb([
                channel(0, 1)? * 17,
                channel(1, 1)? * 17,
                channel(2, 1)? * 17,
            ])),
            6 | 8 => Some(Rgb([channel(0, 2)?, channel(2, 2)?, channel(4, 2)?])),
            _ => None,
        }
    }

    /// This colour moved `amount` (0 to 1) of the way to `other`.
    fn mix(self, other: Rgb, amount: f64) -> Rgb {
        Rgb(std::array::from_fn(|at| {
            let (from, to) = (f64::from(self.0[at]), f64::from(other.0[at]));
            (from + (to - from) * amount).round() as u8
        }))
    }

    fn hex(self) -> String {
        let [r, g, b] = self.0;
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    fn rgba(self, alpha: f64) -> String {
        let [r, g, b] = self.0;
        format!("rgba({r}, {g}, {b}, {alpha:.2})")
    }

    /// Whether it reads as a light colour: its luma is over half.
    fn is_light(self) -> bool {
        let [r, g, b] = self.0.map(f64::from);
        0.2126 * r + 0.7152 * g + 0.0722 * b > 127.5
    }

    /// How much colour it has, from 0 (a grey) to 1: its HSV saturation.
    fn saturation(self) -> f64 {
        let max = self.0.into_iter().max().unwrap_or(0);
        let min = self.0.into_iter().min().unwrap_or(0);
        if max == 0 {
            0.0
        } else {
            f64::from(max - min) / f64::from(max)
        }
    }

    /// Its relative luminance, as WCAG defines it.
    fn luminance(self) -> f64 {
        let linear = |channel: u8| {
            let channel = f64::from(channel) / 255.0;
            if channel <= 0.04045 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        };
        let [r, g, b] = self.0.map(linear);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    /// The contrast ratio with `other`, from 1 to 21.
    fn contrast(self, other: Rgb) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every token in the template is known, so no placeholder is left in
    /// either scheme, and the schemes differ.
    #[test]
    fn fills_every_token() {
        let dark = stylesheet(&DARK);
        let light = stylesheet(&LIGHT);
        for css in [&dark, &light] {
            assert!(!css.contains("{{"), "unfilled token in {css}");
            assert!(css.contains(".card"));
        }
        assert_ne!(dark, light);
        assert!(dark.contains(&*DARK.accent));
        assert!(light.contains(&*LIGHT.accent));
    }

    /// Dark is the default: light only when the system prefers it, which
    /// libadwaita works out from "prefer dark", or for the light picture.
    #[test]
    fn opens_dark_unless_asked_for_light() {
        assert_eq!(color_scheme(None, None), ColorScheme::PreferDark);
        assert_eq!(
            color_scheme(Some(Scene::Activity), None),
            ColorScheme::PreferDark
        );
        assert_eq!(
            color_scheme(Some(Scene::Board), None),
            ColorScheme::PreferDark
        );
        assert_eq!(
            color_scheme(Some(Scene::Light), None),
            ColorScheme::ForceLight
        );
    }

    /// An Omarchy theme says whether it is dark, whatever the system or
    /// the picture asks for, and its palette is the one drawn; without one
    /// the window's own palette follows the scheme.
    #[test]
    fn omarchy_decides_the_scheme_and_the_palette() {
        let dark = parse_omarchy(SAMPLE).unwrap();
        let light = parse_omarchy(&light_colors()).unwrap();
        assert!(dark.dark);
        assert!(!light.dark);
        assert_eq!(color_scheme(None, Some(&dark)), ColorScheme::ForceDark);
        assert_eq!(
            color_scheme(Some(Scene::Light), Some(&dark)),
            ColorScheme::ForceDark
        );
        assert_eq!(color_scheme(None, Some(&light)), ColorScheme::ForceLight);
        assert_eq!(palette(false, Some(&dark)), dark.palette);
        assert_eq!(palette(true, None), DARK);
        assert_eq!(palette(false, None), LIGHT);
    }

    fn light_colors() -> String {
        "background = \"#faf4ed\"\nforeground = \"#575279\"\naccent = \"#d7827e\"\n".to_owned()
    }

    fn rgb(colour: &str) -> [i32; 3] {
        let hex = colour.trim_start_matches('#');
        [0, 2, 4].map(|at| i32::from_str_radix(&hex[at..at + 2], 16).unwrap())
    }

    /// An Omarchy theme is drawn with the window's own proportions, mixed
    /// from the theme's colours and made to read by the same rule: fed the
    /// main colours of the window's own palettes, it gives those palettes
    /// back, to within a few steps of 255 (the window's own greys lean a
    /// little to blue, mixes do not).
    #[test]
    fn mixes_omarchy_colours_in_the_house_proportions() {
        for (dark, house) in [(true, DARK), (false, LIGHT)] {
            let colour = |colour: &str| Rgb::parse(colour).unwrap();
            let states = [&house.ok, &house.warn, &house.danger].map(|state| colour(state));
            let palette = Mixed::new(
                dark,
                colour(&house.bg),
                colour(&house.text),
                colour(&house.accent),
                states,
            )
            .readable()
            .palette();
            let mode = if dark { "dark" } else { "light" };
            for ((name, mixed), (_, own)) in palette.tokens().into_iter().zip(house.tokens()) {
                if own.starts_with('#') {
                    let gap = rgb(mixed)
                        .into_iter()
                        .zip(rgb(own))
                        .map(|(a, b)| (a - b).abs())
                        .max()
                        .unwrap();
                    assert!(gap <= 8, "{mode} {name}: {mixed} is not {own}");
                } else {
                    assert_eq!(mixed, own, "{mode} {name}");
                }
            }
        }
    }

    /// The theme's own colours are drawn as they are: its background, its
    /// text, its accent and its green, yellow and red for the states; the
    /// stylesheet is filled.
    #[test]
    fn draws_the_theme_colours_as_they_are() {
        let theme = parse_omarchy(SAMPLE).unwrap();
        let colours: std::collections::BTreeMap<String, String> = toml::from_str(SAMPLE).unwrap();
        assert_eq!(theme.palette.bg, colours["background"]);
        assert_eq!(theme.palette.text, colours["foreground"]);
        assert_eq!(theme.palette.accent, colours["accent"]);
        assert_eq!(theme.palette.ok, colours["green"]);
        assert_eq!(theme.palette.warn, colours["yellow"]);
        assert_eq!(theme.palette.danger, colours["red"]);
        let css = stylesheet(&theme.palette);
        assert!(!css.contains("{{"));
        assert!(css.contains(&*theme.palette.accent));
    }

    /// Text on the accent is white or the theme's dark colour (its
    /// background on a dark theme, its text on a light one), whichever
    /// stands out more on the accent.
    #[test]
    fn writes_on_the_accent_in_what_stands_out() {
        let pale = parse_omarchy(SAMPLE).unwrap();
        assert_eq!(pale.palette.on_accent, pale.palette.bg);
        let jade = parse_omarchy(
            "background = \"#111c18\"\nforeground = \"#c1c497\"\naccent = \"#509475\"",
        )
        .unwrap();
        assert_eq!(jade.palette.on_accent, "#111c18");
        let deep = parse_omarchy(
            "background = \"#111c18\"\nforeground = \"#c1c497\"\naccent = \"#3b4cca\"",
        )
        .unwrap();
        assert_eq!(deep.palette.on_accent, "#ffffff");
        let light = parse_omarchy(
            "background = \"#eff1f5\"\nforeground = \"#4c4f69\"\naccent = \"#dce0e8\"",
        )
        .unwrap();
        assert_eq!(light.palette.on_accent, "#4c4f69");
    }

    /// Without `mode`, the background tells: a light one makes a light
    /// theme. A short `#rgb` colour is read; the states the theme leaves
    /// out are the window's own for its scheme, deepened where the theme's
    /// ground needs it.
    #[test]
    fn works_out_what_a_theme_leaves_out() {
        let light = parse_omarchy(&light_colors()).unwrap();
        assert!(!light.dark);
        let (own, drawn) = (colour(&LIGHT.ok), colour(&light.palette.ok));
        assert!(drawn.contrast(colour(&light.palette.bg)) >= 4.5);
        assert!((hue(own) - hue(drawn)).abs() <= 6.0, "{drawn:?}");
        let short =
            parse_omarchy("background = \"#111\"\nforeground = \"#eee\"\naccent = \"#7aa2f7\"")
                .unwrap();
        assert!(short.dark);
        assert_eq!(short.palette.bg, "#111111");
        assert_eq!(short.palette.danger, DARK.danger);
    }

    /// A theme without its background, text or accent, with a colour that
    /// is not one, or that is not TOML at all is refused, saying why.
    #[test]
    fn refuses_a_theme_it_cannot_draw() {
        let no_accent = "background = \"#111111\"\nforeground = \"#eeeeee\"";
        assert!(parse_omarchy(no_accent).unwrap_err().contains("accent"));
        let bad = "background = \"#11\"\nforeground = \"#eeeeee\"\naccent = \"#7aa2f7\"";
        assert!(parse_omarchy(bad).unwrap_err().contains("background"));
        let named = "background = \"black\"\nforeground = \"#eeeeee\"\naccent = \"#7aa2f7\"";
        assert!(parse_omarchy(named).unwrap_err().contains("background"));
        assert!(parse_omarchy("not toml = = =").is_err());
        // What is not a colour is not repeated back, however long.
        let long = format!(
            "background = \"{}\"\nforeground = \"#eeeeee\"\naccent = \"#7aa2f7\"",
            "x".repeat(10_000)
        );
        assert!(parse_omarchy(&long).unwrap_err().len() < 100);
    }

    /// Contrast is measured the way WCAG measures it.
    #[test]
    fn measures_contrast_as_wcag_does() {
        let black = Rgb([0; 3]);
        assert!((Rgb::WHITE.contrast(black) - 21.0).abs() < 1e-9);
        assert!((Rgb::WHITE.contrast(Rgb::WHITE) - 1.0).abs() < 1e-9);
        let grey = Rgb::parse("#777777").unwrap();
        assert!((grey.contrast(Rgb::WHITE) - 4.48).abs() < 0.01);
    }

    /// Themes from Omarchy's own set that leave some of their colours
    /// hard to read on the window's grounds, as `colors.toml` has them:
    /// mode, background, foreground, accent, green, yellow and red.
    const HARD: [[&str; 7]; 9] = [
        [
            "light", "#faf4ed", "#575279", "#56949f", "#286983", "#ea9d34", "#b4637a",
        ],
        [
            "light", "#eff1f5", "#4c4f69", "#1e66f5", "#40a02b", "#df8e1d", "#d20f39",
        ],
        [
            "light", "#FFFCF0", "#100F0F", "#205EA6", "#879A39", "#D0A215", "#D14D41",
        ],
        [
            "dark", "#222222", "#c2c2b0", "#78824b", "#5f875f", "#b36d43", "#685742",
        ],
        [
            "dark", "#2e3440", "#d8dee9", "#81a1c1", "#a3be8c", "#ebcb8b", "#bf616a",
        ],
        [
            "dark", "#1f1f28", "#dcd7ba", "#dcd7ba", "#76946a", "#c0a36e", "#c34043",
        ],
        [
            "dark", "#0c0b0c", "#FAFCFB", "#b59790", "#87a9b0", "#6B5E73", "#c38b7b",
        ],
        [
            "dark", "#101315", "#cacccc", "#798186", "#9fa5a9", "#d9dbdc", "#565d60",
        ],
        [
            "light", "#ffffff", "#000000", "#6e6e6e", "#3a3a3a", "#4a4a4a", "#2a2a2a",
        ],
    ];

    fn hard_theme([mode, bg, fg, accent, green, yellow, red]: [&str; 7]) -> OmarchyTheme {
        parse_omarchy(&format!(
            "mode = \"{mode}\"\nbackground = \"{bg}\"\nforeground = \"{fg}\"\n\
             accent = \"{accent}\"\ngreen = \"{green}\"\nyellow = \"{yellow}\"\n\
             red = \"{red}\"\n"
        ))
        .unwrap()
    }

    fn colour(token: &str) -> Rgb {
        Rgb::parse(token).unwrap()
    }

    /// The grounds the stylesheet writes the texts on: the page, a group,
    /// a card, a card under the pointer and a marked one, a count and a
    /// stale or unavailable notice.
    fn grounds(palette: &Palette, dark: bool) -> Vec<Rgb> {
        let soft = if dark { 0.14 } else { 0.10 };
        let card = colour(&palette.surface_2);
        let group = colour(&palette.surface);
        vec![
            colour(&palette.bg),
            group,
            card,
            colour(&palette.card_hover),
            card.mix(colour(&palette.accent), soft),
            colour(&palette.border),
            group.mix(colour(&palette.warn), 0.08),
            group.mix(colour(&palette.danger), 0.08),
        ]
    }

    fn worst(ink: Rgb, grounds: &[Rgb]) -> f64 {
        grounds
            .iter()
            .map(|ground| ink.contrast(*ground))
            .fold(f64::INFINITY, f64::min)
    }

    /// Everything written in `palette`'s colours reads at 4.5 to 1 (WCAG
    /// AA) on every ground it is written on: the texts on all of theirs;
    /// the accent as text on the page, a group, a card, a card under the
    /// pointer and a checked tab; the count on a checked tab on its chip
    /// (a fifth of the accent over the tab); each state there and on its
    /// own chip (a tenth of it over the page); and the text on the accent.
    fn assert_reads(palette: &Palette, dark: bool, what: &str) {
        let what = |name: &str| format!("{name} in {what}");
        for (name, ink) in [
            ("text", &palette.text),
            ("text-2", &palette.text_2),
            ("text-3", &palette.text_3),
        ] {
            assert!(
                worst(colour(ink), &grounds(palette, dark)) >= 4.5,
                "{}",
                what(name)
            );
        }
        let bg = colour(&palette.bg);
        let accent = colour(&palette.accent);
        let mut plain = grounds(palette, dark)[..4].to_vec();
        let soft = if dark { 0.14 } else { 0.10 };
        let checked = bg.mix(accent, soft);
        let ink = colour(&palette.accent_ink);
        assert!(worst(ink, &plain) >= 4.5, "{}", what("accent-ink"));
        assert!(
            ink.contrast(checked) >= 4.5,
            "{}",
            what("accent-ink, checked")
        );
        assert!(
            colour(&palette.checked_count).contrast(checked.mix(accent, 0.2)) >= 4.5,
            "{}",
            what("checked-count")
        );
        for (name, state) in [
            ("ok", &palette.ok),
            ("warn", &palette.warn),
            ("danger", &palette.danger),
        ] {
            let state = colour(state);
            plain.push(bg.mix(state, 0.10));
            assert!(worst(state, &plain) >= 4.5, "{}", what(name));
            plain.pop();
        }
        assert!(
            colour(&palette.on_accent).contrast(accent) >= 4.5,
            "{}",
            what("on-accent")
        );
    }

    /// Everything the window writes in a theme's colours reads.
    #[test]
    fn writes_every_theme_colour_so_it_reads() {
        for colours in HARD {
            let theme = hard_theme(colours);
            assert_reads(&theme.palette, theme.dark, colours[1]);
        }
    }

    /// The window's own palettes are held to the same: everything written
    /// in them reads at 4.5 to 1 on its grounds.
    #[test]
    fn writes_the_window_palettes_so_they_read() {
        assert_reads(&DARK, true, "the dark palette");
        assert_reads(&LIGHT, false, "the light palette");
    }

    /// The hue (0 to 360) of a colour with one.
    fn hue(Rgb([r, g, b]): Rgb) -> f64 {
        let [r, g, b] = [r, g, b].map(f64::from);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let turn = if max == r {
            ((g - b) / (max - min)).rem_euclid(6.0)
        } else if max == g {
            (b - r) / (max - min) + 2.0
        } else {
            (r - g) / (max - min) + 4.0
        };
        turn * 60.0
    }

    /// A colour made easier to read keeps its hue, and one that already
    /// reads is drawn as it is.
    #[test]
    fn keeps_the_hue_of_what_it_makes_readable() {
        for colours in HARD {
            let theme = hard_theme(colours);
            let palette = &theme.palette;
            for (given, drawn) in [
                (colours[3], &palette.accent_ink),
                (colours[4], &palette.ok),
                (colours[5], &palette.warn),
                (colours[6], &palette.danger),
            ] {
                let (given, drawn) = (colour(given), colour(drawn));
                if drawn == colour(&palette.text) || given.saturation() <= 0.2 {
                    continue;
                }
                let gap = (hue(given) - hue(drawn)).abs();
                assert!(gap.min(360.0 - gap) <= 6.0, "{given:?} became {drawn:?}");
            }
        }
        let nord = hard_theme(HARD[4]);
        assert_eq!(nord.palette.ok, "#a3be8c");
        assert_eq!(nord.palette.warn, "#ebcb8b");
        let sample = parse_omarchy(SAMPLE).unwrap();
        assert_eq!(sample.palette.accent_ink, sample.palette.accent);
    }

    /// The texts keep their order on every ground: the text stands out
    /// most, the second text less, the third least.
    #[test]
    fn keeps_the_texts_in_their_order() {
        for colours in HARD.into_iter().chain([[
            "dark", "#1d1a17", "#e8dcc8", "#e3a15c", "#9cbf6d", "#e6c15a", "#e06c5f",
        ]]) {
            let theme = hard_theme(colours);
            let palette = &theme.palette;
            for ground in grounds(palette, theme.dark) {
                let [text, second, third] = [&palette.text, &palette.text_2, &palette.text_3]
                    .map(|ink| colour(ink).contrast(ground));
                assert!(
                    text >= second && second > third,
                    "{} on {ground:?}",
                    colours[1]
                );
            }
        }
        for (house, dark) in [(DARK, true), (LIGHT, false)] {
            for ground in grounds(&house, dark) {
                let [text, second, third] = [&house.text, &house.text_2, &house.text_3]
                    .map(|ink| colour(ink).contrast(ground));
                assert!(text >= second && second > third, "{dark} on {ground:?}");
            }
        }
    }

    /// The window's own colours as the rule sees them.
    fn mixed_of(palette: &Palette, dark: bool) -> Mixed {
        Mixed {
            dark,
            bg: colour(&palette.bg),
            surface: colour(&palette.surface),
            surface_2: colour(&palette.surface_2),
            border: colour(&palette.border),
            border_strong: colour(&palette.border_strong),
            card_hover: colour(&palette.card_hover),
            text: colour(&palette.text),
            text_2: colour(&palette.text_2),
            text_3: colour(&palette.text_3),
            accent: colour(&palette.accent),
            accent_ink: colour(&palette.accent_ink),
            checked_count: colour(&palette.checked_count),
            on_accent: colour(&palette.on_accent),
            ok: colour(&palette.ok),
            warn: colour(&palette.warn),
            danger: colour(&palette.danger),
        }
    }

    /// The window's own palettes are made to read by the same rule as a
    /// theme's, and no further: the rule finds nothing left to change in
    /// them.
    #[test]
    fn makes_the_window_palettes_read_by_the_same_rule() {
        for (house, dark) in [(DARK, true), (LIGHT, false)] {
            let before = mixed_of(&house, dark);
            let after = before.readable();
            for (name, own, ruled) in [
                ("text", before.text, after.text),
                ("text-2", before.text_2, after.text_2),
                ("text-3", before.text_3, after.text_3),
                ("accent-ink", before.accent_ink, after.accent_ink),
                ("checked-count", before.checked_count, after.checked_count),
                ("on-accent", before.on_accent, after.on_accent),
                ("ok", before.ok, after.ok),
                ("warn", before.warn, after.warn),
                ("danger", before.danger, after.danger),
            ] {
                assert_eq!(own.hex(), ruled.hex(), "{name}, dark: {dark}");
            }
        }
    }

    /// A colour is made easier to read towards whichever of white or
    /// black can stand out on the theme's background, even from a theme
    /// that says it is dark over a light one.
    #[test]
    fn makes_colours_readable_away_from_the_background() {
        let theme = hard_theme([
            "dark", "#f4f1ea", "#3c3a36", "#c7a26a", "#9fbf7f", "#e0c070", "#e09080",
        ]);
        assert!(theme.dark);
        let palette = &theme.palette;
        for ink in [&palette.text_3, &palette.accent_ink, &palette.warn] {
            assert!(colour(ink).contrast(colour(&palette.bg)) >= 4.5, "{ink}");
        }
    }

    /// A state the theme draws without hue (a grey green, yellow or red)
    /// cannot say what it means, and a grey one reads as switched off: it
    /// is drawn in the theme's text colour. One with hue keeps its own.
    #[test]
    fn draws_a_state_without_hue_in_the_text_colour() {
        let solitude = hard_theme(HARD[7]);
        for state in [
            &solitude.palette.ok,
            &solitude.palette.warn,
            &solitude.palette.danger,
        ] {
            assert_eq!(state, &solitude.palette.text);
        }
        let white = hard_theme(HARD[8]);
        assert_eq!(white.palette.danger, white.palette.text);
        let horizon = hard_theme(HARD[6]);
        assert_eq!(horizon.palette.warn, horizon.palette.text);
        assert_eq!(horizon.palette.ok, "#87a9b0");
    }

    /// Omarchy 4 keeps the theme in use under the state directory, and
    /// Omarchy 3 kept it under the config one: the first with a palette
    /// is read, named after Omarchy's own `theme.name`.
    #[test]
    fn reads_the_theme_omarchy_applied() {
        let home =
            std::env::temp_dir().join(format!("tasks-pending-omarchy-{}", std::process::id()));
        let dirs = omarchy_dirs(&home);
        assert_eq!(
            dirs,
            [
                home.join(".local/state/omarchy/current"),
                home.join(".config/omarchy/current"),
            ]
        );
        assert!(
            read_omarchy(&dirs)
                .unwrap_err()
                .contains("no Omarchy theme")
        );
        assert_eq!(omarchy_watch_dir(&dirs), None, "no Omarchy here");

        let old = &dirs[1];
        std::fs::create_dir_all(old.join("theme")).unwrap();
        let refused = read_omarchy(&dirs).unwrap_err();
        assert!(
            refused.contains("no colors.toml in") && refused.contains(".config/omarchy"),
            "{refused}"
        );
        assert_eq!(omarchy_watch_dir(&dirs).as_ref(), Some(old));
        std::fs::write(old.join("theme/colors.toml"), light_colors()).unwrap();
        let read = read_omarchy(&dirs).unwrap();
        assert!(!read.dark);
        assert_eq!(read.name, None);

        let new = &dirs[0];
        std::fs::create_dir_all(new.join("theme")).unwrap();
        std::fs::write(new.join("theme/colors.toml"), SAMPLE).unwrap();
        std::fs::write(new.join("theme.name"), "osaka-jade\n").unwrap();
        let read = read_omarchy(&dirs).unwrap();
        assert!(read.dark);
        assert_eq!(read.name.as_deref(), Some("Osaka Jade"));
        assert_eq!(omarchy_watch_dir(&dirs).as_ref(), Some(new));

        let colors = new.join("theme/colors.toml");
        std::fs::write(&colors, "background = 1").unwrap();
        let refused = read_omarchy(&dirs).unwrap_err();
        assert!(refused.contains("colors.toml"), "{refused}");

        // A file far bigger than a palette, or one that is not a file (a
        // link to a device that never ends), is not read.
        std::fs::write(&colors, " ".repeat(100_000) + SAMPLE).unwrap();
        let refused = read_omarchy(&dirs).unwrap_err();
        assert!(refused.contains("larger than"), "{refused}");
        std::fs::remove_file(&colors).unwrap();
        std::os::unix::fs::symlink("/dev/zero", &colors).unwrap();
        let refused = read_omarchy(&dirs).unwrap_err();
        assert!(refused.contains("not a file"), "{refused}");
        let _ = std::fs::remove_dir_all(&home);
    }
}
