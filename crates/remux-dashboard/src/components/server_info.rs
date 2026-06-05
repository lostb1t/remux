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
                KvRow { label: "Movies", value: c.movie_count.to_string() }
                KvRow { label: "Series", value: c.series_count.to_string() }
                KvRow { label: "Episodes", value: c.episode_count.to_string() }
                KvRow { label: "Albums", value: c.album_count.to_string() }
                KvRow { label: "Tracks", value: c.song_count.to_string() }
            }
        }
    }
}
