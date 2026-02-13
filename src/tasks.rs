use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{self, StreamExt};
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
    meta_provider::{AioMetaProvider, MetaProvider},
};

#[derive(Debug)]
pub enum TaskStatus {
    Idle,
    Active,
    Stopped,
    Failed(anyhow::Error),
}

#[async_trait]
pub trait Task: Send + Sync {
    fn id(&self) -> Uuid;
    fn name(&self) -> &str;
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

impl TaskHandler {
    pub fn new(task: Arc<dyn Task>) -> Self {
        Self {
            status: TaskStatus::Idle,
            task,
            jobs: Vec::new(),
            handle: None,
        }
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
        let task_id = task.id();

        let handle = tokio::spawn(async move {
            //let start_at = Utc::now().naive_utc();
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
                error!(task_id = %task_id, error = %e, "Task failed");
            }

            let task_result = db::TaskResult {
                task_id,
                start_at: start_time,
                end_at,
                status,
            };

            if let Err(e) = task_result.save(&ctx.db).await {
                error!(task_id = %task_id, error = %e, "Failed to save task result");
            }
        });

        self.handle = Some(handle);
    }

    pub fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            self.status = TaskStatus::Stopped;
            info!(task_id = %self.task.id(), "Task stopped");
        }
    }
}

#[derive(Clone)]
pub struct TaskService {
    scheduler: JobScheduler,
    tasks: Arc<Mutex<HashMap<Uuid, TaskHandler>>>,
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
            .register_task(Arc::new(MediaScanTask::default()))
            .await?;
        service
            .register_task(Arc::new(CatalogImportTask::default()))
            .await?;

        let triggers = db::TaskTrigger::get_all(&service.ctx.db).await?;
        for trigger in triggers {
            service.add_trigger(trigger).await?;
        }

        Ok(service)
    }

    pub async fn register_task(&self, task: Arc<dyn Task>) -> Result<()> {
        let task_id = task.id();

        self.tasks
            .lock()
            .await
            .insert(task_id, TaskHandler::new(task.clone()));

        Ok(())
    }

    pub async fn add_trigger(&self, trigger: db::TaskTrigger) -> Result<()> {
        let Some(cron) = trigger.cron else {
            return Ok(());
        };

        let tasks = self.tasks.clone();
        let ctx = self.ctx.clone();
        let task_service = self.clone();

        let job = Job::new_async(cron.as_str(), move |_uuid, _l| {
            let tasks = tasks.clone();
            let ctx = ctx.clone();
            let task_service = task_service.clone();

            Box::pin(async move {
                if let Some(handler) = tasks.lock().await.get_mut(&trigger.task_id) {
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
        task_id: Uuid,
        triggers: Vec<db::TaskTrigger>,
    ) -> Result<()> {
        let mut tasks = self.tasks.lock().await;
        let handler = tasks
            .get_mut(&task_id)
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

    pub async fn run_task(&self, task_id: Uuid) -> Result<()> {
        if let Some(handler) = self.tasks.lock().await.get_mut(&task_id) {
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
                let task_id = trigger.task_id;
                let tasks_clone = tasks.clone();
                let ctx = ctx.clone();

                tokio::spawn({
                    let task_service = self.clone();
                    async move {
                        if let Some(handler) =
                            tasks_clone.lock().await.get_mut(&task_id)
                        {
                            handler.run(ctx, task_service.into());
                        } else {
                            error!(task_id = %task_id, "Task handler not found for startup task");
                        }
                    }
                });
            }
        }

        Ok(())
    }

    pub async fn stop_task(&self, task_id: Uuid) -> Result<()> {
        if let Some(handler) = self.tasks.lock().await.get_mut(&task_id) {
            handler.stop();
        }
        Ok(())
    }

    pub async fn start(&self) -> Result<()> {
        self.scheduler.start().await?;
        Ok(())
    }
}

#[derive(Default)]
pub struct MediaScanTask;

#[async_trait]
impl Task for MediaScanTask {
    fn id(&self) -> Uuid {
        uuid!("f47ac10b-58cc-4372-a567-0e02b2c3d479")
    }

    fn name(&self) -> &str {
        "Media Scan"
    }

    fn default_triggers(&self) -> Vec<db::TaskTrigger> {
        vec![]
    }

    async fn run(
        self: Arc<Self>,
        ctx: AppContext,
        _task_service: Arc<TaskService>,
    ) -> Result<()> {
        let provider = AioMetaProvider {};
        let media_list = db::Media::get_refreshable(&ctx.db).await?;
        //let mut updated_media: Vec<db::Media> = vec![];
        let mut updated_media = provider.apply_many(media_list, ctx.clone()).await?;
        db::Media::upsert(&ctx.db, &updated_media).await?;

        Ok(())
    }
}

#[derive(Default)]
pub struct CatalogImportTask;

#[async_trait]
impl Task for CatalogImportTask {
    fn id(&self) -> Uuid {
        uuid!("73733828-2828-4b8a-9e1a-737338282828")
    }

    fn name(&self) -> &str {
        "Catalog Import"
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

        // Kick off MediaScanTask
        let media_scan_task_id = uuid!("f47ac10b-58cc-4372-a567-0e02b2c3d479");
        task_service.run_task(media_scan_task_id).await?;

        Ok(())
    }
}

#[derive(Default)]
pub struct SeriesSyncTask;

#[async_trait]
impl Task for SeriesSyncTask {
    fn id(&self) -> Uuid {
        uuid!("f47ac10b-58cc-4372-a567-0e02b2c3d479")
    }

    fn name(&self) -> &str {
        "Series sync"
    }

    fn default_triggers(&self) -> Vec<db::TaskTrigger> {
        vec![]
    }

    async fn run(
        self: Arc<Self>,
        ctx: AppContext,
        _task_service: Arc<TaskService>,
    ) -> Result<()> {
        let provider = AioMetaProvider {};
        let media_list = db::Media::get_by_filter(
            &ctx.db,
            &db::MediaFilter {
                kind: Some(vec![db::MediaKind::Series]),
                ..Default::default()
            },
        )
        .await?
        .records;

        let updated_media = stream::iter(media_list)
            .map(|media| {
                let provider = &provider;
                let ctx = ctx.clone();
                async move { provider.apply(media, &ctx).await }
            })
            .buffer_unordered(10)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<db::Media>>>()?;

        db::Media::upsert(&ctx.db, &updated_media).await?;

        Ok(())
    }
}
