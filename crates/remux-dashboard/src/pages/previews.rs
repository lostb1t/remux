use crate::{
    components::{Card, ErrorAlert, LoadingText, SuccessAlert, ToggleRow},
    router::Route,
    state::AppState,
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    GetSystemConfiguration, ServerConfiguration, UpdateSystemConfiguration,
};

#[derive(Clone, Copy)]
struct SeekPreviewSettingsState {
    config: Signal<Option<ServerConfiguration>>,
    enabled: Signal<bool>,
    cache_mb: Signal<i64>,
    keep_days: Signal<i64>,
    start_at_position: Signal<bool>,
    loading: Signal<bool>,
    saving: Signal<bool>,
    saved: Signal<bool>,
    error: Signal<Option<String>>,
}

#[component]
pub fn SeekPreviewSettingsPage(app_state: AppState) -> Element {
    let mut state = SeekPreviewSettingsState {
        config: use_signal(|| None),
        enabled: use_signal(|| false),
        cache_mb: use_signal(|| 1024),
        keep_days: use_signal(|| 30),
        start_at_position: use_signal(|| false),
        loading: use_signal(|| true),
        saving: use_signal(|| false),
        saved: use_signal(|| false),
        error: use_signal(|| None),
    };
    use_context_provider(|| state);

    let mut load_state = state;
    let load_client = app_state.clone();
    use_effect(move || {
        let client = load_client.clone();
        spawn(async move {
            match client
                .execute(GetSystemConfiguration)
                .await
            {
                Ok(cfg) => {
                    load_state
                        .enabled
                        .set(
                            cfg.trickplay_while_streaming
                                .unwrap_or(false),
                        );
                    load_state
                        .cache_mb
                        .set(
                            cfg.trickplay_cache_size_mb
                                .unwrap_or(1024),
                        );
                    load_state
                        .keep_days
                        .set(
                            cfg.trickplay_keep_days
                                .unwrap_or(30),
                        );
                    load_state
                        .start_at_position
                        .set(
                            cfg.trickplay_start_at_position
                                .unwrap_or(false),
                        );
                    load_state
                        .config
                        .set(Some(cfg));
                }
                Err(error) => load_state
                    .error
                    .set(Some(format!(
                        "Failed to load seek preview settings: {error}"
                    ))),
            }
            load_state
                .loading
                .set(false);
        });
    });

    let on_submit = move |event: Event<FormData>| {
        event.prevent_default();
        let Some(config) = state
            .config
            .peek()
            .clone()
        else {
            return;
        };
        let updated = ServerConfiguration {
            trickplay_while_streaming: Some(
                *state
                    .enabled
                    .peek(),
            ),
            trickplay_cache_size_mb: Some(
                *state
                    .cache_mb
                    .peek(),
            ),
            trickplay_keep_days: Some(
                *state
                    .keep_days
                    .peek(),
            ),
            trickplay_start_at_position: Some(
                *state
                    .start_at_position
                    .peek(),
            ),
            ..config
        };
        state
            .saving
            .set(true);
        state
            .saved
            .set(false);
        state
            .error
            .set(None);
        let client = app_state.clone();
        spawn(async move {
            match client
                .execute(UpdateSystemConfiguration {
                    config: updated.clone(),
                })
                .await
            {
                Ok(_) => {
                    state
                        .config
                        .set(Some(updated));
                    state
                        .saved
                        .set(true);
                }
                Err(error) => state
                    .error
                    .set(Some(error.user_message())),
            }
            state
                .saving
                .set(false);
        });
    };

    rsx! {
        if *state.loading.read() {
            LoadingText {}
        } else {
            form {
                onsubmit: on_submit,
                style: "display:flex;flex-direction:column;gap:14px",
                PreviewsMainPanel {}
                PreviewsStoragePanel {}
                PreviewsStartPanel {}
                PreviewsAddonPanel {}

                if let Some(error) = state.error.read().as_ref() {
                    ErrorAlert { message: error.clone() }
                }
                if *state.saved.read() {
                    SuccessAlert { message: "Seek preview settings saved.".to_string() }
                }
                div { class: "form-actions",
                    button {
                        r#type: "submit",
                        class: "btn btn-primary",
                        disabled: *state.saving.read() || state.config.peek().is_none(),
                        if *state.saving.read() { "Saving…" } else { "Save settings" }
                    }
                }
            }
        }
    }
}

const INFO_TEXT: &str =
    "font-size:.8rem;color:var(--text-secondary);line-height:1.5;margin:0";

/// A paragraph of explanation, as on the RemuxDB page.
#[component]
fn Info(text: &'static str) -> Element {
    rsx! {
        p { style: INFO_TEXT, "{text}" }
    }
}

#[component]
fn PreviewsMainPanel() -> Element {
    let mut state = use_context::<SeekPreviewSettingsState>();
    rsx! {
        Card { title: "Seek previews",
            div { style: "display:flex;flex-direction:column;gap:14px",
                Info { text: "Seek previews are the small pictures a client shows while you seek through a video. Remux makes them in the background while a video plays, reading the file through its own proxy." }
                Info { text: "Until a video's previews are complete, each play reads up to one extra copy of the file. That holds a second connection to the source, uses about one CPU core, and counts toward debrid, usenet and data limits. A source that allows only one connection may refuse it or interrupt playback." }
                Info { text: "Seek previews are not made for Live TV, torrents, HLS streams, music, or videos under 5 minutes or over 4 hours." }
                ToggleRow {
                    label: "Make seek previews while streaming",
                    description: "Read files from addons with \"seek previews\" switched on, and make previews in the background.",
                    checked: *state.enabled.read(),
                    disabled: *state.saving.read(),
                    on_change: move |value| {
                        state.enabled.set(value);
                        state.saved.set(false);
                    }
                }
            }
        }
    }
}

#[component]
fn PreviewsStoragePanel() -> Element {
    let mut state = use_context::<SeekPreviewSettingsState>();
    let disabled = *state
        .saving
        .read()
        || !*state
            .enabled
            .read();
    rsx! {
        Card { title: "Storage",
            div { style: "display:flex;flex-direction:column;gap:14px",
                div { class: "field",
                    label { class: "field-label", r#for: "trickplay-cache", "Preview cache size (MB)" }
                    input {
                        id: "trickplay-cache",
                        r#type: "number",
                        class: "field-input",
                        min: "100",
                        value: "{state.cache_mb}",
                        disabled,
                        oninput: move |event| {
                            if let Ok(value) = event.value().parse::<i64>() {
                                state.cache_mb.set(value);
                                state.saved.set(false);
                            }
                        },
                    }
                    p { class: "field-hint", "A two-hour movie takes about 9 MB. When full, the previews shown longest ago are deleted first. Minimum 100. Default: 1024." }
                }
                div { class: "field",
                    label { class: "field-label", r#for: "trickplay-keep", "Keep previews for (days)" }
                    input {
                        id: "trickplay-keep",
                        r#type: "number",
                        class: "field-input",
                        min: "0",
                        max: "3650",
                        value: "{state.keep_days}",
                        disabled,
                        oninput: move |event| {
                            if let Ok(value) = event.value().parse::<i64>() {
                                state.keep_days.set(value);
                                state.saved.set(false);
                            }
                        },
                    }
                    p { class: "field-hint", "Previews not shown for this many days are deleted. 0 keeps them until the cache is full. Default: 30." }
                }
            }
        }
    }
}

#[component]
fn PreviewsStartPanel() -> Element {
    let mut state = use_context::<SeekPreviewSettingsState>();
    rsx! {
        Card { title: "Start position",
            div { class: "field",
                ToggleRow {
                    label: "Start at the playback position",
                    description: "Start at the position being watched and follow up to 4 seeks, instead of working from the start.",
                    checked: *state.start_at_position.read(),
                    disabled: *state.saving.read() || !*state.enabled.read(),
                    on_change: move |value| {
                        state.start_at_position.set(value);
                        state.saved.set(false);
                    }
                }
                div { class: "field-hint", "Clients that load every preview when play starts may show the wrong picture until a video's previews are complete. Each followed seek may briefly open another connection to the source." }
            }
        }
    }
}

#[component]
fn PreviewsAddonPanel() -> Element {
    rsx! {
        Card {
            title: "Addons",
            action: rsx! {
                button {
                    r#type: "button",
                    class: "btn btn-ghost",
                    style: "height:32px;font-size:.68rem",
                    onclick: move |_| {
                        navigator().push(Route::AddonsRoute);
                    },
                    "Open Addons"
                }
            },
            div { style: "display:flex;flex-direction:column;gap:14px",
                Info { text: "Remux only reads addons with \"seek previews\" switched on under Addons, Edit, Resources. With \"stream\" also on, it reads the version being played, or another version of the same movie or episode with the same length. IPTV addons and addons that bypass the Remux proxy are never read." }
                Info { text: "An addon kept just for previews can make them quicker. Previews only use the picture, so its results can leave out the language and quality filters viewers need, and put the smallest, quickest-loading files first. Remux still needs a file the same length as the one being played. Switch \"seek previews\" on and \"stream\" off for it." }
                Info { text: "After a play starts, Remux asks it for that movie or episode's streams in the background, at most once a day per title, and never offers its streams to viewers." }
                Info { text: "Another version is a file nobody chose to play, and reading it counts toward that source's limits. A debrid file that has left the cache can start a download in your account. Its previews may come from a different cut of the same length." }
            }
        }
    }
}
