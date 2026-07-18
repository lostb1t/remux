use crate::{
    components::{Card, Select, SelectOption, ThemeModeSegment},
    theme::{is_valid_hex, ThemePrefs, ACCENT_PRESETS, SCALE_OPTIONS, THEME_PRESETS},
};
use dioxus::prelude::*;

/// Appearance settings: theme mode, theme preset (full palette), accent color
/// (presets + custom), and UI scale. Every control mutates the shared
/// [`ThemePrefs`] signal, which the `App` root reflects to the DOM and persists —
/// so changes preview instantly across the whole admin panel.
#[component]
pub fn AppearancePage() -> Element {
    let mut prefs = use_context::<Signal<ThemePrefs>>();
    let current = prefs
        .read()
        .clone();

    rsx! {
        Card { title: "Mode",
            div { class: "field",
                ThemeModeSegment {}
                p { class: "field-hint",
                    "Auto follows your device's light / dark setting; Light and Dark override it."
                }
            }
        }

        Card { title: "Theme preset",
            div { class: "field",
                p { class: "field-hint", style: "margin-bottom:4px",
                    "A complete color palette. The accent below can still be customised on top."
                }
                div { class: "preset-grid",
                    for preset in THEME_PRESETS.iter() {
                        {
                            let id = preset.id.to_string();
                            let accent = preset.accent.to_string();
                            let selected = current.preset == preset.id;
                            rsx! {
                                button {
                                    r#type: "button",
                                    class: if selected {
                                        "preset-card preset-card--selected"
                                    } else {
                                        "preset-card"
                                    },
                                    aria_pressed: selected,
                                    onclick: move |_| {
                                        let mut p = prefs.write();
                                        p.preset = id.clone();
                                        p.accent = accent.clone();
                                    },
                                    // Miniature palette preview.
                                    div {
                                        class: "preset-swatch",
                                        style: "background:{preset.swatch_bg}",
                                        div {
                                            class: "preset-swatch-panel",
                                            style: "background:{preset.swatch_panel}",
                                        }
                                        div {
                                            class: "preset-swatch-dot",
                                            style: "background:{preset.accent}",
                                        }
                                    }
                                    span { class: "preset-name", "{preset.name}" }
                                }
                            }
                        }
                    }
                }
            }
        }

        Card { title: "Accent color",
            div { class: "field",
                span { class: "field-label", "Presets" }
                div { class: "accent-swatches",
                    for preset in ACCENT_PRESETS.iter() {
                        {
                            let hex = preset.hex.to_string();
                            let selected = current.accent.eq_ignore_ascii_case(preset.hex);
                            rsx! {
                                button {
                                    r#type: "button",
                                    class: if selected {
                                        "accent-swatch accent-swatch--selected"
                                    } else {
                                        "accent-swatch"
                                    },
                                    style: "background:{preset.hex}",
                                    title: "{preset.name}",
                                    aria_label: "{preset.name}",
                                    aria_pressed: selected,
                                    onclick: move |_| prefs.write().accent = hex.clone(),
                                }
                            }
                        }
                    }
                }
            }
            div { class: "field",
                span { class: "field-label", "Custom" }
                div { class: "accent-custom",
                    input {
                        r#type: "color",
                        class: "accent-color-input",
                        value: "{current.accent}",
                        // The native color input only ever yields valid hex, but
                        // guard anyway so bad values can never reach --accent.
                        oninput: move |e| {
                            let v = e.value();
                            if is_valid_hex(&v) {
                                prefs.write().accent = v;
                            }
                        },
                    }
                    span { class: "mono-value", "{current.accent}" }
                }
            }
        }

        Card { title: "Interface scale",
            div { class: "field",
                Select {
                    class: "max-w-[220px]".to_string(),
                    value: current.scale.to_string(),
                    options: SCALE_OPTIONS
                        .iter()
                        .map(|s| SelectOption::new(s.to_string(), format!("{s}%")))
                        .collect(),
                    on_change: move |v: String| {
                        if let Ok(n) = v.parse::<u8>() {
                            prefs.write().scale = n;
                        }
                    },
                }
                p { class: "field-hint", "Scales the entire admin interface up or down." }
            }
        }
    }
}
