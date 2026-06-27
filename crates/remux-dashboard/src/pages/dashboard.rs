use crate::{components::*, state::AppState};
use dioxus::prelude::*;

#[component]
pub fn DashboardPage(app_state: AppState) -> Element {
    rsx! {
        ServerInfoCard { app_state: app_state.clone() }
        MediaStatsCard { app_state: app_state.clone() }
        MetricsCard { app_state: app_state.clone() }
        SessionsCard { app_state: app_state.clone() }
        TasksCard { app_state: app_state.clone(), running_only: true }
    }
}
