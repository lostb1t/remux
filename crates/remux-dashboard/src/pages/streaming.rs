use crate::{
    components::{Card, ErrorAlert, LoadingText, SuccessAlert, ToggleRow},
    state::AppState,
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    GetSystemConfiguration, ServerConfiguration, SortMediaSourcesMode,
    UpdateSystemConfiguration,
};

#[derive(Clone, Copy)]
struct StreamingSettingsState {
    config: Signal<Option<ServerConfiguration>>,
    sort_mode: Signal<SortMediaSourcesMode>,
    show_decision: Signal<bool>,
    probe_timeout: Signal<i64>,
    probe_timeout_p2p: Signal<i64>,
    auto_next_stream: Signal<bool>,
    max_fallback_streams: Signal<i64>,
    p2p_enabled: Signal<bool>,
    p2p_upload_speed: Signal<i64>,
    p2p_download_speed: Signal<i64>,
    loading: Signal<bool>,
    saving: Signal<bool>,
    saved: Signal<bool>,
    error: Signal<Option<String>>,
}

#[component]
pub fn StreamingGeneralSettingsPage(app_state: AppState) -> Element {
    let mut state = StreamingSettingsState {
        config: use_signal(|| None),
        sort_mode: use_signal(|| SortMediaSourcesMode::Best),
        show_decision: use_signal(|| true),
        probe_timeout: use_signal(|| 20),
        probe_timeout_p2p: use_signal(|| 60),
        auto_next_stream: use_signal(|| true),
        max_fallback_streams: use_signal(|| 3),
        p2p_enabled: use_signal(|| true),
        p2p_upload_speed: use_signal(|| 0),
        p2p_download_speed: use_signal(|| 0),
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
                        .sort_mode
                        .set(
                            cfg.sort_media_sources
                                .unwrap_or_default(),
                        );
                    load_state
                        .show_decision
                        .set(
                            cfg.show_playback_decision_in_title
                                .unwrap_or(true),
                        );
                    load_state
                        .probe_timeout
                        .set(
                            cfg.probe_timeout_secs
                                .unwrap_or(20),
                        );
                    load_state
                        .probe_timeout_p2p
                        .set(
                            cfg.probe_timeout_p2p_secs
                                .unwrap_or(60),
                        );
                    load_state
                        .auto_next_stream
                        .set(
                            cfg.auto_next_stream_on_probe_fail
                                .unwrap_or(true),
                        );
                    load_state
                        .max_fallback_streams
                        .set(
                            cfg.max_probe_fallback_streams
                                .unwrap_or(3),
                        );
                    load_state
                        .p2p_enabled
                        .set(
                            cfg.p2p_enabled
                                .unwrap_or(true),
                        );
                    load_state
                        .p2p_upload_speed
                        .set(
                            cfg.p2p_upload_speed_kbps
                                .unwrap_or(0),
                        );
                    load_state
                        .p2p_download_speed
                        .set(
                            cfg.p2p_download_speed_kbps
                                .unwrap_or(0),
                        );
                    load_state
                        .config
                        .set(Some(cfg));
                }
                Err(error) => load_state
                    .error
                    .set(Some(format!("Failed to load streaming settings: {error}"))),
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
            sort_media_sources: Some(
                *state
                    .sort_mode
                    .peek(),
            ),
            show_playback_decision_in_title: Some(
                *state
                    .show_decision
                    .peek(),
            ),
            probe_timeout_secs: Some(
                *state
                    .probe_timeout
                    .peek(),
            ),
            probe_timeout_p2p_secs: Some(
                *state
                    .probe_timeout_p2p
                    .peek(),
            ),
            auto_next_stream_on_probe_fail: Some(
                *state
                    .auto_next_stream
                    .peek(),
            ),
            max_probe_fallback_streams: Some(
                *state
                    .max_fallback_streams
                    .peek(),
            ),
            p2p_enabled: Some(
                *state
                    .p2p_enabled
                    .peek(),
            ),
            p2p_upload_speed_kbps: Some(
                *state
                    .p2p_upload_speed
                    .peek(),
            ),
            p2p_download_speed_kbps: Some(
                *state
                    .p2p_download_speed
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
                StreamingSortPanel {}
                StreamingProbePanel {}
                StreamingP2pPanel {}

                if let Some(error) = state.error.read().as_ref() {
                    ErrorAlert { message: error.clone() }
                }
                if *state.saved.read() {
                    SuccessAlert { message: "Streaming settings saved.".to_string() }
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

#[component]
fn StreamingSortPanel() -> Element {
    let mut state = use_context::<StreamingSettingsState>();
    let description = match *state.sort_mode.read() {
        SortMediaSourcesMode::Disabled => {
            "MediaSources stay in probe/addon order — no capability-based sorting."
        }
        SortMediaSourcesMode::Best => {
            "Direct Play and Direct Stream count equally, so quality picks the winner between them. A version that needs a re-encode still ranks below both. Recommended for most setups."
        }
        SortMediaSourcesMode::Compatibility => {
            "Never prefer a version that needs any transcode over a direct play, and never prefer a remux over a true direct play."
        }
        SortMediaSourcesMode::Quality => {
            "Best quality (resolution, HDR, bit depth, audio) always wins, even if it means transcoding."
        }
    };

    rsx! {
        Card { title: "Stream selection",
            div { style: "display:flex;flex-direction:column;gap:14px",
                div { class: "field",
                    label { class: "field-label", r#for: "sort-media-sources", "Sort streams by device capability" }
                    div { class: "field-hint", "{description}" }
                    select {
                        id: "sort-media-sources",
                        class: "select-input",
                        disabled: *state.saving.read(),
                        value: "{state.sort_mode.read().to_string()}",
                        onchange: move |event: Event<FormData>| {
                            if let Ok(mode) = event.value().parse::<SortMediaSourcesMode>() {
                                state.sort_mode.set(mode);
                                state.saved.set(false);
                            }
                        },
                        option { value: "Disabled", selected: *state.sort_mode.read() == SortMediaSourcesMode::Disabled, "Disabled" }
                        option { value: "Best", selected: *state.sort_mode.read() == SortMediaSourcesMode::Best, "Best" }
                        option { value: "Compatibility", selected: *state.sort_mode.read() == SortMediaSourcesMode::Compatibility, "Compatibility" }
                        option { value: "Quality", selected: *state.sort_mode.read() == SortMediaSourcesMode::Quality, "Quality" }
                    }
                }
                ToggleRow {
                    label: "Show playback decision in title",
                    description: "Append the playback decision to each stream title.",
                    checked: *state.show_decision.read(),
                    disabled: *state.saving.read(),
                    on_change: move |value| {
                        state.show_decision.set(value);
                        state.saved.set(false);
                    }
                }
            }
        }
    }
}

#[component]
fn StreamingProbePanel() -> Element {
    let mut state = use_context::<StreamingSettingsState>();
    rsx! {
        Card { title: "Stream probing",
            div { style: "display:flex;flex-direction:column;gap:14px",
                div { class: "field",
                    label { class: "field-label", r#for: "probe-timeout", "Probe timeout (seconds)" }
                    div { class: "field-hint", "Seconds to wait for HTTP/local stream probes." }
                    input {
                        id: "probe-timeout",
                        r#type: "number",
                        class: "text-input",
                        min: "1",
                        max: "300",
                        value: "{state.probe_timeout}",
                        disabled: *state.saving.read(),
                        oninput: move |event| {
                            if let Ok(value) = event.value().parse::<i64>() {
                                state.probe_timeout.set(value);
                                state.saved.set(false);
                            }
                        },
                    }
                }
                div { class: "field",
                    label { class: "field-label", r#for: "probe-timeout-p2p", "P2P probe timeout (seconds)" }
                    div { class: "field-hint", "Seconds to wait for torrent/P2P stream probes." }
                    input {
                        id: "probe-timeout-p2p",
                        r#type: "number",
                        class: "text-input",
                        min: "1",
                        max: "600",
                        value: "{state.probe_timeout_p2p}",
                        disabled: *state.saving.read(),
                        oninput: move |event| {
                            if let Ok(value) = event.value().parse::<i64>() {
                                state.probe_timeout_p2p.set(value);
                                state.saved.set(false);
                            }
                        },
                    }
                }
                ToggleRow {
                    label: "Auto next stream on probe failure",
                    description: "Try the next stream with matching resolution and type when probing fails.",
                    checked: *state.auto_next_stream.read(),
                    disabled: *state.saving.read(),
                    on_change: move |value| {
                        state.auto_next_stream.set(value);
                        state.saved.set(false);
                    }
                }
                div { class: "field",
                    label { class: "field-label", r#for: "max-fallback", "Max stream retries" }
                    div { class: "field-hint", "How many alternative streams to try before returning an error." }
                    input {
                        id: "max-fallback",
                        r#type: "number",
                        class: "text-input",
                        min: "0",
                        max: "20",
                        value: "{state.max_fallback_streams}",
                        disabled: *state.saving.read(),
                        oninput: move |event| {
                            if let Ok(value) = event.value().parse::<i64>() {
                                state.max_fallback_streams.set(value);
                                state.saved.set(false);
                            }
                        },
                    }
                }
            }
        }
    }
}

#[component]
fn StreamingP2pPanel() -> Element {
    let mut state = use_context::<StreamingSettingsState>();
    rsx! {
        Card { title: "P2P / torrent streams",
            div { style: "display:flex;flex-direction:column;gap:14px",
                ToggleRow {
                    label: "Enable P2P streams",
                    description: "Allow torrent/magnet streams from AIO sources.",
                    checked: *state.p2p_enabled.read(),
                    disabled: *state.saving.read(),
                    on_change: move |value| {
                        state.p2p_enabled.set(value);
                        state.saved.set(false);
                    }
                }
                if *state.p2p_enabled.read() {
                    div { class: "field",
                        label { class: "field-label", r#for: "p2p-up", "Upload speed limit (KB/s)" }
                        input {
                            id: "p2p-up",
                            r#type: "number",
                            class: "field-input",
                            min: "0",
                            value: "{state.p2p_upload_speed}",
                            disabled: *state.saving.read(),
                            oninput: move |event| {
                                if let Ok(value) = event.value().parse::<i64>() {
                                    state.p2p_upload_speed.set(value);
                                    state.saved.set(false);
                                }
                            },
                        }
                        p { class: "field-hint", "0 = no uploading (seeding disabled)." }
                    }
                    div { class: "field",
                        label { class: "field-label", r#for: "p2p-down", "Download speed limit (KB/s)" }
                        input {
                            id: "p2p-down",
                            r#type: "number",
                            class: "field-input",
                            min: "0",
                            value: "{state.p2p_download_speed}",
                            disabled: *state.saving.read(),
                            oninput: move |event| {
                                if let Ok(value) = event.value().parse::<i64>() {
                                    state.p2p_download_speed.set(value);
                                    state.saved.set(false);
                                }
                            },
                        }
                        p { class: "field-hint", "0 = unlimited." }
                    }
                }
            }
        }
    }
}
