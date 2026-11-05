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
    pub fn tokens(&self) -> [(&'static str, &str); 17] {
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
    text_2: fixed("#a4abb8"),
    text_3: fixed("#6d7482"),
    accent: fixed("#7c8cff"),
    accent_hover: fixed("#8c9aff"),
    accent_soft: fixed("rgba(124, 140, 255, 0.14)"),
    on_accent: fixed("#ffffff"),
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
    text_2: fixed("#555d6b"),
    text_3: fixed("#8a91a0"),
    accent: fixed("#4f5ee8"),
    accent_hover: fixed("#6170ee"),
    accent_soft: fixed("rgba(79, 94, 232, 0.10)"),
    on_accent: fixed("#ffffff"),
    ok: fixed("#16a36a"),
    warn: fixed("#c98a06"),
    danger: fixed("#d9364e"),
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

const DARK_MIX: Mix = Mix {
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

const LIGHT_MIX: Mix = Mix {
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
    let state = |key: &str, own: &Cow<'static, str>| -> Result<Cow<'static, str>, String> {
        Ok(colour(key)?.map_or_else(|| own.clone(), |colour| colour.hex().into()))
    };
    let surface_to = if dark { text } else { Rgb::WHITE };
    let palette = Palette {
        bg: bg.hex().into(),
        surface: bg.mix(surface_to, mix.surface).hex().into(),
        surface_2: bg.mix(text, mix.surface_2).hex().into(),
        border: bg.mix(text, mix.border).hex().into(),
        border_strong: bg.mix(text, mix.border_strong).hex().into(),
        text: text.hex().into(),
        text_2: text.mix(bg, mix.text_2).hex().into(),
        text_3: text.mix(bg, mix.text_3).hex().into(),
        accent: accent.hex().into(),
        accent_hover: accent.mix(Rgb::WHITE, mix.accent_hover).hex().into(),
        accent_soft: accent.rgba(mix.accent_soft_alpha).into(),
        on_accent: on(accent, if dark { bg } else { text }).hex().into(),
        ok: state("green", &mix.own.ok)?,
        warn: state("yellow", &mix.own.warn)?,
        danger: state("red", &mix.own.danger)?,
        card_hover: bg.mix(text, mix.card_hover).hex().into(),
        shadow: fixed(mix.shadow),
    };
    Ok(OmarchyTheme {
        name: None,
        dark,
        palette,
    })
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

    /// `colors.toml` with the main colours of `palette`, as Omarchy writes
    /// one.
    fn colors_of(mode: &str, palette: &Palette) -> String {
        format!(
            "# A theme\nmode = \"{mode}\"\n\naccent = \"{}\"\nbackground = \"{}\"\n\
             foreground = \"{}\"\ngreen = \"{}\"\nyellow = \"{}\"\nred = \"{}\"\n\
             muted = \"#53685B\"\n",
            palette.accent, palette.bg, palette.text, palette.ok, palette.warn, palette.danger
        )
    }

    fn light_colors() -> String {
        "background = \"#faf4ed\"\nforeground = \"#575279\"\naccent = \"#d7827e\"\n".to_owned()
    }

    fn rgb(colour: &str) -> [i32; 3] {
        let hex = colour.trim_start_matches('#');
        [0, 2, 4].map(|at| i32::from_str_radix(&hex[at..at + 2], 16).unwrap())
    }

    /// An Omarchy theme is drawn with the window's own proportions, mixed
    /// from the theme's colours: fed the main colours of the window's own
    /// palettes, it gives those palettes back, to within a few steps of
    /// 255 (the window's own greys lean a little to blue, mixes do not).
    #[test]
    fn mixes_omarchy_colours_in_the_house_proportions() {
        for (mode, house) in [("dark", DARK), ("light", LIGHT)] {
            let theme = parse_omarchy(&colors_of(mode, &house)).unwrap();
            assert_eq!(theme.dark, mode == "dark");
            // The window's own text on its accent is white by choice;
            // a theme's is whatever stands out more.
            for ((name, mixed), (_, own)) in theme.palette.tokens().into_iter().zip(house.tokens())
            {
                if name == "on-accent" {
                    continue;
                }
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
    /// out are the window's own.
    #[test]
    fn works_out_what_a_theme_leaves_out() {
        let light = parse_omarchy(&light_colors()).unwrap();
        assert!(!light.dark);
        assert_eq!(light.palette.ok, LIGHT.ok);
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
