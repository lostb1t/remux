use crate::theme::{ThemeMode, ThemePrefs};
use dioxus::prelude::*;

/// A compact segmented **Auto / Light / Dark** control bound to the shared
/// [`ThemePrefs`] context. Used both in the sidebar footer (quick toggle) and
/// on the Appearance settings page, so the two always stay in sync.
#[component]
pub fn ThemeModeSegment() -> Element {
    let mut prefs = use_context::<Signal<ThemePrefs>>();
    let current = prefs
        .read()
        .mode;
    rsx! {
        div { class: "theme-seg", role: "group", aria_label: "Theme",
            for mode in ThemeMode::ALL {
                button {
                    r#type: "button",
                    class: if current == mode {
                        "theme-seg-btn theme-seg-btn--active"
                    } else {
                        "theme-seg-btn"
                    },
                    aria_pressed: current == mode,
                    onclick: move |_| prefs.write().mode = mode,
                    "{mode.label()}"
                }
            }
        }
    }
}
