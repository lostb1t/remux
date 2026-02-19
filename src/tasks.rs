use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use futures::stream::StreamExt;
use itertools::Itertools;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_cron_scheduler::job::JobId;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info, warn};
use uuid::Uuid;
use uuid::uuid;

// Assuming these are imported from your project:
use crate::{
    AppContext, aio, db,
    meta_provider::{AioMetaProvider, AioTreeSyncProvider, MetaProviderService},
};

#[derive(Debug)]
pub enum TaskStatus {
    Idle,
    Active,
    Stopped,
    Failed(anyhow::Error),
}

impl Clone for TaskStatus {
    fn clone(&self) -> Self {
        match self {
            TaskStatus::Idle => TaskStatus::Idle,
            TaskStatus::Active => TaskStatus::Active,
            TaskStatus::Stopped => TaskStatus::Stopped,
            TaskStatus::Failed(err) => TaskStatus::Failed(anyhow::anyhow!(err.to_string())),
        }
    }
}

#[async_trait]
pub trait Task: Send + Sync {
    fn key(&self) -> &str {
        // Default implementation uses the task name as key
        self.name()
    }
    
    fn name(&self) -> &str;
    fn category(&self) -> &str {
        // Default category
        "System"
    }
    fn default_triggers(&self) -> Vec<db::TaskTrigger>;

    async fn run(
        self: Arc<Self>,
        ctx: AppContext,
        task_service: Arc<TaskService>,
    ) -> Result<()>;
}

pub struct TaskHandler {
    pub status: TaskStatus,
    pub task: Arc<dyn Task>,
    pub jobs: Vec<JobId>,
    pub handle: Option<JoinHandle<()>>,
}
pub struct TaskHandlerSnapshot {
    pub status: TaskStatus,
    pub task: Arc<dyn Task>,
    pub jobs: Vec<JobId>,
}

impl TaskHandler {
    pub fn new(task: Arc<dyn Task>) -> Self {
        Self {
            status: TaskStatus::Idle,
            task,
            jobs: Vec::new(),
            handle: None,
        }
    }

    pub fn to_snapshot(&self) -> TaskHandlerSnapshot {
        TaskHandlerSnapshot {
            status: self.status.clone(),
            task: self.task.clone(),
            jobs: self.jobs.clone(),
        }
    }

    pub fn key(&self) -> &str {
        self.task.key()
    }

    pub fn is_running(&self) -> bool {
        self.handle
            .as_ref()
            .map(|h| !h.is_finished())
            .unwrap_or(false)
    }

    pub fn run(&mut self, ctx: AppContext, task_service: Arc<TaskService>) {
        if self.is_running() {
            return;
        }

        self.status = TaskStatus::Active;
        let task = self.task.clone();
        let task_key = task.key().to_string();

        let handle = tokio::spawn(async move {
            let start_time = Utc::now().naive_utc();
            let instant_start = Instant::now();
            let task_name = task.name().to_string();
            info!(name = %task_name, "starting task");
            let result = task.run(ctx.clone(), task_service).await;
            let duration = instant_start.elapsed();
            info!(name = %task_name, duration = format!("{}s", duration.as_secs()), "finished task");

            let end_at = Utc::now().naive_utc();
            let status = match &result {
                Ok(_) => db::TaskResultStatus::Completed,
                Err(_) => db::TaskResultStatus::Failed,
            };

            if let Err(e) = &result {
                error!(task_key = %task_key, error = %e, "Task failed");
            }

            let task_result = db::TaskResult {
                task_id: task_key.clone(),
                start_at: start_time,
                end_at,
                status,
            };

            if let Err(e) = task_result.save(&ctx.db).await {
                error!(task_key = %task_key, error = %e, "Failed to save task result");
            }
        });

        self.handle = Some(handle);
    }

    pub fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            self.status = TaskStatus::Stopped;
            info!(task_key = %self.task.key(), "Task stopped");
        }
    }
}

#[derive(Clone)]
pub struct TaskService {
    scheduler: JobScheduler,
    tasks: Arc<Mutex<HashMap<String, TaskHandler>>>, // Now keyed by task key (String)
    ctx: AppContext,
}

impl TaskService {
    pub async fn new(ctx: AppContext) -> Result<Self> {
        let scheduler = JobScheduler::new().await?;
        let tasks = Arc::new(Mutex::new(HashMap::new()));

        let service = Self {
            scheduler,
            tasks,
            ctx: ctx.clone(),
        };

        service
            .register_task(Arc::new(RefreshLibraryTask::default()))
            .await?;
        service
            .register_task(Arc::new(CatalogImportTask::default()))
            .await?;
        service
            .register_task(Arc::new(SeriesSyncTask::default()))
            .await?;

        // Register default triggers for tasks
        service.register_default_triggers().await?;

        let triggers = db::TaskTrigger::get_all(&service.ctx.db).await?;
        for trigger in triggers {
            service.add_trigger(trigger).await?;
        }

        Ok(service)
    }

    pub async fn register_task(&self, task: Arc<dyn Task>) -> Result<()> {
        let task_key = task.key().to_string();

        self.tasks
            .lock()
            .await
            .insert(task_key, TaskHandler::new(task.clone()));

        Ok(())
    }

    pub async fn add_trigger(&self, trigger: db::TaskTrigger) -> Result<()> {
        let Some(cron) = trigger.cron else {
            return Ok(());
        };

        let tasks = self.tasks.clone();
        let ctx = self.ctx.clone();
        let task_service = self.clone();

        let task_id = trigger.task_id.clone(); // Clone for the closure
        let job = Job::new_async(cron.as_str(), move |_uuid, _l| {
            let tasks = tasks.clone();
            let ctx = ctx.clone();
            let task_service = task_service.clone();
            let task_id = task_id.clone(); // Clone again for the async block

            Box::pin(async move {
                if let Some(handler) = tasks.lock().await.get_mut(&task_id) {
                    handler.run(ctx, task_service.into());
                }
            })
        })?;

        let job_id = job.guid();
        self.scheduler.add(job).await?;

        if let Some(handler) = self.tasks.lock().await.get_mut(&trigger.task_id) {
            handler.jobs.push(job_id);
        }

        Ok(())
    }

    pub async fn replace_triggers(
        &self,
        task_key: &str,
        triggers: Vec<db::TaskTrigger>,
    ) -> Result<()> {
        let mut tasks = self.tasks.lock().await;
        let handler = tasks
            .get_mut(task_key)
            .ok_or_else(|| anyhow!("Task not found"))?;

        for job_id in handler.jobs.drain(..) {
            let _ = self.scheduler.remove(&job_id).await;
        }

        drop(tasks);

        for trigger in triggers {
            self.add_trigger(trigger).await?;
        }

        Ok(())
    }

    /// Run a task by its UUID (kept for backward compatibility, but tasks now use keys)
    pub async fn run_task(&self, _task_id: Uuid) -> Result<()> {
        // Tasks now use keys instead of UUIDs, so this method is deprecated
        // Keep it for compatibility but it won't find anything
        Ok(())
    }

    /// Run a task by its key (primary method)
    pub async fn run_task_by_key(&self, task_key: &str) -> Result<()> {
        if let Some(handler) = self.tasks.lock().await.get_mut(task_key) {
            handler.run(self.ctx.clone(), self.clone().into());
        }
        Ok(())
    }

    pub async fn run_startup_tasks(&self) -> Result<()> {
        let triggers = db::TaskTrigger::get_all(&self.ctx.db).await?;
        let tasks = self.tasks.clone();
        let ctx = self.ctx.clone();

        for trigger in triggers {
            if trigger.kind == db::TaskTriggerKind::Startup {
                let task_key = trigger.task_id;
                let tasks_clone = tasks.clone();
                let ctx = ctx.clone();

                tokio::spawn({
                    let task_service = self.clone();
                    async move {
                        if let Some(handler) =
                            tasks_clone.lock().await.get_mut(&task_key)
                        {
                            handler.run(ctx, task_service.into());
                        } else {
                            error!(task_key = %task_key, "Task handler not found for startup task");
                        }
                    }
                });
            }
        }

        Ok(())
    }

    /// Stop a task by its UUID (backward compatibility)
    pub async fn stop_task(&self, task_id: Uuid) -> Result<()> {
        // Try to find by UUID first
        if let Some(handler) = self.tasks.lock().await.values()
            .find(|h| false) { // Always false since we don't use UUIDs anymore
            let task_key = handler.key().to_string();
            if let Some(handler) = self.tasks.lock().await.get_mut(&task_key) {
                handler.stop();
            }
        }
        Ok(())
    }

    /// Stop a task by its key (primary method)
    pub async fn stop_task_by_key(&self, task_key: &str) -> Result<()> {
        if let Some(handler) = self.tasks.lock().await.get_mut(task_key) {
            handler.stop();
        }
        Ok(())
    }

    pub async fn start(&self) -> Result<()> {
        self.scheduler.start().await?;
        Ok(())
    }

    /// Register default triggers for tasks
    pub async fn register_default_triggers(&self) -> Result<()> {
        let tasks = self.tasks.lock().await;
        
        // Register CatalogImportTask as a startup trigger
        for (task_id, handler) in tasks.iter() {
            if handler.task.key() == "CatalogImport" {
                // Use task key as the task_id instead of UUID
                let trigger = db::TaskTrigger {
                    id: Uuid::new_v4().to_string(), // Keep UUID for trigger ID
                    task_id: handler.task.key().to_string(), // Use task key
                    kind: db::TaskTriggerKind::Startup,
                    time_limit_hours: None,
                    cron: None,
                };
                
                // Check if this trigger already exists
                let existing_triggers = db::TaskTrigger::get_all(&self.ctx.db).await?;
                let trigger_exists = existing_triggers.iter()
                    .any(|t| t.task_id == handler.task.key() && t.kind == db::TaskTriggerKind::Startup);
                
                if !trigger_exists {
                    trigger.save(&self.ctx.db).await?;
                    self.add_trigger(trigger).await?;
                }
                break;
            }
        }
        
        Ok(())
    }

    /// Get a copy of all task handlers for read-only access
    pub async fn get_task_handlers(&self) -> std::collections::HashMap<String, crate::tasks::TaskHandlerSnapshot> {
        self.tasks.lock().await.iter()
            .map(|(k, v)| (k.clone(), v.to_snapshot()))
            .collect()
    }
}

#[derive(Default)]
pub struct RefreshLibraryTask;

#[async_trait]
impl Task for RefreshLibraryTask {
    fn name(&self) -> &str {
        "Refresh Library"
    }

    fn key(&self) -> &str {
        "RefreshLibrary"
    }

    fn category(&self) -> &str {
        "Library"
    }

    fn default_triggers(&self) -> Vec<db::TaskTrigger> {
        vec![]
    }

    async fn run(
        self: Arc<Self>,
        ctx: AppContext,
        _task_service: Arc<TaskService>,
    ) -> Result<()> {
        let service = MetaProviderService::new(
            vec![Box::new(AioMetaProvider)],
            vec![Box::new(AioTreeSyncProvider)],
        );
        let media_list = db::Media::get_refreshable(&ctx.db).await?;
        let updated_media = service.process(media_list, &ctx).await?;
        db::Media::upsert(&ctx.db, &updated_media).await?;

        Ok(())
    }
}

#[derive(Default)]
pub struct CatalogImportTask;

#[async_trait]
impl Task for CatalogImportTask {
    fn name(&self) -> &str {
        "Catalog Import"
    }

    fn key(&self) -> &str {
        "CatalogImport"
    }

    fn category(&self) -> &str {
        "Library"
    }

    fn default_triggers(&self) -> Vec<db::TaskTrigger> {
        vec![]
    }

    async fn run(
        self: Arc<Self>,
        ctx: AppContext,
        task_service: Arc<TaskService>,
    ) -> Result<()> {
        let manifest = ctx.aio.get_manifest().await?;
        let start_time = Instant::now();
        let mut total_imported = 0;

        info!("starting catalog import ({})", manifest.catalogs.len());

        for cat in manifest.catalogs {
            if cat.id.contains("search") {
                continue;
            };

            info!("Importing catalog {} {}", cat.id, cat.kind);
            let aio_id = format!("{}:{}", cat.kind, cat.id);
            let mut media_cat = db::Media::get_by_filter(
                &ctx.db,
                &db::MediaFilter {
                    aio_id: Some(aio_id.clone()),
                    ..Default::default()
                },
            )
            .await?
            .records
            .first()
            .cloned()
            .unwrap_or_else(|| db::Media {
                title: cat.name.clone(),
                kind: db::MediaKind::Catalog,
                aio_id: Some(aio_id),
                catalog_kind: Some(db::CatalogKind::Manual),
                ..Default::default()
            });

            media_cat.save(&ctx.db).await?;

            let mut meta_stream = ctx.aio.get_catalog_stream(&cat).await.chunks(500);
            let mut count = 0;
            while let Some(mut metas) = meta_stream.next().await {
                // metas.retain(|obj| obj.imdb_id.is_some());
                let remaining = ctx.config.catalog_max_items.saturating_sub(count);

                metas = metas.into_iter().take(remaining).collect();
                // let imdb_ids: Vec<String> =
                //     metas.iter().map(|m| m.imdb_id.clone().unwrap()).collect();

                // let existing_imdb_ids = db::Media::get_by_filter(
                //     &ctx.db,
                //     &db::MediaFilter {
                //         imdb_id: Some(imdb_ids),
                //         ..Default::default()
                //     },
                // )
                // .await?
                // .records
                //.iter()
                //.filter_map(|m| m.imdb_id.as_deref())
                // .collect();

                // metas.retain(|m| {
                //     !existing_imdb_ids.contains(m.imdb_id.as_deref().unwrap())
                // });
                //if count > ctx.config.catalog_max_items {

                let items: Vec<db::Media> = metas
                    .into_iter()
                    .unique_by(|meta| meta.id.clone())
                    .flat_map(|meta| match Vec::<db::Media>::try_from(meta) {
                        Ok(items) => {
                            // check if catalog provides series tree
                            //if meta.is_series() && meta.videos.is_none() {
                            //return provider.get_series_tree().await?;
                            //}
                            items.into_iter()
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to convert metadata, skipping");
                            Vec::<db::Media>::new().into_iter()
                        }
                    })
                    .collect();

                if !items.is_empty() {
                    if let Err(e) = db::Media::insert(&ctx.db, &items).await {
                        error!("Failed to import chunk: {}", e);
                    } else {
                        count += items.len();
                        total_imported += count;
                    }
                } else {
                    drop(meta_stream);
                    break;
                }

                if count > ctx.config.catalog_max_items {
                    drop(meta_stream);
                    break;
                }
            }

            info!(
                "finished Importing catalog {} | {} ({} items)",
                cat.id, cat.kind, count
            );
        }

        let duration = start_time.elapsed();
        info!(
            "import complete. Total media items imported: {}.",
            total_imported
        );

        // Kick off RefreshLibraryTask using its key
        task_service.run_task_by_key("RefreshLibrary").await?;

        Ok(())
    }
}

#[derive(Default)]
pub struct SeriesSyncTask;

#[async_trait]
impl Task for SeriesSyncTask {
    fn name(&self) -> &str {
        "Series sync"
    }

    fn category(&self) -> &str {
        "Library"
    }

    fn default_triggers(&self) -> Vec<db::TaskTrigger> {
        vec![]
    }

    async fn run(
        self: Arc<Self>,
        ctx: AppContext,
        _task_service: Arc<TaskService>,
    ) -> Result<()> {
        let service = MetaProviderService::new(
            vec![Box::new(AioMetaProvider)],
            vec![Box::new(AioTreeSyncProvider)],
        );
        let media_list = db::Media::get_by_filter(
            &ctx.db,
            &db::MediaFilter {
                kind: Some(vec![db::MediaKind::Series]),
                ..Default::default()
            },
        )
        .await?
        .records;

        let updated_media = service.process(media_list, &ctx).await?;
        db::Media::upsert(&ctx.db, &updated_media).await?;

        Ok(())
    }
}
