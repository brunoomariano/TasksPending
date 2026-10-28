//! The app's look: its own palette (dark by default, light when the system
//! asks), written into the stylesheet GTK loads. GTK CSS has no variables
//! on every version the app runs on, so the tokens are filled in here.

use libadwaita::ColorScheme;

use crate::Scene;

/// The colours of one scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub bg: &'static str,
    pub surface: &'static str,
    pub surface_2: &'static str,
    pub border: &'static str,
    pub border_strong: &'static str,
    pub text: &'static str,
    pub text_2: &'static str,
    pub text_3: &'static str,
    pub accent: &'static str,
    /// The accent, lightened for a hover.
    pub accent_hover: &'static str,
    /// The accent at low alpha, for tinted surfaces.
    pub accent_soft: &'static str,
    pub ok: &'static str,
    pub warn: &'static str,
    pub danger: &'static str,
    /// A card under the pointer.
    pub card_hover: &'static str,
    /// Dialog shadow.
    pub shadow: &'static str,
}

pub const DARK: Palette = Palette {
    bg: "#0e0f12",
    surface: "#16181d",
    surface_2: "#1c1f25",
    border: "#262a31",
    border_strong: "#343a43",
    text: "#e7e9ee",
    text_2: "#a4abb8",
    text_3: "#6d7482",
    accent: "#7c8cff",
    accent_hover: "#8c9aff",
    accent_soft: "rgba(124, 140, 255, 0.14)",
    ok: "#4ccb8f",
    warn: "#f2c14e",
    danger: "#f2667a",
    card_hover: "#23272e",
    shadow: "rgba(0, 0, 0, 0.45)",
};

pub const LIGHT: Palette = Palette {
    bg: "#f6f7f9",
    surface: "#ffffff",
    surface_2: "#f1f3f6",
    border: "#e4e7ec",
    border_strong: "#d3d8e0",
    text: "#15181e",
    text_2: "#555d6b",
    text_3: "#8a91a0",
    accent: "#4f5ee8",
    accent_hover: "#6170ee",
    accent_soft: "rgba(79, 94, 232, 0.10)",
    ok: "#16a36a",
    warn: "#c98a06",
    danger: "#d9364e",
    card_hover: "#e8ebf0",
    shadow: "rgba(16, 24, 40, 0.18)",
};

/// The scheme the window asks for: dark unless the system prefers light
/// (libadwaita's own default is the other way round), and light for the
/// light screenshot whatever the system says.
pub fn color_scheme(screenshot: Option<Scene>) -> ColorScheme {
    match screenshot {
        Some(Scene::Light) => ColorScheme::ForceLight,
        _ => ColorScheme::PreferDark,
    }
}

const TEMPLATE: &str = include_str!("style.css");

/// The stylesheet for `palette`.
pub fn stylesheet(palette: &Palette) -> String {
    let tokens = [
        ("bg", palette.bg),
        ("surface", palette.surface),
        ("surface-2", palette.surface_2),
        ("border", palette.border),
        ("border-strong", palette.border_strong),
        ("text", palette.text),
        ("text-2", palette.text_2),
        ("text-3", palette.text_3),
        ("accent", palette.accent),
        ("accent-hover", palette.accent_hover),
        ("accent-soft", palette.accent_soft),
        ("ok", palette.ok),
        ("warn", palette.warn),
        ("danger", palette.danger),
        ("card-hover", palette.card_hover),
        ("shadow", palette.shadow),
    ];
    let mut css = TEMPLATE.to_owned();
    for (name, value) in tokens {
        css = css.replace(&format!("{{{{{name}}}}}"), value);
    }
    css
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
        assert!(dark.contains(DARK.accent));
        assert!(light.contains(LIGHT.accent));
    }

    /// Dark is the default: light only when the system prefers it, which
    /// libadwaita works out from "prefer dark", or for the light picture.
    #[test]
    fn opens_dark_unless_asked_for_light() {
        assert_eq!(color_scheme(None), ColorScheme::PreferDark);
        assert_eq!(color_scheme(Some(Scene::Board)), ColorScheme::PreferDark);
        assert_eq!(color_scheme(Some(Scene::Light)), ColorScheme::ForceLight);
    }
}
