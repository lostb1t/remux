use std::collections::HashMap;
use std::sync::Arc;

use crate::Settings;
use crate::aio;
use crate::db;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use futures_util::StreamExt;
use itertools::Itertools;
use sqlx::SqlitePool;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_cron_scheduler::job::JobId;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::info;
use tracing::warn;
use uuid::Uuid;
use uuid::uuid;

#[derive(Clone)]
pub struct TaskContext {
    pub db: SqlitePool,
    pub config: Settings,
    pub aio: aio::AioService,
}

#[derive(Debug)]
pub enum TaskState {
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

    async fn run(self: Arc<Self>, context: TaskContext) -> Result<()>;
}

pub struct TaskHandler {
    pub state: TaskState,
    pub task: Arc<dyn Task>,
    pub jobs: Vec<JobId>,
    pub handle: Option<JoinHandle<()>>,
}

impl TaskHandler {
    pub fn new(task: Arc<dyn Task>) -> Self {
        Self {
            state: TaskState::Idle,
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

    pub fn run(&mut self, context: TaskContext) {
        if self.is_running() {
            return;
        }

        self.state = TaskState::Active;
        let task = self.task.clone();
        let task_id = task.id();

        let handle = tokio::spawn(async move {
            let start_at = Utc::now().naive_utc();
            let task_name = task.name().to_string();
            tracing::info!(name = %task_name, "starting task");
            let result = task.run(context.clone()).await;
            tracing::info!(name = %task_name, "finished task");

            let end_at = Utc::now().naive_utc();
            let state = match &result {
                Ok(_) => db::TaskResultState::Completed,
                Err(_) => db::TaskResultState::Failed,
            };

            if let Err(e) = &result {
                tracing::error!(task_id = %task_id, error = %e, "Task failed");
            }

            let task_result = db::TaskResult {
                task_id,
                start_at,
                end_at,
                state,
            };

            if let Err(e) = task_result.save(&context.db).await {
                tracing::error!(task_id = %task_id, error = %e, "Failed to save task result");
            }
        });

        self.handle = Some(handle);
    }

    pub fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            self.state = TaskState::Stopped;
            tracing::info!(task_id = %self.task.id(), "Task stopped");
        }
    }
}

#[derive(Clone)]
pub struct TaskService {
    scheduler: JobScheduler,
    tasks: Arc<Mutex<HashMap<Uuid, TaskHandler>>>,
    context: TaskContext,
}

impl TaskService {
    pub async fn new(context: TaskContext) -> Result<Self> {
        let scheduler = JobScheduler::new().await?;
        let tasks = Arc::new(Mutex::new(HashMap::new()));

        let service = Self {
            scheduler,
            tasks,
            context: context,
        };

        service
            .register_task(Arc::new(MediaScanTask::default()))
            .await?;

        let triggers = db::TaskTrigger::get_all(&service.context.db).await?;
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
        let context = self.context.clone();

        let job = Job::new_async(cron.as_str(), move |_id, _lock| {
            let tasks = tasks.clone();
            let context = context.clone();

            Box::pin(async move {
                if let Some(handler) = tasks.lock().await.get_mut(&trigger.task_id) {
                    handler.run(context);
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
            handler.run(self.context.clone());
        }
        Ok(())
    }

    pub async fn run_startup_tasks(&self) -> Result<()> {
        let triggers = db::TaskTrigger::get_all(&self.context.db).await?;
        let tasks = self.tasks.clone();
        let context = self.context.clone();

        for trigger in triggers {
            if trigger.kind == db::TaskTriggerKind::Startup {
                let task_id = trigger.task_id;
                let tasks_clone = tasks.clone();
                let context = context.clone();

                tokio::spawn(async move {
                    if let Some(handler) = tasks_clone.lock().await.get_mut(&task_id) {
                        handler.run(context);
                    } else {
                        tracing::error!(task_id = %task_id, "Task handler not found for startup task");
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
        uuid!("73733828-2828-4b8a-9e1a-737338282828")
    }

    fn name(&self) -> &str {
        "Media scan"
    }

    fn default_triggers(&self) -> Vec<db::TaskTrigger> {
        vec![]
    }

    async fn run(self: Arc<Self>, context: TaskContext) -> Result<()> {
        let manifest = context.aio.get_manifest().await?;

        let start_time = Instant::now();
        let mut total_imported = 0;

        let media_items: Vec<db::Media> = manifest
            .catalogs
            .clone()
            .into_iter()
            .map(db::Media::from)
            .collect();

        db::Media::upsert(&context.db, &media_items).await.unwrap();

        info!("starting catalog import ({})", manifest.catalogs.len());

        for cat in manifest.catalogs {
            let aio_id = format!("{}:{}", cat.kind, cat.id);

            // upsert category
            let mut media_cat = db::Media::get_by_filter(
                &context.db,
                &db::MediaFilter {
                    aio_id: Some(aio_id.clone()),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .records
            .first()
            .cloned()
            .unwrap_or_else(|| db::Media {
                title: cat.name.clone(),
                kind: db::MediaKind::Catalog,
                aio_id: Some(aio_id),
                catalog_kind: Some(db::CatalogKind::Manual.to_string()),
                ..Default::default()
            });

            media_cat.save(&context.db).await.unwrap();

            let mut meta_stream =
                context.aio.get_catalog_stream(&cat).await.chunks(900);
            let mut count = 0;
            while let Some(metas) = meta_stream.next().await {
                let items: Vec<db::Media> = metas
                    .into_iter()
                    .unique_by(|meta| meta.id.clone())
                    .flat_map(|meta| match Vec::<db::Media>::try_from(meta) {
                        Ok(items) => items.into_iter(),
                        Err(e) => {
                            warn!(error = %e, "Failed to convert metadata, skipping");
                            Vec::<db::Media>::new().into_iter()
                        }
                    })
                    // .filter(|item| {
                    //     matches!(
                    //         item.kind,
                    //         db::MediaKind::Movie | db::MediaKind::Series
                    //     )
                    // })
                    .collect();

                if !items.is_empty() {
                    if let Err(e) = db::Media::insert(&context.db, &items).await {
                        tracing::error!("Failed to import chunk: {}", e);
                    } else {
                        count += items.len();
                        total_imported += count;
                    }
                }
            }

            info!(
                "Imported catalog {} | {} ({} items)",
                cat.id, cat.kind, count
            );
        }

        let duration = start_time.elapsed();
        info!(
            "Import complete. Total media items imported: {}. Time taken: {:?}",
            total_imported, duration
        );

        Ok(())
    }
}
