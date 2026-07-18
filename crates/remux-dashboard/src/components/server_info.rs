use crate::{
    components::{Card, LoadingText},
    state::AppState,
};
use dioxus::prelude::*;
use remux_sdks::remux::{GetItemCounts, ItemCounts, PublicSystemInfo};

#[component]
pub fn KvRow(label: &'static str, value: String) -> Element {
    rsx! {
        div { class: "kv-row",
            span { class: "kv-label", "{label}" }
            span { class: "kv-value", "{value}" }
        }
    }
}

/// Group digits with thousands separators (`104727` → `104,727`) so large
/// library counts read at a glance. Pure, so it is unit-tested.
pub fn fmt_count(n: impl Into<i64>) -> String {
    let n = n.into();
    let digits = n
        .unsigned_abs()
        .to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3 + 1);
    for (i, ch) in digits
        .chars()
        .enumerate()
    {
        // Insert a separator before every group of three from the right.
        if i != 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// A single dashboard statistic: a large tabular number over a label.
#[component]
pub fn StatTile(num: String, label: &'static str) -> Element {
    rsx! {
        div { class: "stat-tile",
            span { class: "stat-num", "{num}" }
            span { class: "stat-key", "{label}" }
        }
    }
}

#[component]
pub fn ServerInfoCard(app_state: AppState) -> Element {
    let mut server_info: Signal<Option<PublicSystemInfo>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);

    use_effect(move || {
        let client = app_state
            .client
            .clone();
        spawn(async move {
            match client
                .execute(PublicSystemInfo::default())
                .await
            {
                Ok(info) => {
                    server_info.set(Some(info));
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to fetch server info: {e}"))),
            }
            loading.set(false);
        });
    });

    rsx! {
        Card { title: "Server",
            if *loading.read() {
                LoadingText {}
            } else if let Some(err) = error.read().as_ref() {
                span { class: "loading-text", style: "color:var(--error)", "{err}" }
            } else if let Some(info) = server_info.read().as_ref() {
                KvRow { label: "Name", value: info.server_name.clone() }
                KvRow { label: "Version", value: info.remux_version.clone() }
            }
        }
    }
}

#[component]
pub fn MediaStatsCard(app_state: AppState) -> Element {
    let mut counts: Signal<Option<ItemCounts>> = use_signal(|| None);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);

    use_effect(move || {
        let client = app_state
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetItemCounts)
                .await
            {
                Ok(c) => {
                    counts.set(Some(c));
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to fetch media counts: {e}"))),
            }
            loading.set(false);
        });
    });

    rsx! {
        Card { title: "Library",
            if *loading.read() {
                LoadingText {}
            } else if let Some(err) = error.read().as_ref() {
                span { class: "loading-text", style: "color:var(--error)", "{err}" }
            } else if let Some(c) = counts.read().as_ref() {
                div { class: "stat-grid",
                    StatTile { num: fmt_count(c.movie_count), label: "Movies" }
                    StatTile { num: fmt_count(c.series_count), label: "Series" }
                    StatTile { num: fmt_count(c.episode_count), label: "Episodes" }
                    StatTile { num: fmt_count(c.album_count), label: "Albums" }
                    StatTile { num: fmt_count(c.song_count), label: "Tracks" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fmt_count;

    #[test]
    fn fmt_count_groups_thousands() {
        assert_eq!(fmt_count(0), "0");
        assert_eq!(fmt_count(42), "42");
        assert_eq!(fmt_count(999), "999");
        assert_eq!(fmt_count(1000), "1,000");
        assert_eq!(fmt_count(5310), "5,310");
        assert_eq!(fmt_count(715062), "715,062");
        assert_eq!(fmt_count(1234567), "1,234,567");
        assert_eq!(fmt_count(-1234), "-1,234");
    }
}
