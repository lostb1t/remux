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

/// Built-in accent presets. The first entry is the brand default.
pub const ACCENT_PRESETS: &[AccentPreset] = &[
    AccentPreset {
        name: "Emerald",
        hex: "#009245",
    },
    AccentPreset {
        name: "Blue",
        hex: "#2f80ed",
    },
    AccentPreset {
        name: "Indigo",
        hex: "#6366f1",
    },
    AccentPreset {
        name: "Teal",
        hex: "#0d9488",
    },
    AccentPreset {
        name: "Purple",
        hex: "#8b5cf6",
    },
    AccentPreset {
        name: "Orange",
        hex: "#e67e22",
    },
    AccentPreset {
        name: "Rose",
        hex: "#e5484d",
    },
    AccentPreset {
        name: "Slate",
        hex: "#64748b",
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
    /// Global UI scale as a whole percentage (see [`MIN_SCALE`]/[`MAX_SCALE`]).
    #[serde(default = "default_scale")]
    pub scale: u8,
}

fn default_accent() -> String {
    DEFAULT_ACCENT.to_string()
}

fn default_scale() -> u8 {
    DEFAULT_SCALE
}

impl Default for ThemePrefs {
    fn default() -> Self {
        Self {
            mode: ThemeMode::default(),
            accent: default_accent(),
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

/// Apply the theme *mode* to `<html>` via `web-sys`: set `data-theme` for an
/// explicit light/dark choice, or remove it for System.
pub fn apply_mode_to_dom(mode: ThemeMode) {
    let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    else {
        return;
    };
    match mode.data_attr() {
        Some(value) => {
            let _ = root.set_attribute("data-theme", value);
        }
        None => {
            let _ = root.remove_attribute("data-theme");
        }
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
        apply_mode_to_dom(prefs.mode);
        prefs
    });
    // Make preferences available to the sidebar control and Appearance page.
    use_context_provider(|| prefs);
    // Re-apply the mode and persist whenever any preference changes.
    use_effect(move || {
        let prefs = prefs
            .read()
            .clone();
        apply_mode_to_dom(prefs.mode);
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
        assert_eq!(prefs.scale, DEFAULT_SCALE);
    }

    #[test]
    fn normalize_repairs_bad_accent_and_scale() {
        let mut prefs = ThemePrefs {
            mode: ThemeMode::Light,
            accent: "not-a-color".into(),
            scale: 250,
        };
        prefs.normalize();
        assert_eq!(prefs.accent, DEFAULT_ACCENT);
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
            scale: 110,
        };
        assert!((prefs.scale_factor() - 1.10).abs() < f32::EPSILON);
        let css = theme_style_css(&prefs);
        assert!(css.contains("--accent:#009245"));
        assert!(css.contains("--ui-scale:1.10"));
    }
}
