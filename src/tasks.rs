use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use futures::stream::StreamExt;
use itertools::Itertools;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;
use tokio_cron_scheduler::job::JobId;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info, warn};

use crate::{
    AppContext, db,
    meta_provider::{AioMetaProvider, AioTreeSyncProvider, MetaProviderService},
};

// --- Progress reporting ---

#[derive(Clone)]
pub struct ProgressReporter(Arc<AtomicU64>);

impl ProgressReporter {
    fn new(inner: Arc<AtomicU64>) -> Self {
        Self(inner)
    }

    pub fn set(&self, pct: f64) {
        let rounded = (pct.clamp(0.0, 100.0) * 10.0).round() / 10.0;
        self.0.store(rounded.to_bits(), Ordering::Relaxed);
    }
}

// --- Task status ---

#[derive(Debug, Clone, PartialEq)]
pub enum TaskStatus {
    Idle,
    Running,
    Stopped,
    Failed(String),
}

// --- Task trait ---

#[async_trait]
pub trait Task: Send + Sync + 'static {
    fn key(&self) -> &str;
    fn name(&self) -> &str;
    fn category(&self) -> &str {
        "System"
    }

    async fn run(
        &self,
        ctx: AppContext,
        tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()>;
}

// --- TaskView (read-only snapshot for API consumers) ---

pub struct TaskView {
    pub task: Arc<dyn Task>,
    pub status: TaskStatus,
    pub progress: f64,
}

// --- TaskHandler ---

pub struct TaskHandler {
    task: Arc<dyn Task>,
    pub jobs: Vec<JobId>,
    status: Arc<Mutex<TaskStatus>>,
    progress: Arc<AtomicU64>,
    handle: Option<JoinHandle<()>>,
}

impl TaskHandler {
    fn new(task: Arc<dyn Task>) -> Self {
        Self {
            task,
            jobs: Vec::new(),
            status: Arc::new(Mutex::new(TaskStatus::Idle)),
            progress: Arc::new(AtomicU64::new(0)),
            handle: None,
        }
    }

    pub fn key(&self) -> &str {
        self.task.key()
    }

    pub fn is_running(&self) -> bool {
        self.handle.as_ref().map(|h| !h.is_finished()).unwrap_or(false)
    }

    pub fn view(&self) -> TaskView {
        TaskView {
            task: self.task.clone(),
            status: self.status.lock().unwrap().clone(),
            progress: f64::from_bits(self.progress.load(Ordering::Relaxed)),
        }
    }

    pub fn run(&mut self, ctx: AppContext, task_service: Arc<TaskService>) {
        if self.is_running() {
            return;
        }

        self.progress.store(0u64, Ordering::Relaxed);
        *self.status.lock().unwrap() = TaskStatus::Running;

        let task = self.task.clone();
        let task_key = task.key().to_string();
        let status = self.status.clone();
        let progress = ProgressReporter::new(self.progress.clone());

        let handle = tokio::spawn(async move {
            let start_at = Utc::now().naive_utc();
            let instant = Instant::now();
            info!(task = %task.name(), "starting");

            let result = task.run(ctx.clone(), task_service, progress).await;
            let elapsed = instant.elapsed();

            let (new_status, db_status) = match &result {
                Ok(_) => {
                    info!(task = %task.name(), elapsed = ?elapsed, "completed");
                    (TaskStatus::Idle, db::TaskResultStatus::Completed)
                }
                Err(e) => {
                    error!(task = %task_key, error = %e, "failed");
                    (TaskStatus::Failed(e.to_string()), db::TaskResultStatus::Failed)
                }
            };

            *status.lock().unwrap() = new_status;

            let task_result = db::TaskResult {
                task_id: task_key.clone(),
                start_at,
                end_at: Utc::now().naive_utc(),
                status: db_status,
            };

            if let Err(e) = task_result.save(&ctx.db).await {
                error!(task = %task_key, error = %e, "failed to save result");
            }
        });

        self.handle = Some(handle);
    }

    pub fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            *self.status.lock().unwrap() = TaskStatus::Stopped;
            info!(task = %self.task.key(), "stopped");
        }
    }
}

// --- TaskService ---

#[derive(Clone)]
pub struct TaskService {
    scheduler: JobScheduler,
    tasks: Arc<AsyncMutex<HashMap<String, TaskHandler>>>,
    ctx: AppContext,
}

impl TaskService {
    pub async fn new(ctx: AppContext) -> Result<Self> {
        let scheduler = JobScheduler::new().await?;
        let tasks = Arc::new(AsyncMutex::new(HashMap::new()));

        let service = Self { scheduler, tasks, ctx: ctx.clone() };

        service.register_task(Arc::new(RefreshLibraryTask)).await?;
        service.register_task(Arc::new(CatalogImportTask)).await?;
        service.register_task(Arc::new(SeriesSyncTask)).await?;

        let triggers = db::TaskTrigger::get_all(&service.ctx.db).await?;
        for trigger in triggers {
            service.add_trigger(trigger).await?;
        }

        Ok(service)
    }

    pub async fn register_task(&self, task: Arc<dyn Task>) -> Result<()> {
        let key = task.key().to_lowercase();
        self.tasks.lock().await.insert(key, TaskHandler::new(task));
        Ok(())
    }

    pub async fn add_trigger(&self, trigger: db::TaskTrigger) -> Result<()> {
        let Some(cron) = trigger.cron else {
            return Ok(());
        };

        let tasks = self.tasks.clone();
        let ctx = self.ctx.clone();
        let task_service = self.clone();
        let task_id = trigger.task_id.clone();

        let job = Job::new_async(cron.as_str(), move |_uuid, _l| {
            let tasks = tasks.clone();
            let ctx = ctx.clone();
            let task_service = Arc::new(task_service.clone());
            let task_id = task_id.clone();

            Box::pin(async move {
                if let Some(handler) = tasks.lock().await.get_mut(&task_id) {
                    handler.run(ctx, task_service);
                }
            })
        })?;

        let job_id = job.guid();
        self.scheduler.add(job).await?;

        if let Some(handler) = self.tasks.lock().await.get_mut(&trigger.task_id.to_lowercase()) {
            handler.jobs.push(job_id);
        }

        Ok(())
    }

    pub async fn replace_triggers(
        &self,
        task_key: &str,
        triggers: Vec<db::TaskTrigger>,
    ) -> Result<()> {
        let key = task_key.to_lowercase();
        let mut tasks = self.tasks.lock().await;
        let handler = tasks
            .get_mut(&key)
            .ok_or_else(|| anyhow!("Task not found: {task_key}"))?;

        for job_id in handler.jobs.drain(..) {
            let _ = self.scheduler.remove(&job_id).await;
        }

        drop(tasks);

        for trigger in triggers {
            self.add_trigger(trigger).await?;
        }

        Ok(())
    }

    pub async fn run_task(&self, task_key: &str) -> Result<()> {
        if let Some(handler) = self.tasks.lock().await.get_mut(&task_key.to_lowercase()) {
            handler.run(self.ctx.clone(), Arc::new(self.clone()));
        }
        Ok(())
    }

    pub async fn run_startup_tasks(&self) -> Result<()> {
        let triggers = db::TaskTrigger::get_all(&self.ctx.db).await?;
        for trigger in triggers {
            if trigger.kind == db::TaskTriggerKind::Startup {
                self.run_task(&trigger.task_id).await?;
            }
        }
        Ok(())
    }

    pub async fn stop_task(&self, task_key: &str) -> Result<()> {
        if let Some(handler) = self.tasks.lock().await.get_mut(&task_key.to_lowercase()) {
            handler.stop();
        }
        Ok(())
    }

    pub async fn start(&self) -> Result<()> {
        self.scheduler.start().await?;
        Ok(())
    }

    pub async fn get_task_handlers(&self) -> HashMap<String, TaskView> {
        self.tasks
            .lock()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), v.view()))
            .collect()
    }
}

// --- Task implementations ---

pub struct RefreshLibraryTask;

#[async_trait]
impl Task for RefreshLibraryTask {
    fn key(&self) -> &str { "RefreshLibrary" }
    fn name(&self) -> &str { "Refresh Library" }
    fn category(&self) -> &str { "Library" }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        _progress: ProgressReporter,
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

pub struct CatalogImportTask;

#[async_trait]
impl Task for CatalogImportTask {
    fn key(&self) -> &str { "CatalogImport" }
    fn name(&self) -> &str { "Catalog Import" }
    fn category(&self) -> &str { "Library" }

    async fn run(
        &self,
        ctx: AppContext,
        tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        let manifest = ctx.aio.get_manifest().await?;
        let mut total_imported = 0;

        let catalogs: Vec<_> = manifest.catalogs.into_iter()
            .filter(|c| !c.id.contains("search"))
            .collect();
        let total = catalogs.len();

        info!("starting catalog import ({} catalogs)", total);

        for (i, cat) in catalogs.into_iter().enumerate() {
            progress.set(i as f64 / total as f64 * 100.0);
            info!("importing catalog {} {}", cat.id, cat.kind);
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
                let remaining = ctx.config.catalog_max_items.saturating_sub(count);
                metas = metas.into_iter().take(remaining).collect();

                let items: Vec<db::Media> = metas
                    .into_iter()
                    .unique_by(|meta| meta.id.clone())
                    .flat_map(|meta| match Vec::<db::Media>::try_from(meta) {
                        Ok(items) => items.into_iter(),
                        Err(e) => {
                            warn!(error = %e, "failed to convert metadata, skipping");
                            Vec::<db::Media>::new().into_iter()
                        }
                    })
                    .collect();

                if items.is_empty() {
                    break;
                }

                if let Err(e) = db::Media::insert(&ctx.db, &items).await {
                    error!("failed to import chunk: {}", e);
                } else {
                    count += items.len();
                    total_imported += count;
                }

                if count >= ctx.config.catalog_max_items {
                    break;
                }
            }

            info!("finished importing catalog {} {} ({} items)", cat.id, cat.kind, count);
        }

        info!("import complete, total: {}", total_imported);

        tasks.run_task("RefreshLibrary").await?;

        Ok(())
    }
}

pub struct SeriesSyncTask;

#[async_trait]
impl Task for SeriesSyncTask {
    fn key(&self) -> &str { "SeriesSync" }
    fn name(&self) -> &str { "Series Sync" }
    fn category(&self) -> &str { "Library" }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        _progress: ProgressReporter,
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
