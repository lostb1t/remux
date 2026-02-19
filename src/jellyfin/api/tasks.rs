use axum::Json;
use axum::extract::State;
use remux_macros::get;
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
        
        // Get the last execution result from database
        let task_id = task.id();
        let last_result = jellyfin::db::TaskResult::get_by_task_id(&state.ctx.db, task_id).await.ok().flatten();
        
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
                    id: Some(task.id().to_string()),
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
            current_progress_percentage: None,
            id: task.id().to_string(),
            last_execution_result,
            triggers: Some(Vec::new()), // Empty triggers for now
            description: Some(task.name().to_string()),
            category: Some(task.category().to_string()),
            is_hidden: Some(false),
            key: Some(task.id().to_string()),
            last_execution_date: last_execution_date,
            can_be_terminated: Some(true),
            can_be_deleted: Some(false),
        });
    }
    
    let total_count = task_infos.len() as i64;
    let response = jellyfin::TaskQueryResult {
        items: task_infos,
        total_record_count: total_count,
    };
    
    Ok(Json(response))
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