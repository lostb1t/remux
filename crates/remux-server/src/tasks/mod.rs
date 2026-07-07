use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use tokio::{sync::Mutex as AsyncMutex, task::JoinHandle};
use tokio_cron_scheduler::{Job, JobScheduler, job::JobId};
use tracing::{error, info};

use crate::{AppContext, db, ws};
use remux_sdks::remux::TaskTriggerInfoType;
use strum_macros::{Display, EnumString};

mod catalog_import_shared;
mod clean_transcode_folder;
mod clear_cache;
mod clear_image_cache;
mod jellyfin_import;
mod purge_iptv;
mod purge_media;
mod purge_metrics;
mod refresh_all_meta;
mod refresh_iptv;
mod refresh_library;
mod refresh_popularity;
mod series_sync;
mod stress_meta;

pub use crate::common::ProgressReporter;
use clean_transcode_folder::CleanTranscodeFolderTask;
use clear_cache::ClearCacheTask;
use clear_image_cache::ClearImageCacheTask;
use jellyfin_import::JellyfinImportTask;
use purge_iptv::PurgeIptvTask;
use purge_media::PurgeMediaTask;
use purge_metrics::PurgeMetricsTask;
use refresh_all_meta::RefreshAllMetaTask;
use refresh_iptv::RefreshIptvTask;
use refresh_library::RefreshLibraryTask;
use refresh_popularity::RefreshPopularityTask;
use series_sync::SeriesSyncTask;
use stress_meta::StressMetaTask;

// --- Task category ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Display, EnumString)]
pub enum TaskCategory {
    Library,
    #[strum(to_string = "Live TV")]
    LiveTv,
    Users,
    Maintenance,
}

impl TaskCategory {
    pub fn rank(self) -> usize {
        match self {
            Self::Library => 0,
            Self::LiveTv => 1,
            Self::Users => 2,
            Self::Maintenance => 3,
        }
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
    fn description(&self) -> &str {
        ""
    }
    fn short_description(&self) -> &str {
        ""
    }
    fn category(&self) -> TaskCategory {
        TaskCategory::Maintenance
    }
    fn destructive(&self) -> bool {
        false
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
    jobs: Vec<JobId>,
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
        self.task
            .key()
    }

    fn lock_status(&self) -> std::sync::MutexGuard<'_, TaskStatus> {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_running(&self) -> bool {
        self.handle
            .as_ref()
            .map(|h| !h.is_finished())
            .unwrap_or(false)
    }

    pub fn view(&self) -> TaskView {
        TaskView {
            task: self
                .task
                .clone(),
            status: self
                .lock_status()
                .clone(),
            progress: f64::from_bits(
                self.progress
                    .load(Ordering::Relaxed),
            ),
        }
    }

    pub fn run(&mut self, ctx: AppContext, task_service: Arc<TaskService>) {
        if self.is_running() {
            return;
        }

        self.progress
            .store(0u64, Ordering::Relaxed);
        *self.lock_status() = TaskStatus::Running;

        let task = self
            .task
            .clone();
        let task_key = task
            .key()
            .to_string();
        let status = self
            .status
            .clone();
        let progress = ProgressReporter::new(
            self.progress
                .clone(),
        );

        let handle = tokio::spawn(async move {
            let start_at = Utc::now().naive_utc();
            let instant = Instant::now();
            info!(task = %task.name(), "starting");

            let result = task
                .run(ctx.clone(), task_service, progress)
                .await;
            let elapsed = instant.elapsed();

            // Flush WAL after every task so write bursts don't accumulate into
            // a large WAL that degrades subsequent read performance.
            sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
                .execute(&ctx.db)
                .await
                .ok();

            // Update query-planner statistics so bulk-write tasks don't leave
            // stale sqlite_stat1 rows that cause full-table scans on reads.
            sqlx::query("PRAGMA optimize")
                .execute(&ctx.db)
                .await
                .ok();

            let (new_status, db_status) = match &result {
                Ok(_) => {
                    info!(task = %task.name(), elapsed = ?elapsed, "completed");
                    let _ = ctx
                        .ws_tx
                        .send(ws::WsEvent::LibraryChanged);
                    (TaskStatus::Idle, db::TaskResultStatus::Completed)
                }
                Err(e) => {
                    error!(task = %task_key, error = %e, "failed");
                    (
                        TaskStatus::Failed(e.to_string()),
                        db::TaskResultStatus::Failed,
                    )
                }
            };

            *status
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = new_status;

            let task_result = db::TaskResult {
                task_id: task_key.clone(),
                start_at,
                end_at: Utc::now().naive_utc(),
                status: db_status,
            };

            if let Err(e) = task_result
                .save(&ctx.db)
                .await
            {
                error!(task = %task_key, error = %e, "failed to save result");
            }
        });

        self.handle = Some(handle);
    }

    pub fn stop(&mut self) {
        if let Some(handle) = self
            .handle
            .take()
        {
            handle.abort();
            *self.lock_status() = TaskStatus::Stopped;
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

        let service = Self {
            scheduler,
            tasks,
            ctx: ctx.clone(),
        };

        service
            .register_task(Arc::new(ClearCacheTask))
            .await?;
        service
            .register_task(Arc::new(ClearImageCacheTask))
            .await?;
        service
            .register_task(Arc::new(CleanTranscodeFolderTask))
            .await?;
        service
            .register_task(Arc::new(RefreshLibraryTask))
            .await?;
        service
            .register_task(Arc::new(RefreshAllMetaTask))
            .await?;
        // service.register_task(Arc::new(SeriesSyncTask)).await?;
        service
            .register_task(Arc::new(PurgeMediaTask))
            .await?;
        service
            .register_task(Arc::new(PurgeIptvTask))
            .await?;
        service
            .register_task(Arc::new(JellyfinImportTask))
            .await?;
        service
            .register_task(Arc::new(RefreshIptvTask))
            .await?;
        service
            .register_task(Arc::new(RefreshPopularityTask))
            .await?;
        service
            .register_task(Arc::new(PurgeMetricsTask))
            .await?;
        service
            .register_task(Arc::new(StressMetaTask))
            .await?;

        let triggers = db::TaskTrigger::get_all(
            &service
                .ctx
                .db,
        )
        .await?;
        for trigger in triggers {
            if let Err(e) = service
                .add_trigger(trigger)
                .await
            {
                error!("Failed to add trigger (skipping): {}", e);
            }
        }

        Ok(service)
    }

    pub async fn register_task(&self, task: Arc<dyn Task>) -> Result<()> {
        let key = task
            .key()
            .to_lowercase();
        self.tasks
            .lock()
            .await
            .insert(key, TaskHandler::new(task));
        Ok(())
    }

    pub async fn add_trigger(&self, trigger: db::TaskTrigger) -> Result<()> {
        let Some(cron) = trigger.cron else {
            return Ok(());
        };

        let tasks = self
            .tasks
            .clone();
        let ctx = self
            .ctx
            .clone();
        let task_service = self.clone();
        let task_id = trigger
            .task_id
            .to_lowercase();
        let task_id_for_closure = task_id.clone();

        let job = Job::new_async(cron.as_str(), move |_uuid, _l| {
            let tasks = tasks.clone();
            let ctx = ctx.clone();
            let task_service = Arc::new(task_service.clone());
            let task_id = task_id_for_closure.clone();

            Box::pin(async move {
                if let Some(handler) = tasks
                    .lock()
                    .await
                    .get_mut(&task_id)
                {
                    handler.run(ctx, task_service);
                }
            })
        })?;

        let job_id = job.guid();
        self.scheduler
            .add(job)
            .await?;

        if let Some(handler) = self
            .tasks
            .lock()
            .await
            .get_mut(&task_id)
        {
            handler
                .jobs
                .push(job_id);
        }

        Ok(())
    }

    pub async fn replace_triggers(
        &self,
        task_key: &str,
        triggers: Vec<db::TaskTrigger>,
    ) -> Result<()> {
        let key = task_key.to_lowercase();
        let mut tasks = self
            .tasks
            .lock()
            .await;
        let handler = tasks
            .get_mut(&key)
            .ok_or_else(|| anyhow!("Task not found: {task_key}"))?;

        for job_id in handler
            .jobs
            .drain(..)
        {
            let _ = self
                .scheduler
                .remove(&job_id)
                .await;
        }

        drop(tasks);

        db::TaskTrigger::delete_by_task_id(
            &self
                .ctx
                .db,
            task_key,
        )
        .await?;

        for trigger in triggers {
            trigger
                .save(
                    &self
                        .ctx
                        .db,
                )
                .await?;
            self.add_trigger(trigger)
                .await?;
        }

        Ok(())
    }

    pub async fn run_task(&self, task_key: &str) -> Result<()> {
        if let Some(handler) = self
            .tasks
            .lock()
            .await
            .get_mut(&task_key.to_lowercase())
        {
            handler.run(
                self.ctx
                    .clone(),
                Arc::new(self.clone()),
            );
        }
        Ok(())
    }

    pub async fn run_startup_tasks(&self) -> Result<()> {
        let triggers = db::TaskTrigger::get_all(
            &self
                .ctx
                .db,
        )
        .await?;
        for trigger in triggers {
            if trigger.kind == TaskTriggerInfoType::StartupTrigger {
                self.run_task(&trigger.task_id)
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn stop_task(&self, task_key: &str) -> Result<()> {
        if let Some(handler) = self
            .tasks
            .lock()
            .await
            .get_mut(&task_key.to_lowercase())
        {
            handler.stop();
        }
        Ok(())
    }

    pub async fn deregister_task(&self, key: &str) -> Result<()> {
        let key = key.to_lowercase();
        let mut tasks = self
            .tasks
            .lock()
            .await;
        if let Some(mut handler) = tasks.remove(&key) {
            handler.stop();
            for job_id in &handler.jobs {
                let _ = self
                    .scheduler
                    .remove(job_id)
                    .await;
            }
        }
        Ok(())
    }

    pub async fn start(&self) -> Result<()> {
        self.scheduler
            .start()
            .await?;
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

pub(super) fn iter_dir(
    path: impl AsRef<std::path::Path>,
) -> impl Iterator<Item = std::fs::DirEntry> {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
}

/// Returns the current RSS in MiB.
/// Linux: reads VmRSS from /proc/self/status (current, can decrease).
/// macOS: reads ru_maxrss from getrusage (peak, monotonically increasing).
pub(crate) fn rss_mb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("VmRSS:"))
                    .and_then(|l| {
                        l.split_whitespace()
                            .nth(1)
                    })
                    .and_then(|v| {
                        v.parse::<u64>()
                            .ok()
                    })
            })
            .unwrap_or(0)
            / 1024
    }
    #[cfg(not(target_os = "linux"))]
    {
        unsafe {
            let mut info = std::mem::zeroed::<libc::rusage>();
            libc::getrusage(libc::RUSAGE_SELF, &mut info);
            (info.ru_maxrss as u64) / (1024 * 1024)
        }
    }
}
