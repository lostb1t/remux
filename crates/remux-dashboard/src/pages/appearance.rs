use crate::{
    components::{Card, ThemeModeSegment},
    theme::{is_valid_hex, ThemePrefs, ACCENT_PRESETS, SCALE_OPTIONS},
};
use dioxus::prelude::*;

/// Appearance settings: theme mode, accent color (presets + custom), and UI
/// scale. Every control mutates the shared [`ThemePrefs`] signal, which the
/// `App` root reflects to the DOM and persists — so changes preview instantly.
#[component]
pub fn AppearancePage() -> Element {
    let mut prefs = use_context::<Signal<ThemePrefs>>();
    let current = prefs
        .read()
        .clone();

    rsx! {
        Card { title: "Theme",
            div { class: "field",
                span { class: "field-label", "Mode" }
                ThemeModeSegment {}
                p { class: "field-hint",
                    "Auto follows your device's light / dark setting; Light and Dark override it."
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
                div { style: "display:flex;align-items:center;gap:10px",
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
                    span { class: "kv-value", "{current.accent}" }
                }
            }
        }

        Card { title: "Scale",
            div { class: "field",
                span { class: "field-label", "Interface size" }
                select {
                    class: "select-input",
                    value: "{current.scale}",
                    onchange: move |e| {
                        if let Ok(n) = e.value().parse::<u8>() {
                            prefs.write().scale = n;
                        }
                    },
                    for opt in SCALE_OPTIONS.iter().copied() {
                        option {
                            value: "{opt}",
                            selected: current.scale == opt,
                            "{opt}%"
                        }
                    }
                }
                p { class: "field-hint", "Scales the entire admin interface up or down." }
            }
        }
    }
}
