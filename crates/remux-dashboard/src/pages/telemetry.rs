use crate::{
    components::{Card, LoadingText},
    state::AppState,
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    GetTelemetryOverview, GetTelemetryRankings, TelemetryOverview, TelemetryRankingRow,
};

#[component]
pub fn TelemetryPage(app_state: AppState) -> Element {
    let mut data = use_signal(|| None::<TelemetryOverview>);
    let mut rankings = use_signal(Vec::<TelemetryRankingRow>::new);
    let mut dimension = use_signal(|| "route".to_string());
    let mut hours = use_signal(|| 24_i64);
    use_effect(move || {
        let client = app_state
            .client
            .clone();
        spawn(async move {
            if let Ok(value) = client
                .execute(GetTelemetryOverview)
                .await
            {
                data.set(Some(value));
            }
            if let Ok(value) = client
                .execute(GetTelemetryRankings {
                    dimension: dimension(),
                    hours: hours(),
                })
                .await
            {
                rankings.set(value);
            }
        });
    });
    rsx! {
    Card { title: "Performance Telemetry · last 24 hours",
        if let Some(value) = data.read().as_ref() {
            div { class: "kv-row", span { class: "kv-label", "Requests" } span { class: "kv-value", "{value.request_count}" } }
            div { class: "kv-row", span { class: "kv-label", "Errors" } span { class: "kv-value", "{value.error_count}" } }
            div { class: "kv-row", span { class: "kv-label", "Mean latency" } span { class: "kv-value", "{value.mean_latency_ms:.1} ms" } }
            div { class: "kv-row", span { class: "kv-label", "Slowest request" } span { class: "kv-value", "{value.max_latency_ms:.1} ms" } }
            div { class: "kv-row", span { class: "kv-label", "Playback milestones" } span { class: "kv-value", "{value.playback_events}" } }
            div { class: "kv-row", span { class: "kv-label", "Startup failures" } span { class: "kv-value", "{value.startup_failures}" } }
        } else { LoadingText {} }
    }
    Card { title: "Telemetry ranking",
        div { class: "kv-row",
            select { value: "{dimension}", onchange: move |event| dimension.set(event.value()),
                option { value: "route", "Endpoint" }
                option { value: "device", "Device" }
                option { value: "client", "Client" }
                option { value: "user", "User" }
                option { value: "content", "Content" }
            }
            select { value: "{hours}", onchange: move |event| if let Ok(value) = event.value().parse::<i64>() { hours.set(value); },
                option { value: "1", "1 hour" }
                option { value: "24", "24 hours" }
                option { value: "168", "7 days" }
                option { value: "720", "30 days" }
            }
        }
        if rankings.read().is_empty() { LoadingText {} }
        else { for row in rankings.read().iter().take(10) {
            div { class: "kv-row", span { class: "kv-label", "{row.label}" } span { class: "kv-value", "{row.mean_latency_ms:.1} ms avg · {row.count} requests" } }
        } }
    }
    }
}
