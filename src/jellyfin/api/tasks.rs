use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use remux_macros::{get, post, delete};
use uuid::Uuid;

use crate::AppState;
use crate::jellyfin;
use crate::tasks::{TaskStatus, TaskHandlerSnapshot};
use axum_anyhow::ApiResult as Result;

#[cfg(test)]
use crate::integration_test;

/// Get scheduled tasks
#[get("/scheduledtasks")]
pub async fn scheduled_tasks(
    State(state): State<AppState>,
) -> Result<impl axum::response::IntoResponse> {
    // Get all registered tasks from the task service
    let task_handlers = state.tasks.get_task_handlers().await;
    
    // Convert task handlers to Jellyfin TaskInfo format
    let mut task_infos: Vec<jellyfin::TaskInfo> = Vec::new();
    
    for (_, handler) in task_handlers.iter() {
        let task = handler.task.clone();
        
        // Get the last execution result from database using task key
        let task_key = task.key();
        let last_result = jellyfin::db::TaskResult::get_by_task_id(&state.ctx.db, task_key).await.ok().flatten();
        
        // Convert task status to Jellyfin format
        let state_str = match &handler.status {
            TaskStatus::Idle => "Idle",
            TaskStatus::Active => "Running",
            TaskStatus::Stopped => "Stopped",
            TaskStatus::Failed(_) => "Failed",
        };
        
        // Convert last result to Jellyfin format
        let (last_execution_result, last_execution_date) = match last_result {
            Some(db_result) => {
                let execution_result = jellyfin::TaskResult {
                    status: Some(match db_result.status {
                        jellyfin::db::TaskResultStatus::Completed => "Completed",
                        jellyfin::db::TaskResultStatus::Failed => "Failed",
                        jellyfin::db::TaskResultStatus::Stopped => "Stopped",
                    }.to_string()),
                    name: Some(task.name().to_string()),
                    id: Some(task.key().to_string()),
                    key: Some(task.key().to_string()),
                    start_time_utc: Some(db_result.start_at.to_string()),
                    end_time_utc: Some(db_result.end_at.to_string()),
                    ..Default::default()
                };
                (Some(execution_result), Some(db_result.end_at.to_string()))
            },
            None => (None, None),
        };
        
        task_infos.push(jellyfin::TaskInfo {
            name: task.name().to_string(),
            state: Some(state_str.to_string()),
            current_progress_percentage: Some(handler.current_progress),  // Include progress from handler
            id: task.key().to_string(),
            last_execution_result,
            triggers: Some(Vec::new()), // Empty triggers for now
            description: Some(task.name().to_string()),
            category: Some(task.category().to_string()),
            is_hidden: Some(false),
            is_enabled: Some(true), // Tasks are enabled by default
            key: Some(task.key().to_string()),
            last_execution_date: last_execution_date,
            can_be_terminated: Some(true),
            can_be_deleted: Some(false),
        });
    }
    
    // Return direct array like Jellyfin does, not wrapped in QueryResult
    Ok(Json(task_infos))
}

/// Get task by ID
#[get("/scheduledtasks/{task_id}")]
pub async fn get_task_by_id(
    Path(task_id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl axum::response::IntoResponse> {
    let task_handlers = state.tasks.get_task_handlers().await;
    
    // Find the task by key
    let task_handler = task_handlers.get(&task_id);
    
    if task_handler.is_none() {
        return Err(anyhow::anyhow!("Task not found").into());
    }
    
    let handler = task_handler.unwrap();
    let task = handler.task.clone();
    
    // Get the last execution result from database
    let last_result = jellyfin::db::TaskResult::get_by_task_id(&state.ctx.db, &task_id).await.ok().flatten();
    
    // Convert task status to Jellyfin format
    let state_str = match &handler.status {
        TaskStatus::Idle => "Idle",
        TaskStatus::Active => "Running",
        TaskStatus::Stopped => "Stopped",
        TaskStatus::Failed(_) => "Failed",
    };
    
    // Convert last result to Jellyfin format
    let (last_execution_result, last_execution_date) = match last_result {
        Some(db_result) => {
            let execution_result = jellyfin::TaskResult {
                status: Some(match db_result.status {
                    jellyfin::db::TaskResultStatus::Completed => "Completed",
                    jellyfin::db::TaskResultStatus::Failed => "Failed",
                    jellyfin::db::TaskResultStatus::Stopped => "Stopped",
                }.to_string()),
                name: Some(task.name().to_string()),
                id: Some(task.key().to_string()),
                key: Some(task.key().to_string()),
                start_time_utc: Some(db_result.start_at.to_string()),
                end_time_utc: Some(db_result.end_at.to_string()),
                ..Default::default()
            };
            (Some(execution_result), Some(db_result.end_at.to_string()))
        },
        None => (None, None),
    };
    
    let task_info = jellyfin::TaskInfo {
        name: task.name().to_string(),
        state: Some(state_str.to_string()),
        current_progress_percentage: None,
        id: task.key().to_string(),
        last_execution_result,
        triggers: Some(Vec::new()), // TODO: Implement trigger retrieval
        description: Some(task.name().to_string()),
        category: Some(task.category().to_string()),
        is_hidden: Some(false),
        is_enabled: Some(true), // Tasks are enabled by default
        key: Some(task.key().to_string()),
        last_execution_date: last_execution_date,
        can_be_terminated: Some(true),
        can_be_deleted: Some(false),
    };
    
    Ok(Json(task_info))
}

/// Start specified task
#[post("/scheduledtasks/running/{task_id}")]
pub async fn start_task(
    Path(task_id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl axum::response::IntoResponse> {
    state.tasks.run_task(&task_id).await?;
    
    Ok(StatusCode::NO_CONTENT)
}

/// Stop specified task
#[delete("/scheduledtasks/running/{task_id}")]
pub async fn stop_task(
    Path(task_id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl axum::response::IntoResponse> {
    state.tasks.stop_task(&task_id).await?;
    
    Ok(StatusCode::NO_CONTENT)
}

/// Update specified task triggers
#[post("/scheduledtasks/{task_id}/triggers")]
pub async fn update_task_triggers(
    Path(task_id): Path<String>,
    State(state): State<AppState>,
    Json(trigger_infos): Json<Vec<jellyfin::TaskTriggerInfo>>,
) -> Result<impl axum::response::IntoResponse> {
    // Convert Jellyfin triggers to our internal format
    let triggers: Vec<jellyfin::db::TaskTrigger> = trigger_infos
        .into_iter()
        .map(|trigger_info| {
            jellyfin::db::TaskTrigger {
                id: Uuid::new_v4().to_string(),
                task_id: task_id.clone(),
                kind: match trigger_info.r#type.as_deref() {
                    Some("Daily") => jellyfin::db::TaskTriggerKind::Schedule,
                    Some("Startup") => jellyfin::db::TaskTriggerKind::Startup,
                    _ => jellyfin::db::TaskTriggerKind::Schedule,
                },
                time_limit_hours: None, // TODO: Implement time limit
                cron: None, // TODO: Implement cron parsing
            }
        })
        .collect();
    
    // Update triggers for the task
    state.tasks.replace_triggers(&task_id, triggers).await?;
    
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
#[tokio::test]
async fn scheduled_tasks_test() {
    let server = crate::integration_test::new_test_server().await.unwrap();

    let response = server.get("/ScheduledTasks").await;

    response.assert_status_ok();
    let task_result: crate::jellyfin::TaskQueryResult = response.json();
    
    // Check that we have at least some tasks (should have MediaScanTask and CatalogImportTask)
    assert!(task_result.items.len() >= 2);
    
    // Check that we have the expected structure
    let task_names: Vec<String> = task_result.items.iter().map(|task| task.name.clone()).collect();
    assert!(task_names.contains(&"Media Scan".to_string()) || task_names.contains(&"Catalog Import".to_string()));
    
    // Check that all tasks have required fields
    for task in &task_result.items {
        assert!(task.id.len() > 0);
        assert!(task.name.len() > 0);
        assert!(task.state.is_some());
        assert!(task.category.is_some());
    }
}