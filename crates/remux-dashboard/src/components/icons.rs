//! Inline SVG line-icons for the sidebar navigation.
//!
//! Kept as a single `NavIcon` component with a `name` → paths match so nav items
//! stay declarative (`NavIcon { name: "library" }`). Icons are 24×24 stroke
//! icons using `currentColor`, so they inherit the nav item's text color and
//! active-state accent automatically.

use dioxus::prelude::*;

/// A 24×24 stroke icon selected by `name`. Unknown names fall back to a dot.
#[component]
pub fn NavIcon(name: &'static str) -> Element {
    let inner = icon_paths(name);
    rsx! {
        svg {
            class: "nav-icon",
            xmlns: "http://www.w3.org/2000/svg",
            width: "18",
            height: "18",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.75",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            {inner}
        }
    }
}

fn icon_paths(name: &str) -> Element {
    match name {
        "dashboard" => rsx! {
            rect { x: "3", y: "3", width: "7", height: "9", rx: "1" }
            rect { x: "14", y: "3", width: "7", height: "5", rx: "1" }
            rect { x: "14", y: "12", width: "7", height: "9", rx: "1" }
            rect { x: "3", y: "16", width: "7", height: "5", rx: "1" }
        },
        "addons" => rsx! {
            path { d: "M12 2 2 7l10 5 10-5-10-5Z" }
            path { d: "m2 17 10 5 10-5" }
            path { d: "m2 12 10 5 10-5" }
        },
        "tasks" => rsx! {
            path { d: "m3 17 2 2 4-4" }
            path { d: "m3 7 2 2 4-4" }
            line { x1: "13", y1: "6", x2: "21", y2: "6" }
            line { x1: "13", y1: "12", x2: "21", y2: "12" }
            line { x1: "13", y1: "18", x2: "21", y2: "18" }
        },
        "content" => rsx! {
            path { d: "M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" }
        },
        "library" => rsx! {
            path { d: "m16 6 4 14" }
            path { d: "M12 6v14" }
            path { d: "M8 8v12" }
            path { d: "M4 4v16" }
        },
        "iptv" => rsx! {
            rect { x: "2", y: "7", width: "20", height: "15", rx: "2" }
            path { d: "m17 2-5 5-5-5" }
        },
        "streaming" => rsx! {
            path { d: "M2 8a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-6" }
            path { d: "M2 12a4 4 0 0 1 4 4" }
            path { d: "M2 16a1 1 0 0 1 1 1" }
        },
        "groups" => rsx! {
            path { d: "M12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83Z" }
            path { d: "m22 17.65-9.17 4.16a2 2 0 0 1-1.66 0L2 17.65" }
            path { d: "m22 12.65-9.17 4.16a2 2 0 0 1-1.66 0L2 12.65" }
        },
        "probing" => rsx! {
            path { d: "M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2" }
        },
        "p2p" => rsx! {
            circle { cx: "18", cy: "5", r: "3" }
            circle { cx: "6", cy: "12", r: "3" }
            circle { cx: "18", cy: "19", r: "3" }
            line { x1: "8.59", y1: "13.51", x2: "15.42", y2: "17.49" }
            line { x1: "15.41", y1: "6.51", x2: "8.59", y2: "10.49" }
        },
        "settings" => rsx! {
            path { d: "M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2Z" }
            circle { cx: "12", cy: "12", r: "3" }
        },
        "general" => rsx! {
            line { x1: "4", y1: "21", x2: "4", y2: "14" }
            line { x1: "4", y1: "10", x2: "4", y2: "3" }
            line { x1: "12", y1: "21", x2: "12", y2: "12" }
            line { x1: "12", y1: "8", x2: "12", y2: "3" }
            line { x1: "20", y1: "21", x2: "20", y2: "16" }
            line { x1: "20", y1: "12", x2: "20", y2: "3" }
            line { x1: "1", y1: "14", x2: "7", y2: "14" }
            line { x1: "9", y1: "8", x2: "15", y2: "8" }
            line { x1: "17", y1: "16", x2: "23", y2: "16" }
        },
        "playback" => rsx! {
            circle { cx: "12", cy: "12", r: "10" }
            polygon { points: "10 8 16 12 10 16 10 8" }
        },
        "search" => rsx! {
            circle { cx: "11", cy: "11", r: "8" }
            line { x1: "21", y1: "21", x2: "16.65", y2: "16.65" }
        },
        "sync" => rsx! {
            path { d: "M3 12a9 9 0 0 1 15-6.7L21 8" }
            path { d: "M21 3v5h-5" }
            path { d: "M21 12a9 9 0 0 1-15 6.7L3 16" }
            path { d: "M3 21v-5h5" }
        },
        "intro" => rsx! {
            polygon { points: "5 4 15 12 5 20 5 4" }
            line { x1: "19", y1: "5", x2: "19", y2: "19" }
        },
        "branding" => rsx! {
            circle { cx: "13.5", cy: "6.5", r: ".5", fill: "currentColor" }
            circle { cx: "17.5", cy: "10.5", r: ".5", fill: "currentColor" }
            circle { cx: "8.5", cy: "7.5", r: ".5", fill: "currentColor" }
            circle { cx: "6.5", cy: "12.5", r: ".5", fill: "currentColor" }
            path { d: "M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1 1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2Z" }
        },
        "appearance" => rsx! {
            path { d: "M2 12s3-7 10-7 10 7 10 7-3 7-10 7-10-7-10-7Z" }
            circle { cx: "12", cy: "12", r: "3" }
        },
        "access" => rsx! {
            path { d: "M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1Z" }
        },
        "users" => rsx! {
            path { d: "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" }
            circle { cx: "9", cy: "7", r: "4" }
            path { d: "M22 21v-2a4 4 0 0 0-3-3.87" }
            path { d: "M16 3.13a4 4 0 0 1 0 7.75" }
        },
        "api-keys" => rsx! {
            circle { cx: "7.5", cy: "15.5", r: "5.5" }
            path { d: "m21 2-9.6 9.6" }
            path { d: "m15.5 7.5 3 3L22 7l-3-3" }
        },
        "devices" => rsx! {
            rect { x: "2", y: "4", width: "14", height: "12", rx: "2" }
            rect { x: "17", y: "9", width: "5", height: "11", rx: "1.5" }
            line { x1: "6", y1: "20", x2: "11", y2: "20" }
        },
        "system" => rsx! {
            rect { x: "2", y: "2", width: "20", height: "8", rx: "2" }
            rect { x: "2", y: "14", width: "20", height: "8", rx: "2" }
            line { x1: "6", y1: "6", x2: "6.01", y2: "6" }
            line { x1: "6", y1: "18", x2: "6.01", y2: "18" }
        },
        "activity" => rsx! {
            path { d: "M22 12h-4l-3 9L9 3l-3 9H2" }
        },
        "logs" => rsx! {
            path { d: "M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" }
            path { d: "M14 2v5h5" }
            line { x1: "8", y1: "13", x2: "16", y2: "13" }
            line { x1: "8", y1: "17", x2: "16", y2: "17" }
            line { x1: "8", y1: "9", x2: "10", y2: "9" }
        },
        "sessions" => rsx! {
            rect { x: "2", y: "3", width: "20", height: "14", rx: "2" }
            line { x1: "8", y1: "21", x2: "16", y2: "21" }
            line { x1: "12", y1: "17", x2: "12", y2: "21" }
        },
        _ => rsx! {
            circle { cx: "12", cy: "12", r: "9" }
        },
    }
}
