//! Appearance (theme) runtime for the admin dashboard.
//!
//! The color system lives in `assets/theme.css`; this module is the small Rust
//! layer that lets the user *choose* a theme and persists that choice.
//!
//! Responsibilities:
//! * Model the user's preferences ([`ThemePrefs`]: mode, accent color, UI scale).
//! * Persist/restore them via `LocalStorage` (same pattern as `state.rs`).
//! * Apply them to the live DOM:
//!   - the [`ThemeMode`] toggles the `data-theme` attribute on `<html>`
//!     (which flips CSS `color-scheme`); **System** removes the attribute so the
//!     OS `prefers-color-scheme` governs the theme with zero JS.
//!   - the accent color and UI scale are injected as `:root` custom-property
//!     overrides through a `<style>` node rendered by the `App` component
//!     (see [`theme_style_css`]).
//!
//! Only [`apply_mode_to_dom`] touches `web-sys`; everything else is pure and
//! unit-tested below.

use dioxus::prelude::*;
use gloo_storage::{LocalStorage, Storage};
use serde::{Deserialize, Serialize};

/// `LocalStorage` key under which appearance preferences are stored.
pub const THEME_KEY: &str = "remux_theme";

/// Default accent color (the Remux brand green).
pub const DEFAULT_ACCENT: &str = "#009245";

/// Default UI scale, as a whole percentage (100 = 1.0×).
pub const DEFAULT_SCALE: u8 = 100;

/// Smallest / largest UI scale the user may select (percent).
pub const MIN_SCALE: u8 = 85;
pub const MAX_SCALE: u8 = 125;

/// Discrete UI-scale choices offered in the Appearance page (percent).
pub const SCALE_OPTIONS: &[u8] = &[85, 90, 100, 110, 125];

/// Which color theme the dashboard renders in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    /// Follow the operating system's `prefers-color-scheme` (no `data-theme`).
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

impl ThemeMode {
    /// All variants in display order — handy for rendering a segmented control.
    pub const ALL: [ThemeMode; 3] =
        [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark];

    /// The `<html data-theme>` value for this mode, or `None` for [`System`].
    ///
    /// System returns `None` on purpose: with the attribute absent, the CSS
    /// `prefers-color-scheme` media query drives the palette and tracks the OS
    /// live, so no JS listener is needed.
    ///
    /// [`System`]: ThemeMode::System
    pub fn data_attr(self) -> Option<&'static str> {
        match self {
            ThemeMode::System => None,
            ThemeMode::Light => Some("light"),
            ThemeMode::Dark => Some("dark"),
        }
    }

    /// Short, user-facing label for controls.
    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::System => "Auto",
            ThemeMode::Light => "Light",
            ThemeMode::Dark => "Dark",
        }
    }
}

/// A named accent color offered as a one-click preset in the Appearance page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccentPreset {
    /// Human-readable name.
    pub name: &'static str,
    /// `#rrggbb` value written into `--accent`.
    pub hex: &'static str,
}

/// Built-in accent swatches. The first entry is the brand default. A broad,
/// harmonious spectrum so the user has "tons of color options".
pub const ACCENT_PRESETS: &[AccentPreset] = &[
    AccentPreset {
        name: "Emerald",
        hex: "#009245",
    },
    AccentPreset {
        name: "Green",
        hex: "#22c55e",
    },
    AccentPreset {
        name: "Teal",
        hex: "#14b8a6",
    },
    AccentPreset {
        name: "Cyan",
        hex: "#06b6d4",
    },
    AccentPreset {
        name: "Sky",
        hex: "#0ea5e9",
    },
    AccentPreset {
        name: "Blue",
        hex: "#3b82f6",
    },
    AccentPreset {
        name: "Indigo",
        hex: "#6366f1",
    },
    AccentPreset {
        name: "Violet",
        hex: "#8b5cf6",
    },
    AccentPreset {
        name: "Purple",
        hex: "#a855f7",
    },
    AccentPreset {
        name: "Fuchsia",
        hex: "#d946ef",
    },
    AccentPreset {
        name: "Pink",
        hex: "#ec4899",
    },
    AccentPreset {
        name: "Rose",
        hex: "#f43f5e",
    },
    AccentPreset {
        name: "Red",
        hex: "#ef4444",
    },
    AccentPreset {
        name: "Orange",
        hex: "#f97316",
    },
    AccentPreset {
        name: "Amber",
        hex: "#f59e0b",
    },
    AccentPreset {
        name: "Yellow",
        hex: "#eab308",
    },
    AccentPreset {
        name: "Lime",
        hex: "#84cc16",
    },
    AccentPreset {
        name: "Slate",
        hex: "#64748b",
    },
];

/// Default theme-preset id (the built-in Remux palette; no `data-preset`).
pub const DEFAULT_PRESET: &str = "default";

/// A named, complete color palette (surfaces + text) applied via the
/// `data-preset` attribute on `<html>`. The palette values themselves live in
/// `theme.css` under `:root[data-preset="<id>"]`; this struct is the registry
/// the picker renders from and pairs each preset with a fitting default accent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemePreset {
    /// `data-preset` value / storage id.
    pub id: &'static str,
    /// Human-readable name.
    pub name: &'static str,
    /// Accent this preset ships with (applied when the preset is chosen).
    pub accent: &'static str,
    /// Representative background color for the picker card preview.
    pub swatch_bg: &'static str,
    /// Representative surface color for the picker card preview.
    pub swatch_panel: &'static str,
}

/// Built-in theme presets. `default` is the native Remux palette.
pub const THEME_PRESETS: &[ThemePreset] = &[
    ThemePreset {
        id: "default",
        name: "Remux",
        accent: "#009245",
        swatch_bg: "#0b0c0f",
        swatch_panel: "#1a1d24",
    },
    ThemePreset {
        id: "midnight",
        name: "Midnight",
        accent: "#5b8def",
        swatch_bg: "#080b14",
        swatch_panel: "#131a2b",
    },
    ThemePreset {
        id: "nord",
        name: "Nord",
        accent: "#88c0d0",
        swatch_bg: "#2e3440",
        swatch_panel: "#3b4252",
    },
    ThemePreset {
        id: "dracula",
        name: "Dracula",
        accent: "#bd93f9",
        swatch_bg: "#282a36",
        swatch_panel: "#44475a",
    },
    ThemePreset {
        id: "catppuccin",
        name: "Catppuccin",
        accent: "#cba6f7",
        swatch_bg: "#1e1e2e",
        swatch_panel: "#302f42",
    },
    ThemePreset {
        id: "rose-pine",
        name: "Rosé Pine",
        accent: "#ebbcba",
        swatch_bg: "#191724",
        swatch_panel: "#26233a",
    },
    ThemePreset {
        id: "solarized",
        name: "Solarized",
        accent: "#268bd2",
        swatch_bg: "#002b36",
        swatch_panel: "#073642",
    },
    ThemePreset {
        id: "gruvbox",
        name: "Gruvbox",
        accent: "#fabd2f",
        swatch_bg: "#282828",
        swatch_panel: "#3a3735",
    },
];

/// The user's persisted appearance preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemePrefs {
    /// Light / dark / system.
    #[serde(default)]
    pub mode: ThemeMode,
    /// Accent color as `#rrggbb` (drives the whole primary family in CSS).
    #[serde(default = "default_accent")]
    pub accent: String,
    /// Theme-preset id (full palette), applied via `data-preset`. See
    /// [`THEME_PRESETS`]; [`DEFAULT_PRESET`] = the native Remux palette.
    #[serde(default = "default_preset")]
    pub preset: String,
    /// Global UI scale as a whole percentage (see [`MIN_SCALE`]/[`MAX_SCALE`]).
    #[serde(default = "default_scale")]
    pub scale: u8,
}

fn default_accent() -> String {
    DEFAULT_ACCENT.to_string()
}

fn default_preset() -> String {
    DEFAULT_PRESET.to_string()
}

fn default_scale() -> u8 {
    DEFAULT_SCALE
}

impl Default for ThemePrefs {
    fn default() -> Self {
        Self {
            mode: ThemeMode::default(),
            accent: default_accent(),
            preset: default_preset(),
            scale: default_scale(),
        }
    }
}

impl ThemePrefs {
    /// Load preferences from `LocalStorage`, falling back to defaults for any
    /// missing/corrupt data, and normalize the result.
    pub fn load() -> Self {
        let mut prefs: ThemePrefs = LocalStorage::get(THEME_KEY).unwrap_or_default();
        prefs.normalize();
        prefs
    }

    /// Persist preferences to `LocalStorage` (best-effort; ignores failure).
    pub fn save(&self) {
        let _ = LocalStorage::set(THEME_KEY, self);
    }

    /// Clamp/repair fields so a hand-edited or stale storage entry can never
    /// produce an invalid accent or an out-of-range scale.
    pub fn normalize(&mut self) {
        if !is_valid_hex(&self.accent) {
            self.accent = default_accent();
        }
        if !THEME_PRESETS
            .iter()
            .any(|p| p.id == self.preset)
        {
            self.preset = default_preset();
        }
        self.scale = self
            .scale
            .clamp(MIN_SCALE, MAX_SCALE);
    }

    /// UI scale as a CSS `zoom` factor (e.g. `110` → `1.10`).
    pub fn scale_factor(&self) -> f32 {
        self.scale as f32 / 100.0
    }
}

/// Return `true` if `s` is a `#rgb` or `#rrggbb` hex color.
///
/// Used to reject junk before it reaches `--accent` (a bad value would silently
/// break every accent surface).
pub fn is_valid_hex(s: &str) -> bool {
    let Some(hex) = s.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 6)
        && hex
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
}

/// Build the `<style>` body that injects the live accent + UI-scale overrides
/// onto `:root`. Rendered after the base stylesheet, so it wins for `--accent`
/// and `--ui-scale` while leaving every other token untouched.
pub fn theme_style_css(prefs: &ThemePrefs) -> String {
    format!(
        ":root{{--accent:{};--ui-scale:{:.2}}}",
        prefs.accent,
        prefs.scale_factor()
    )
}

/// Apply the theme *mode* and *preset* to `<html>` via `web-sys`:
/// `data-theme` for an explicit light/dark choice (removed for System), and
/// `data-preset` for a non-default palette (removed for the native Remux one).
pub fn apply_to_dom(prefs: &ThemePrefs) {
    let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    else {
        return;
    };
    match prefs
        .mode
        .data_attr()
    {
        Some(value) => {
            let _ = root.set_attribute("data-theme", value);
        }
        None => {
            let _ = root.remove_attribute("data-theme");
        }
    }
    if prefs.preset == DEFAULT_PRESET {
        let _ = root.remove_attribute("data-preset");
    } else {
        let _ = root.set_attribute("data-preset", &prefs.preset);
    }
}

/// Initialise the appearance system once at the root of the app.
///
/// Loads persisted preferences, applies the mode to the DOM immediately (to
/// avoid a flash of the wrong theme), provides the reactive [`ThemePrefs`]
/// signal as context, and keeps the DOM + storage in sync on every change.
/// Returns the signal so the caller can render the live accent/scale `<style>`.
pub fn use_theme() -> Signal<ThemePrefs> {
    // Load once and apply the mode synchronously on first render.
    let prefs = use_signal(|| {
        let prefs = ThemePrefs::load();
        apply_to_dom(&prefs);
        prefs
    });
    // Make preferences available to the sidebar control and Appearance page.
    use_context_provider(|| prefs);
    // Re-apply mode + preset and persist whenever any preference changes.
    use_effect(move || {
        let prefs = prefs
            .read()
            .clone();
        apply_to_dom(&prefs);
        prefs.save();
    });
    prefs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_data_attr_maps_correctly() {
        assert_eq!(ThemeMode::System.data_attr(), None);
        assert_eq!(ThemeMode::Light.data_attr(), Some("light"));
        assert_eq!(ThemeMode::Dark.data_attr(), Some("dark"));
    }

    #[test]
    fn mode_serde_roundtrips_lowercase() {
        assert_eq!(serde_json::to_string(&ThemeMode::Dark).unwrap(), "\"dark\"");
        let back: ThemeMode = serde_json::from_str("\"light\"").unwrap();
        assert_eq!(back, ThemeMode::Light);
    }

    #[test]
    fn prefs_roundtrip_preserves_values() {
        let prefs = ThemePrefs {
            mode: ThemeMode::Dark,
            accent: "#123abc".into(),
            preset: "nord".into(),
            scale: 110,
        };
        let json = serde_json::to_string(&prefs).unwrap();
        let back: ThemePrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(prefs, back);
    }

    #[test]
    fn prefs_defaults_fill_missing_fields() {
        // Empty object must yield the documented defaults.
        let prefs: ThemePrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(prefs.mode, ThemeMode::System);
        assert_eq!(prefs.accent, DEFAULT_ACCENT);
        assert_eq!(prefs.preset, DEFAULT_PRESET);
        assert_eq!(prefs.scale, DEFAULT_SCALE);
    }

    #[test]
    fn normalize_repairs_bad_accent_and_scale() {
        let mut prefs = ThemePrefs {
            mode: ThemeMode::Light,
            accent: "not-a-color".into(),
            preset: "bogus-preset".into(),
            scale: 250,
        };
        prefs.normalize();
        assert_eq!(prefs.accent, DEFAULT_ACCENT);
        assert_eq!(prefs.preset, DEFAULT_PRESET);
        assert_eq!(prefs.scale, MAX_SCALE);

        let mut tiny = ThemePrefs {
            scale: 10,
            ..ThemePrefs::default()
        };
        tiny.normalize();
        assert_eq!(tiny.scale, MIN_SCALE);
    }

    #[test]
    fn hex_validation() {
        assert!(is_valid_hex("#fff"));
        assert!(is_valid_hex("#00AA55"));
        assert!(!is_valid_hex("00AA55")); // missing '#'
        assert!(!is_valid_hex("#12")); // wrong length
        assert!(!is_valid_hex("#gggggg")); // non-hex digits
    }

    #[test]
    fn scale_factor_and_style_css() {
        let prefs = ThemePrefs {
            mode: ThemeMode::System,
            accent: "#009245".into(),
            preset: default_preset(),
            scale: 110,
        };
        assert!((prefs.scale_factor() - 1.10).abs() < f32::EPSILON);
        let css = theme_style_css(&prefs);
        assert!(css.contains("--accent:#009245"));
        assert!(css.contains("--ui-scale:1.10"));
    }

    /// Registry invariants: ids are unique, every accent is a valid hex, and the
    /// default preset id is present. Guards against a malformed preset/accent
    /// entry silently breaking `--accent` or the picker.
    #[test]
    fn theme_presets_registry_is_well_formed() {
        let mut ids = std::collections::HashSet::new();
        for preset in THEME_PRESETS {
            assert!(ids.insert(preset.id), "duplicate preset id: {}", preset.id);
            assert!(
                is_valid_hex(preset.accent),
                "preset {} has invalid accent {}",
                preset.id,
                preset.accent
            );
            assert!(
                is_valid_hex(preset.swatch_bg) && is_valid_hex(preset.swatch_panel),
                "preset {} has an invalid swatch color",
                preset.id
            );
        }
        assert!(
            THEME_PRESETS
                .iter()
                .any(|p| p.id == DEFAULT_PRESET),
            "the default preset id must exist in the registry"
        );
    }

    #[test]
    fn accent_presets_are_valid_hex_and_unique() {
        let mut hexes = std::collections::HashSet::new();
        for accent in ACCENT_PRESETS {
            assert!(
                is_valid_hex(accent.hex),
                "accent {} has invalid hex {}",
                accent.name,
                accent.hex
            );
            assert!(
                hexes.insert(accent.hex),
                "duplicate accent hex: {}",
                accent.hex
            );
        }
        assert_eq!(
            ACCENT_PRESETS[0].hex, DEFAULT_ACCENT,
            "first accent preset must be the brand default"
        );
    }

    #[test]
    fn scale_options_are_within_bounds() {
        for &s in SCALE_OPTIONS {
            assert!(
                (MIN_SCALE..=MAX_SCALE).contains(&s),
                "scale option {s} is out of [{MIN_SCALE}, {MAX_SCALE}]"
            );
        }
        assert!(
            SCALE_OPTIONS.contains(&DEFAULT_SCALE),
            "the default scale must be offered as an option"
        );
    }

    /// Mechanical registry↔CSS parity: every non-default preset in the registry
    /// MUST have a matching `:root[data-preset="<id>"]` block in `theme.css`,
    /// or selecting it would apply no palette. `default` uses bare `:root`.
    #[test]
    fn every_preset_has_a_css_block() {
        const THEME_CSS: &str = include_str!("../assets/theme.css");
        for preset in THEME_PRESETS {
            if preset.id == DEFAULT_PRESET {
                continue;
            }
            let selector = format!("data-preset=\"{}\"", preset.id);
            assert!(
                THEME_CSS.contains(&selector),
                "theme.css is missing a [{}] block for preset {}",
                selector,
                preset.name
            );
        }
    }
}
