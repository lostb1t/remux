//! Asking addons kept for seek previews only for streams, in the background.

use super::*;

/// What asking the addons kept for seek previews only needs (see
/// [`PreviewFinder`]).
#[derive(Debug, Clone)]
pub(crate) struct PreviewAsk {
    /// The movie or episode played.
    pub title_id: Uuid,
    /// The version played: its set and length decide which streams fit.
    pub playing: db::Media,
    /// The user playing: an addon outside their list is neither asked nor
    /// read for them.
    pub user_id: Uuid,
    /// Where the sets are, for the check that a stream has no folder of its
    /// own.
    pub root: PathBuf,
}

/// What a [`PreviewFinder`] found for a play.
#[derive(Debug, Clone, Default)]
pub(crate) struct Found {
    /// The streams that may be read, in order (see [`preview_versions`]).
    pub sources: Vec<PassSource>,
    /// Stored streams that would have been read but for their length (see
    /// [`other_lengths`]).
    pub other_lengths: usize,
}

/// Asks the addons kept for seek previews only for a play's title, when they
/// were not asked lately, and gives what of their stored answers may be read
/// for its pictures. The app's own in production ([`AppFinder`]), a fake in
/// tests. Called once a play has run its start delay and is still watched
/// (see [`TrickplayPasses::begin_with`]), never while a title opens or a play
/// starts.
pub(crate) trait PreviewFinder: Send + Sync {
    fn find(&self, ask: &PreviewAsk) -> impl Future<Output = Found> + Send;
}

/// The finder of a running server: asks through
/// [`crate::addons::AddonService::preview_streams`], then reads the stored
/// answers.
pub(crate) struct AppFinder {
    pub ctx: AppContext,
}

impl PreviewFinder for AppFinder {
    fn find(&self, ask: &PreviewAsk) -> impl Future<Output = Found> + Send {
        find_preview_sources(&self.ctx, ask)
    }
}

pub(super) async fn find_preview_sources(ctx: &AppContext, ask: &PreviewAsk) -> Found {
    let Some(mut title) = db::Media::get_by_id(&ctx.db, &ask.title_id)
        .await
        .ok()
        .flatten()
    else {
        return Found::default();
    };
    // An episode's lookup needs its series' ids, as a viewer's lookup does.
    let _ = title
        .grandparent(&ctx.db)
        .await;
    ctx.addons
        .preview_streams(&title, ctx, Some(ask.user_id))
        .await;
    // One play of a title at a time reads and measures its stored answers,
    // so a row is measured once: a second play reads the length saved by
    // the first.
    static MEASURE_LOCKS: crate::keyed_lock::KeyedLock<Uuid> =
        crate::keyed_lock::KeyedLock::new();
    let _guard = MEASURE_LOCKS
        .lock(ask.title_id)
        .await;
    let stored = match db::PreviewSource::list_fresh(
        &ctx.db,
        ask.title_id,
        chrono::Utc::now().naive_utc(),
    )
    .await
    {
        Ok(stored) => stored,
        Err(error) => {
            warn!(%error, "trick play: could not read the preview sources");
            return Found::default();
        }
    };
    let Ok(limited_to) = crate::addons::user_addon_override(&ctx.db, ask.user_id).await
    else {
        return Found::default();
    };
    let passes = &ctx
        .sessions
        .trickplay;
    let rules = passes.addon_rules();
    // The rows are per title, not per user: an addon this user is not asked
    // for is not read for them either.
    let mut rows: Vec<db::Media> = stored
        .iter()
        .filter(|row| in_scope(&rules, row.addon_id, limited_to.as_deref()))
        .map(|row| row.to_stream_row(title.runtime))
        .collect();
    let has_own_folder = |id: Uuid| set_dir(&ask.root, id).is_dir();
    let set = set_id(&ask.playing);
    let now = Instant::now();
    let unmeasured: Vec<Uuid> =
        preview_unmeasured(&rows, title.runtime, &rules, has_own_folder)
            .into_iter()
            .filter(|row| !passes.failed_recently(row.id, now))
            .map(|row| row.id)
            .collect();
    if !unmeasured.is_empty()
        && preview_versions(&ask.playing, &rows, title.runtime, &rules, has_own_folder)
            .is_empty()
    {
        let cfg = db::Settings::get_config_or_default(&ctx.db).await;
        let wait = Duration::from_secs(
            cfg.probe_timeout_secs
                .and_then(|s| u64::try_from(s).ok())
                .unwrap_or(20),
        );
        for id in unmeasured
            .into_iter()
            .take(MAX_PREVIEW_MEASURES)
        {
            let Some(row) = rows
                .iter_mut()
                .find(|row| row.id == id)
            else {
                continue;
            };
            measure_preview_source(ctx, row, wait).await;
            if set_id(row) == set {
                break;
            }
        }
    }
    let port = ctx
        .config
        .port;
    let sources =
        preview_versions(&ask.playing, &rows, title.runtime, &rules, has_own_folder)
            .into_iter()
            .filter_map(|row| {
                let info = row
                    .stream_info
                    .as_ref()?;
                Some(PassSource {
                    stream_id: row.id,
                    input: pass_input(info, row.id, port),
                    info: info.clone(),
                    kind: SourceKind::Preview,
                })
            })
            .collect();
    Found {
        sources,
        other_lengths: other_lengths(
            &ask.playing,
            &rows,
            title.runtime,
            &rules,
            has_own_folder,
        ),
    }
}

/// Measures a stored preview source the answer gave no length for: one
/// ffprobe of its proxy link, within the server's probe timeout, saved on its
/// stored row (never on a media row). A read that fails, or finds another
/// file than the one listed, leaves it unread for [`OTHER_FAILURE_WAIT`], so
/// a row is measured once.
pub(super) async fn measure_preview_source(
    ctx: &AppContext,
    row: &mut db::Media,
    wait: Duration,
) {
    let Some(info) = row
        .stream_info
        .clone()
    else {
        return;
    };
    let input = pass_input(
        &info,
        row.id,
        ctx.config
            .port,
    );
    let started = Instant::now();
    let probed = tokio::time::timeout(
        wait,
        tokio::task::spawn_blocking(move || {
            crate::playback::probe::probe_media_with(&input, None, Some(wait))
        }),
    )
    .await;
    let passes = &ctx
        .sessions
        .trickplay;
    match probed {
        Ok(Ok(Ok((probe, _))))
            if probe
                .run_time_ticks
                .is_some_and(|t| t > 0)
                && !info.is_other_file(probe.size) =>
        {
            if let Err(error) =
                db::PreviewSource::save_probe_data(&ctx.db, row.id, &probe).await
            {
                warn!(%error, "trick play: could not save a preview source's length");
            }
            info!(
                stream_id = %row.id,
                elapsed = ?started.elapsed(),
                "trick play: measured a preview source"
            );
            row.probe_data = Some(probe);
        }
        _ => {
            info!(
                stream_id = %row.id,
                elapsed = ?started.elapsed(),
                "trick play: could not measure a preview source; it is not read"
            );
            passes.mark_failed(
                row.id,
                Instant::now()
                    + passes
                        .inner
                        .limits
                        .other_failure_wait,
            );
        }
    }
}

/// The streams one answer of an addon kept for seek previews only gives that
/// are worth keeping, at most [`MAX_PREVIEW_KEPT`], in the addon's order:
/// each passes the row checks for `title`, is held where it can be read in
/// place of another (a file on this server or a library addon's WebDAV
/// server, or a link a named service marks as cached), and lists its size
/// when a service holds it. Torrents, uncached links and links with no named
/// service are never kept. Each carries its content key, as the title's own
/// stream rows do.
pub(crate) fn kept_preview_streams(
    title: &db::Media,
    answer: Vec<StreamInfo>,
) -> Vec<db::NewPreviewSource> {
    let mut seen = HashSet::new();
    answer
        .into_iter()
        .enumerate()
        .filter_map(|(idx, info)| {
            let row = db::Media {
                kind: db::MediaKind::Stream,
                parent_id: Some(title.id),
                probe_data: info
                    .probe_data
                    .clone(),
                stream_info: Some(info.clone()),
                ..Default::default()
            };
            eligible_row(&row, &info, runtime_s(&row, title.runtime)).ok()?;
            let held = held(&info)?;
            if held >= Held::Cached
                && info
                    .size
                    .filter(|s| *s > 0)
                    .is_none()
            {
                return None;
            }
            let content_key = crate::addons::AddonService::stream_dedup_key(&row)
                .unwrap_or_else(|| format!("source_{idx}"));
            if !seen.insert(content_key.clone()) {
                return None;
            }
            Some(db::NewPreviewSource {
                content_key,
                probe_data: row.probe_data,
                stream_info: info,
            })
        })
        .take(MAX_PREVIEW_KEPT)
        .collect()
}

/// Whether a title's item may list a version its addon says not to read,
/// because an addon kept for seek previews only may give a file in its place:
/// one is enabled, and the title's last asks did not all keep nothing (see
/// [`TrickplayPasses::kept_nothing`]). Read from memory, no query.
pub fn preview_source_may_help(
    rules: &AddonRules,
    passes: &TrickplayPasses,
    title_id: Uuid,
) -> bool {
    has_preview_source(rules) && !passes.kept_nothing(title_id)
}
