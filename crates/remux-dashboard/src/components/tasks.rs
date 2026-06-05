use crate::{
    components::{Button, ButtonVariant, ErrorAlert, FormActions, LoadingText},
    state::AppState,
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    GetScheduledTasks, StartTask, StopTask, TaskInfo, TaskTriggerInfo,
    TaskTriggerInfoType, UpdateTaskTriggers,
};

fn trigger_label(t: &TaskTriggerInfo) -> String {
    let kind = t
        .r#type
        .as_deref()
        .and_then(|s| {
            s.parse::<TaskTriggerInfoType>()
                .ok()
        });
    match kind {
        Some(TaskTriggerInfoType::StartupTrigger) => "On server startup".into(),
        Some(TaskTriggerInfoType::DailyTrigger) => {
            let ticks = t
                .time_of_day_ticks
                .unwrap_or(0);
            let total_secs = ticks / 10_000_000;
            let hour = total_secs / 3600;
            let min = (total_secs % 3600) / 60;
            format!("Daily at {:02}:{:02}", hour, min)
        }
        Some(TaskTriggerInfoType::WeeklyTrigger) => {
            let ticks = t
                .time_of_day_ticks
                .unwrap_or(0);
            let total_secs = ticks / 10_000_000;
            let hour = total_secs / 3600;
            let min = (total_secs % 3600) / 60;
            let day = t
                .day_of_week
                .as_deref()
                .unwrap_or("Sunday");
            format!("Weekly on {} at {:02}:{:02}", day, hour, min)
        }
        Some(TaskTriggerInfoType::IntervalTrigger) => {
            let ticks = t
                .interval_ticks
                .unwrap_or(0);
            if ticks % 36_000_000_000 == 0 {
                format!("Every {} hour(s)", ticks / 36_000_000_000)
            } else {
                format!("Every {} minute(s)", ticks / 600_000_000)
            }
        }
        None => "Unknown".into(),
    }
}

#[component]
pub fn TaskTriggersModal(
    task: TaskInfo,
    app_state: AppState,
    on_done: EventHandler,
    on_cancel: EventHandler,
) -> Element {
    let mut triggers = use_signal(|| {
        task.triggers
            .clone()
            .unwrap_or_default()
    });
    let mut new_type = use_signal(|| TaskTriggerInfoType::DailyTrigger);
    let mut new_hour = use_signal(|| "0".to_string());
    let mut new_min = use_signal(|| "0".to_string());
    let mut new_day = use_signal(|| "Sunday".to_string());
    let mut new_interval_value = use_signal(|| "24".to_string());
    let mut new_interval_unit = use_signal(|| "hours".to_string());
    let mut saving = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let task_id = task
        .id
        .clone();
    let task_name = task
        .name
        .clone();

    rsx! {
        h2 { class: "modal-title", "Triggers — {task_name}" }
        if let Some(desc) = task.description.as_deref().filter(|d| !d.is_empty()) {
            p { class: "text-muted", style: "margin-top: 0.25rem; margin-bottom: 1rem;", "{desc}" }
        }
        for (i, trigger) in triggers.read().clone().into_iter().enumerate() {
            div {
                class: "field",
                style: "display: flex; align-items: center; justify-content: space-between;",
                span { "{trigger_label(&trigger)}" }
                Button {
                    variant: ButtonVariant::Danger,
                    onclick: move |_| { triggers.write().remove(i); },
                    "×"
                }
            }
        }
        if triggers.read().is_empty() {
            p { class: "text-muted", "No triggers" }
        }

        hr {}

        h3 { "Add trigger" }
        div { class: "field",
            label { class: "field-label", "Type" }
            select {
                class: "select-input",
                value: "{new_type.read()}",
                onchange: move |evt| new_type.set(evt.value().parse().unwrap_or(TaskTriggerInfoType::DailyTrigger)),
                option { value: "{TaskTriggerInfoType::DailyTrigger}", "Daily" }
                option { value: "{TaskTriggerInfoType::WeeklyTrigger}", "Weekly" }
                option { value: "{TaskTriggerInfoType::IntervalTrigger}", "Interval" }
                option { value: "{TaskTriggerInfoType::StartupTrigger}", "On server startup" }
            }
        }
        if *new_type.read() == TaskTriggerInfoType::WeeklyTrigger {
            div { style: "display: flex; gap: 1rem;",
                div { class: "field",
                    label { class: "field-label", "Day" }
                    select {
                        class: "select-input",
                        value: "{new_day.read()}",
                        onchange: move |evt| new_day.set(evt.value()),
                        option { value: "Sunday", "Sunday" }
                        option { value: "Monday", "Monday" }
                        option { value: "Tuesday", "Tuesday" }
                        option { value: "Wednesday", "Wednesday" }
                        option { value: "Thursday", "Thursday" }
                        option { value: "Friday", "Friday" }
                        option { value: "Saturday", "Saturday" }
                    }
                }
                div { class: "field",
                    label { class: "field-label", "Hour (0–23)" }
                    input {
                        class: "field-input",
                        r#type: "number",
                        min: "0",
                        max: "23",
                        value: "{new_hour.read()}",
                        oninput: move |evt| new_hour.set(evt.value()),
                    }
                }
                div { class: "field",
                    label { class: "field-label", "Minute (0–59)" }
                    input {
                        class: "field-input",
                        r#type: "number",
                        min: "0",
                        max: "59",
                        value: "{new_min.read()}",
                        oninput: move |evt| new_min.set(evt.value()),
                    }
                }
            }
        } else if *new_type.read() == TaskTriggerInfoType::DailyTrigger {
            div { style: "display: flex; gap: 1rem;",
                div { class: "field",
                    label { class: "field-label", "Hour (0–23)" }
                    input {
                        class: "field-input",
                        r#type: "number",
                        min: "0",
                        max: "23",
                        value: "{new_hour.read()}",
                        oninput: move |evt| new_hour.set(evt.value()),
                    }
                }
                div { class: "field",
                    label { class: "field-label", "Minute (0–59)" }
                    input {
                        class: "field-input",
                        r#type: "number",
                        min: "0",
                        max: "59",
                        value: "{new_min.read()}",
                        oninput: move |evt| new_min.set(evt.value()),
                    }
                }
            }
        } else if *new_type.read() == TaskTriggerInfoType::IntervalTrigger {
            div { style: "display: flex; gap: 1rem; align-items: flex-end;",
                div { class: "field",
                    label { class: "field-label", "Every" }
                    input {
                        class: "field-input",
                        r#type: "number",
                        min: "1",
                        value: "{new_interval_value.read()}",
                        oninput: move |evt| new_interval_value.set(evt.value()),
                    }
                }
                div { class: "field",
                    label { class: "field-label", "Unit" }
                    select {
                        class: "select-input",
                        onchange: move |evt| new_interval_unit.set(evt.value()),
                        option { value: "minutes", selected: *new_interval_unit.read() == "minutes", "Minutes" }
                        option { value: "hours", selected: *new_interval_unit.read() == "hours", "Hours" }
                    }
                }
            }
        }
        Button {
            variant: ButtonVariant::Secondary,
            onclick: move |_| {
                let kind = *new_type.read();
                let trigger = match kind {
                    TaskTriggerInfoType::StartupTrigger => TaskTriggerInfo {
                        r#type: Some(kind.to_string()),
                        ..Default::default()
                    },
                    TaskTriggerInfoType::IntervalTrigger => {
                        let val: i64 = new_interval_value.read().parse().unwrap_or(24).max(1);
                        let ticks_per_unit: i64 = if *new_interval_unit.read() == "minutes" {
                            600_000_000
                        } else {
                            36_000_000_000
                        };
                        TaskTriggerInfo {
                            r#type: Some(kind.to_string()),
                            interval_ticks: Some(val * ticks_per_unit),
                            ..Default::default()
                        }
                    }
                    TaskTriggerInfoType::DailyTrigger | TaskTriggerInfoType::WeeklyTrigger => {
                        let h: i64 = new_hour.read().parse().unwrap_or(0).clamp(0, 23);
                        let m: i64 = new_min.read().parse().unwrap_or(0).clamp(0, 59);
                        TaskTriggerInfo {
                            r#type: Some(kind.to_string()),
                            time_of_day_ticks: Some(h * 36_000_000_000 + m * 600_000_000),
                            day_of_week: (kind == TaskTriggerInfoType::WeeklyTrigger)
                                .then(|| new_day.read().clone()),
                            ..Default::default()
                        }
                    }
                };
                triggers.write().push(trigger);
            },
            "Add"
        }

        if let Some(e) = error.read().as_ref() {
            ErrorAlert { message: e.clone() }
        }

        FormActions {
            Button {
                variant: ButtonVariant::Ghost,
                onclick: move |_| on_cancel.call(()),
                "Cancel"
            }
            Button {
                variant: ButtonVariant::Primary,
                disabled: *saving.read(),
                onclick: move |_| {
                    let client = app_state.client.clone();
                    let tid = task_id.clone();
                    let t = triggers.read().clone();
                    saving.set(true);
                    error.set(None);
                    spawn(async move {
                        match client.execute(UpdateTaskTriggers { task_id: tid, triggers: t }).await {
                            Ok(_) => on_done.call(()),
                            Err(e) => {
                                saving.set(false);
                                error.set(Some(e.user_message()));
                            }
                        }
                    });
                },
                if *saving.read() { "Saving…" } else { "Save" }
            }
        }
    }
}

#[component]
pub fn TasksCard(
    app_state: AppState,
    #[props(default = false)] running_only: bool,
) -> Element {
    let mut tasks: Signal<Vec<TaskInfo>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut refresh: Signal<u32> = use_signal(|| 0);
    let mut selected_task: Signal<Option<TaskInfo>> = use_signal(|| None);

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        loading.set(true);
        let client = app_state_effect
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetScheduledTasks {
                    is_hidden: Some(false),
                })
                .await
            {
                Ok(t) => {
                    tasks.set(t);
                    error.set(None);
                }
                Err(e) => error.set(Some(format!("Failed to fetch tasks: {e}"))),
            }
            loading.set(false);
        });
    });

    let app_state_poll = app_state.clone();
    use_effect(move || {
        let client = app_state_poll
            .client
            .clone();
        spawn(async move {
            loop {
                gloo_timers::future::sleep(std::time::Duration::from_secs(5)).await;
                if let Ok(t) = client
                    .execute(GetScheduledTasks {
                        is_hidden: Some(false),
                    })
                    .await
                {
                    tasks.set(t);
                }
            }
        });
    });

    rsx! {
        div { class: "card",
            div { class: "card-header",
                span { class: "card-title", if running_only { "Running Tasks" } else { "Scheduled Tasks" } }
            }
            div { class: "card-body tight",
                if *loading.read() && tasks.read().is_empty() {
                    LoadingText {}
                } else if let Some(err) = error.read().as_ref() {
                    span { class: "loading-text", style: "color:var(--error)", "{err}" }
                } else {
                    {
                        let visible: Vec<_> = tasks.read().iter()
                            .filter(|t| !running_only || t.state.as_deref() == Some("Running"))
                            .cloned()
                            .collect();
                        if visible.is_empty() {
                            rsx! {
                                div { class: "empty-state",
                                    if running_only { "No tasks currently running" } else { "No tasks found" }
                                }
                            }
                        } else if running_only {
                            rsx! {
                                div { class: "data-table-container",
                                    div { class: "row-list",
                                        for task in visible {
                                            TaskRow { key: "{task.id}", task }
                                        }
                                    }
                                }
                            }
                        } else {
                            let mut groups: std::collections::BTreeMap<String, Vec<TaskInfo>> =
                                std::collections::BTreeMap::new();
                            for task in visible {
                                let cat = task.category.clone().unwrap_or_else(|| "Other".to_string());
                                groups.entry(cat).or_default().push(task);
                            }
                            let groups: Vec<(String, Vec<TaskInfo>)> = groups
                                .into_iter()
                                .map(|(cat, mut tasks)| {
                                    tasks.sort_by(|a, b| a.name.cmp(&b.name));
                                    (cat, tasks)
                                })
                                .collect();
                            rsx! {
                                for (cat, group_tasks) in groups {
                                    div { key: "{cat}", class: "task-group",
                                        div { class: "task-group-header", "{cat}" }
                                        div { class: "row-list",
                                            for task in group_tasks {
                                                TaskPageRow {
                                                    key: "{task.id}",
                                                    task: task.clone(),
                                                    show_category: false,
                                                    app_state: app_state.clone(),
                                                    on_refresh: move |_| {
                                                        let v = *refresh.peek() + 1;
                                                        refresh.set(v);
                                                    },
                                                    on_edit: move |t: TaskInfo| selected_task.set(Some(t)),
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(task) = selected_task.read().clone() {
            div { class: "modal-backdrop",
                div { class: "modal",
                    TaskTriggersModal {
                        task,
                        app_state: app_state.clone(),
                        on_done: move |_| {
                            selected_task.set(None);
                            let v = *refresh.peek() + 1;
                            refresh.set(v);
                        },
                        on_cancel: move |_| selected_task.set(None),
                    }
                }
            }
        }
    }
}

#[component]
pub fn TaskPageRow(
    task: TaskInfo,
    app_state: AppState,
    on_refresh: EventHandler,
    on_edit: EventHandler<TaskInfo>,
    #[props(default = true)] show_category: bool,
) -> Element {
    let start_id = task
        .id
        .clone();
    let stop_id = task
        .id
        .clone();
    let c_start = app_state
        .client
        .clone();
    let c_stop = app_state
        .client
        .clone();
    let task_for_edit = task.clone();

    rsx! {
        TaskRow {
            task,
            show_category,
            on_click: move |_| on_edit.call(task_for_edit.clone()),
            on_start: move |_| {
                let id = start_id.clone();
                let c = c_start.clone();
                spawn(async move {
                    let _ = c.execute(StartTask { task_id: id }).await;
                    on_refresh.call(());
                });
            },
            on_stop: move |_| {
                let id = stop_id.clone();
                let c = c_stop.clone();
                spawn(async move {
                    let _ = c.execute(StopTask { task_id: id }).await;
                    on_refresh.call(());
                });
            },
        }
    }
}

#[component]
pub fn TaskRow(
    task: TaskInfo,
    #[props(default = true)] show_category: bool,
    #[props(optional)] on_click: Option<EventHandler>,
    #[props(optional)] on_start: Option<EventHandler>,
    #[props(optional)] on_stop: Option<EventHandler>,
) -> Element {
    let state = task
        .state
        .as_deref()
        .unwrap_or("Idle");
    let is_running = state == "Running";

    let last_status = task
        .last_execution_result
        .as_ref()
        .and_then(|r| {
            r.status
                .as_deref()
        })
        .unwrap_or("");

    let display_state = if is_running { state } else { last_status };
    let display_badge = if is_running {
        "task-badge task-badge-running"
    } else {
        match last_status {
            "Completed" => "task-badge task-badge-completed",
            "Failed" => "task-badge task-badge-failed",
            _ => "task-badge task-badge-idle",
        }
    };

    let has_controls = on_start.is_some() || on_stop.is_some();
    let clickable = on_click.is_some();

    rsx! {
        div {
            class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)] even:bg-[rgba(0,0,0,0.02)] even:hover:bg-[rgba(0,0,0,0.03)]",
            style: if clickable { "cursor: pointer;" } else { "" },
            onclick: move |_| { if let Some(ref h) = on_click { h.call(()); } },
            div { class: "flex-1 min-w-0 px-3 py-[10px]",
                div { class: "task-name", "{task.name}" }
                if let Some(sd) = task.short_description.as_deref().filter(|s| !s.is_empty()) {
                    div { class: "task-short-desc", "{sd}" }
                }
                if show_category {
                    if let Some(cat) = &task.category {
                        div { class: "task-category", "{cat}" }
                    }
                }
                if is_running {
                    if let Some(pct) = task.current_progress_percentage {
                        div { class: "task-progress-bar",
                            div {
                                class: "task-progress-fill",
                                style: "width:{pct:.0}%",
                            }
                        }
                    }
                }
            }
            div { class: "shrink-0 px-3 py-[10px]",
                if !display_state.is_empty() {
                    span { class: "{display_badge}", "{display_state}" }
                }
            }
            div { class: "shrink-0 px-3 py-[10px] flex items-center gap-2",
                if has_controls {
                    div { class: "task-actions",
                        if !is_running {
                            button {
                                class: "btn btn-ghost task-btn",
                                title: "Run now",
                                onclick: move |evt: Event<MouseData>| {
                                    evt.stop_propagation();
                                    if let Some(ref h) = on_start { h.call(()); }
                                },
                                "▶"
                            }
                        }
                        if is_running {
                            button {
                                class: "btn btn-ghost task-btn",
                                title: "Stop",
                                onclick: move |evt: Event<MouseData>| {
                                    evt.stop_propagation();
                                    if let Some(ref h) = on_stop { h.call(()); }
                                },
                                "■"
                            }
                        }
                    }
                }
            }
        }
    }
}
