use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use remux_macros::{delete, get, post};
use uuid::Uuid;

use crate::AppState;
use crate::db::auth;
use crate::jellyfin;
use crate::tasks::{TaskStatus, TaskView};
use axum_anyhow::ApiResult as Result;
use shared::sdks::jellyfin::models::TaskTriggerInfoType;

#[cfg(test)]
use crate::integration_test;

// 1 tick = 100 nanoseconds (Windows FILETIME / .NET TimeSpan)
const TICKS_PER_SECOND: i64 = 10_000_000;
const TICKS_PER_HOUR: i64 = 3600 * TICKS_PER_SECOND;
const TICKS_PER_MINUTE: i64 = 60 * TICKS_PER_SECOND;

fn db_trigger_to_jellyfin(
    trigger: &jellyfin::db::TaskTrigger,
) -> jellyfin::TaskTriggerInfo {
    let cron = trigger.cron.as_deref();
    let (time_of_day_ticks, interval_ticks, day_of_week) = match trigger.kind {
        TaskTriggerInfoType::StartupTrigger => (None, None, None),
        TaskTriggerInfoType::IntervalTrigger => {
            let hours = cron.and_then(cron_to_interval_hours);
            (None, hours.map(|h| h * TICKS_PER_HOUR), None)
        }
        TaskTriggerInfoType::WeeklyTrigger => {
            let ticks = cron.and_then(cron_to_time_of_day_ticks);
            let day = cron.and_then(cron_to_day_name).map(str::to_string);
            (ticks, None, day)
        }
        TaskTriggerInfoType::DailyTrigger => {
            (cron.and_then(cron_to_time_of_day_ticks), None, None)
        }
    };

    jellyfin::TaskTriggerInfo {
        r#type: Some(trigger.kind.to_string()),
        time_of_day_ticks,
        interval_ticks,
        day_of_week,
        max_runtime_ticks: trigger.time_limit_hours.map(|h| h * TICKS_PER_HOUR),
    }
}

/// Parse `0 MIN HOUR * * [DAY]` cron into ticks-since-midnight.
/// Field order: SEC MIN HOUR DOM MON DOW
fn cron_to_time_of_day_ticks(cron: &str) -> Option<i64> {
    let mut parts = cron.split_whitespace();
    parts.next(); // sec
    let min: i64 = parts.next()?.parse().ok()?;
    let hour: i64 = parts.next()?.parse().ok()?;
    Some(hour * TICKS_PER_HOUR + min * TICKS_PER_MINUTE)
}

/// Parse `0 0 */HOURS * * *` interval cron into hours.
fn cron_to_interval_hours(cron: &str) -> Option<i64> {
    let mut parts = cron.split_whitespace();
    parts.next(); // sec
    parts.next(); // min
    let hour_part = parts.next()?;
    let hours: i64 = hour_part.strip_prefix("*/")?.parse().ok()?;
    Some(hours)
}

/// Parse `0 MIN HOUR * * DAY_NUM` cron into day name.
/// Croner uses POSIX weekdays: 1=Mon … 6=Sat, 7=Sun.
fn cron_to_day_name(cron: &str) -> Option<&'static str> {
    let day_num: u8 = cron.split_whitespace().nth(5)?.parse().ok()?;
    Some(match day_num {
        1 => "Monday",
        2 => "Tuesday",
        3 => "Wednesday",
        4 => "Thursday",
        5 => "Friday",
        6 => "Saturday",
        _ => "Sunday", // 7 or 0
    })
}

fn task_info(
    handler: &TaskView,
    triggers: Vec<jellyfin::db::TaskTrigger>,
    last_result: Option<jellyfin::db::TaskResult>,
) -> jellyfin::TaskInfo {
    let task = &handler.task;

    let state_str = match &handler.status {
        TaskStatus::Idle => "Idle",
        TaskStatus::Running => "Running",
        TaskStatus::Stopped => "Stopped",
        TaskStatus::Failed(_) => "Failed",
    };

    let (last_execution_result, last_execution_date) = match last_result {
        Some(r) => {
            let result = jellyfin::TaskResult {
                status: Some(
                    match r.status {
                        jellyfin::db::TaskResultStatus::Completed => "Completed",
                        jellyfin::db::TaskResultStatus::Failed => "Failed",
                        jellyfin::db::TaskResultStatus::Stopped => "Stopped",
                    }
                    .to_string(),
                ),
                name: Some(task.name().to_string()),
                id: Some(task.key().to_string()),
                key: Some(task.key().to_string()),
                start_time_utc: Some(r.start_at.to_string()),
                end_time_utc: Some(r.end_at.to_string()),
                ..Default::default()
            };
            (Some(result), Some(r.end_at.to_string()))
        }
        None => (None, None),
    };

    jellyfin::TaskInfo {
        name: task.name().to_string(),
        state: Some(state_str.to_string()),
        current_progress_percentage: Some(handler.progress),
        id: task.key().to_string(),
        key: Some(task.key().to_string()),
        last_execution_result,
        last_execution_date,
        triggers: Some(triggers.iter().map(db_trigger_to_jellyfin).collect()),
        description: Some(task.name().to_string()),
        category: Some(task.category().to_string()),
        is_hidden: Some(false),
        is_enabled: Some(true),
        can_be_terminated: Some(true),
        can_be_deleted: Some(false),
    }
}

/// Get scheduled tasks
#[get("/scheduledtasks")]
pub async fn scheduled_tasks(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<impl axum::response::IntoResponse> {
    let task_handlers = state.tasks.get_task_handlers().await;

    // Fetch all triggers once and group by lowercase task_id to avoid N+1
    let all_triggers = jellyfin::db::TaskTrigger::get_all(&state.ctx.db).await?;
    let mut triggers_by_task: std::collections::HashMap<
        String,
        Vec<jellyfin::db::TaskTrigger>,
    > = std::collections::HashMap::new();
    for trigger in all_triggers {
        triggers_by_task
            .entry(trigger.task_id.to_lowercase())
            .or_default()
            .push(trigger);
    }

    let mut task_infos: Vec<jellyfin::TaskInfo> = Vec::new();

    for (key, handler) in task_handlers.iter() {
        let last_result = jellyfin::db::TaskResult::get_by_task_id(&state.ctx.db, key)
            .await
            .ok()
            .flatten();
        let triggers = triggers_by_task.remove(key).unwrap_or_default();
        task_infos.push(task_info(handler, triggers, last_result));
    }

    Ok(Json(task_infos))
}

/// Get task by ID
#[get("/scheduledtasks/{task_id}")]
pub async fn get_task_by_id(
    Path(task_id): Path<String>,
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<impl axum::response::IntoResponse> {
    let task_handlers = state.tasks.get_task_handlers().await;
    let handler = task_handlers
        .get(&task_id)
        .ok_or_else(|| anyhow::anyhow!("Task not found"))?;

    let last_result = jellyfin::db::TaskResult::get_by_task_id(&state.ctx.db, &task_id)
        .await
        .ok()
        .flatten();
    let triggers =
        jellyfin::db::TaskTrigger::get_by_task_id(&state.ctx.db, &task_id).await?;

    Ok(Json(task_info(handler, triggers, last_result)))
}

#[post("/scheduledtasks/running/{task_id}")]
pub async fn start_task(
    Path(task_id): Path<String>,
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<impl axum::response::IntoResponse> {
    state.tasks.run_task(&task_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[delete("/scheduledtasks/running/{task_id}")]
pub async fn stop_task(
    Path(task_id): Path<String>,
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<impl axum::response::IntoResponse> {
    state.tasks.stop_task(&task_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn day_name_to_cron(day: &str) -> u8 {
    // Croner POSIX weekdays: 1=Mon … 6=Sat, 7=Sun
    match day {
        "Monday"    => 1,
        "Tuesday"   => 2,
        "Wednesday" => 3,
        "Thursday"  => 4,
        "Friday"    => 5,
        "Saturday"  => 6,
        _           => 7, // Sunday
    }
}

fn trigger_to_cron(t: &jellyfin::TaskTriggerInfo) -> Option<String> {
    let kind = t.r#type.as_deref()
        .and_then(|s| s.parse::<TaskTriggerInfoType>().ok())
        .unwrap_or(TaskTriggerInfoType::DailyTrigger);

    match kind {
        TaskTriggerInfoType::StartupTrigger => None,
        TaskTriggerInfoType::IntervalTrigger => {
            let hours = t.interval_ticks? / TICKS_PER_HOUR;
            Some(format!("0 0 */{hours} * * *"))
        }
        TaskTriggerInfoType::DailyTrigger | TaskTriggerInfoType::WeeklyTrigger => {
            let ticks = t.time_of_day_ticks?;
            let total_secs = ticks / TICKS_PER_SECOND;
            let hour = total_secs / 3600;
            let min = (total_secs % 3600) / 60;
            if kind == TaskTriggerInfoType::WeeklyTrigger {
                let day = day_name_to_cron(t.day_of_week.as_deref().unwrap_or("Sunday"));
                Some(format!("0 {min} {hour} * * {day}"))
            } else {
                Some(format!("0 {min} {hour} * * *"))
            }
        }
    }
}

#[post("/scheduledtasks/{task_id}/triggers")]
pub async fn update_task_triggers(
    Path(task_id): Path<String>,
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Json(trigger_infos): Json<Vec<jellyfin::TaskTriggerInfo>>,
) -> Result<impl axum::response::IntoResponse> {
    let triggers: Vec<jellyfin::db::TaskTrigger> = trigger_infos
        .into_iter()
        .map(|t| jellyfin::db::TaskTrigger {
            id: Uuid::new_v4().to_string(),
            task_id: task_id.clone(),
            kind: t.r#type
                .as_deref()
                .and_then(|s| s.parse::<TaskTriggerInfoType>().ok())
                .unwrap_or(TaskTriggerInfoType::DailyTrigger),
            time_limit_hours: t.max_runtime_ticks.map(|ticks| ticks / TICKS_PER_HOUR),
            cron: trigger_to_cron(&t),
        })
        .collect();

    state.tasks.replace_triggers(&task_id, triggers).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
#[tokio::test]
async fn scheduled_tasks_test() {
    use crate::integration_test::{auth_header_with_token, authenticated_server};
    use http::header::HeaderValue;
    let (server, _ctx, token) = authenticated_server().await;
    let auth = auth_header_with_token(&token);

    let response = server
        .get("/scheduledtasks")
        .add_header(
            http::header::AUTHORIZATION,
            HeaderValue::from_str(&auth).unwrap(),
        )
        .await;

    response.assert_status_ok();
    let tasks: Vec<crate::jellyfin::TaskInfo> = response.json();

    assert!(tasks.len() >= 2);

    let task_names: Vec<String> = tasks.iter().map(|task| task.name.clone()).collect();
    assert!(
        task_names.contains(&"Media Scan".to_string())
            || task_names.contains(&"Catalog Import".to_string())
    );

    for task in &tasks {
        assert!(task.id.len() > 0);
        assert!(task.name.len() > 0);
        assert!(task.state.is_some());
        assert!(task.category.is_some());
    }
}
