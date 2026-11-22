//! Resolving a play to the file a pass reads, and the sources it may read.

use super::*;

/// What a play-start report hands to [`start_for_play`].
#[derive(Debug, Clone)]
pub struct PlayStart {
    /// The id of the play session as stored, never a client's missing one.
    pub play_session_id: String,
    pub item_id: Uuid,
    pub media_source_id: Option<Uuid>,
    pub device_id: String,
    pub user_id: Uuid,
}

/// Registers the play at once, then resolves its file and queues a pass in
/// the background. Call only with the switch on.
pub fn start_for_play(ctx: &AppContext, play: PlayStart) {
    ctx.sessions
        .trickplay
        .register(&play.play_session_id, play.item_id, &play.device_id);
    let ctx = ctx.clone();
    tokio::spawn(async move {
        prepare(ctx, play).await;
    });
}

pub(super) async fn prepare(ctx: AppContext, play: PlayStart) {
    let passes = ctx
        .sessions
        .trickplay
        .clone();
    let plan = match resolve_plan(&ctx, &play).await {
        Ok(plan) => plan,
        Err(reason) => {
            debug!(
                item_id = %play.item_id,
                %reason,
                "no trick play pictures for this play"
            );
            passes.release(&play.play_session_id);
            return;
        }
    };
    let sessions = ctx
        .sessions
        .clone();
    let finder = AppFinder { ctx: ctx.clone() };
    passes
        .begin_with(
            &play.play_session_id,
            plan,
            Arc::new(FfmpegRunner),
            move |device_id: &str, item_id: Uuid| {
                watched_from(sessions.get_by_device(device_id), item_id)
            },
            &finder,
        )
        .await;
}

/// Whether a device's latest reported session is on `item_id`, and if so
/// which play it is, how long ago it last reported and where it was. Progress
/// reports and pings both count as a report.
pub(super) fn watched_from(
    latest: Option<crate::playback_session::PlaybackSession>,
    item_id: Uuid,
) -> Option<Watched> {
    let session = latest.filter(|s| s.item_id == item_id)?;
    let quiet = (chrono::Utc::now() - session.last_activity)
        .to_std()
        .unwrap_or_default();
    let position_s = u64::try_from(session.position_ticks / TICKS_PER_SEC).unwrap_or(0);
    Some(Watched {
        quiet,
        position_s,
        play_session_id: session.play_session_id,
    })
}

/// Finds the file a play reads the way the stream route does, and checks it.
pub(super) async fn resolve_plan(
    ctx: &AppContext,
    play: &PlayStart,
) -> Result<PassPlan, Ineligible> {
    let (owner, source) = StreamService::version_target(
        ctx,
        Some(UserId(play.user_id)),
        play.item_id,
        play.media_source_id,
    )
    .await;
    // The item is judged before any stream is looked for, so a play of
    // music never reaches a lookup. A missing row is judged by its stream.
    let item = db::Media::get_by_id(&ctx.db, &owner)
        .await
        .ok()
        .flatten();
    if let Some(item) = &item {
        if item.is_live() {
            return Err(Ineligible::LiveTv);
        }
        if !kind_has_pictures(&item.kind) {
            return Err(Ineligible::NotVideo);
        }
    }
    let selected = StreamService::selected_stream_id(
        ctx,
        Some(
            play.device_id
                .as_str(),
        ),
        owner,
        source,
        Some(
            play.play_session_id
                .as_str(),
        ),
    )
    // A client that sent no play session id was given one by the session
    // start, which PlaybackInfo never saw: follow the device's pick, as the
    // stream route does without one.
    .or_else(|| {
        StreamService::selected_stream_id(
            ctx,
            Some(
                play.device_id
                    .as_str(),
            ),
            owner,
            source,
            None,
        )
    });
    let requested = selected.or(source);
    let stream = match stored_stream(ctx, requested.unwrap_or(owner)).await {
        Some(stream) => stream,
        None => StreamService::lookup(
            ctx,
            owner,
            requested,
            Some(
                play.device_id
                    .as_str(),
            ),
            Some(play.user_id),
        )
        .await
        .map_err(|_| Ineligible::NoStream)?,
    };
    // An addon kept for seek previews only is asked only for a measured
    // version, and the length of this one may be read from its header in the
    // play's first seconds.
    let passes = &ctx
        .sessions
        .trickplay;
    let (stream, delay_spent) = if wants_length(&stream)
        && has_preview_source(&passes.addon_rules())
    {
        passes
            .wait_for_length(&play.play_session_id, stream, |id| stored_stream(ctx, id))
            .await
    } else {
        (stream, Duration::ZERO)
    };
    let info = stream
        .stream_info
        .as_ref()
        .ok_or(Ineligible::NoStream)?;
    let runtime = runtime_s(
        &stream,
        item.as_ref()
            .and_then(|i| i.runtime),
    );
    eligible_row(&stream, info, runtime)?;
    let addon = match info.addon_id {
        Some(id) => addon_from_read(Addon::get(&ctx.db, id).await)?,
        None => None,
    };
    // A deleted addon's leftover streams are not read: off until turned on.
    let refused = if info
        .addon_id
        .is_some()
        && addon.is_none()
    {
        Some(Ineligible::AddonExcluded)
    } else {
        eligible_pass(addon.as_ref()).err()
    };
    let runtime_s = runtime.ok_or(Ineligible::NoRuntime)?;
    let cfg = db::Settings::get_config_or_default(&ctx.db).await;
    let root = root_dir(
        &ctx.config
            .data_dir,
    );
    // Counted from the same length a media source lists: the probed ticks,
    // else the runtime in whole seconds.
    let ticks = stream
        .probe_data
        .as_ref()
        .and_then(|p| p.run_time_ticks)
        .filter(|t| *t > 0)
        .unwrap_or_else(|| {
            i64::try_from(runtime_s)
                .unwrap_or(i64::MAX)
                .saturating_mul(TICKS_PER_SEC)
        });
    let pictures = picture_count(ticks);
    let set_id = set_for(&root, &stream);
    let rules = ctx
        .sessions
        .trickplay
        .addon_rules();
    let (others, other_lengths) = if set_id != stream.id {
        other_sources(ctx, &root, &stream, play.user_id, &rules).await
    } else {
        (Vec::new(), 0)
    };
    let preview = preview_ask(&stream, set_id, &root, play.user_id, &rules);
    let plan = PassPlan {
        stream_id: stream.id,
        set_id,
        input: pass_input(
            info,
            stream.id,
            ctx.config
                .port,
        ),
        root,
        pictures,
        size: info
            .size
            .and_then(|s| u64::try_from(s).ok())
            .filter(|s| *s > 0),
        estimate: estimate_bytes(runtime_s),
        played: info.clone(),
        from_position: start_at_position(&cfg),
        db: Some(
            ctx.db
                .clone(),
        ),
        others,
        other_version: None,
        preview,
        played_refused: None,
        other_lengths,
        delay_spent,
    };
    if plan
        .preview
        .is_some()
    {
        // Whether the version played stands behind a preview source or is
        // replaced by another version is settled once they were asked.
        return Ok(PassPlan {
            played_refused: refused,
            ..plan
        });
    }
    read_other_if_refused(plan, refused)
}

/// What asking the addons kept for seek previews only needs for a play (see
/// [`PreviewFinder`]), or `None` when they are not to be asked: none is
/// enabled, or the version played has no measured length (so no other file
/// can be known to give its pictures), or it is not a stream row under a
/// title, or it keeps a folder of its own.
pub(super) fn preview_ask(
    playing: &db::Media,
    set_id: Uuid,
    root: &Path,
    user_id: Uuid,
    rules: &AddonRules,
) -> Option<PreviewAsk> {
    let title_id = playing.parent_id?;
    let wanted = has_preview_source(rules)
        && set_id != playing.id
        && measured_ticks(playing).is_some();
    wanted.then(|| PreviewAsk {
        title_id,
        playing: playing.clone(),
        user_id,
        root: root.to_path_buf(),
    })
}

/// The addon a stream names, from its read: an error refuses the pass, since
/// that addon's switches are then not known. A missing row (the addon was
/// deleted) is no addon; the caller refuses that stream.
pub(super) fn addon_from_read(
    read: anyhow::Result<Option<Addon>>,
) -> Result<Option<Addon>, Ineligible> {
    match read {
        Ok(addon) => Ok(addon),
        Err(error) => {
            warn!(%error, "trick play: could not read the stream's addon");
            Err(Ineligible::AddonReadFailed)
        }
    }
}

/// `plan` as it reads the version played, or, when its addon says not to read
/// it (`refused`), as it reads the first of its other versions instead (see
/// [`read_other_instead`]).
pub(super) fn read_other_if_refused(
    mut plan: PassPlan,
    refused: Option<Ineligible>,
) -> Result<PassPlan, Ineligible> {
    if let Some(reason) = refused {
        read_other_instead(&mut plan, reason)?;
    }
    Ok(plan)
}

/// Moves `plan`, whose version played its addon says not to read (`reason`),
/// to the first of its other sources: that version is never opened. With no
/// other source there is no pass, and when the only streams there were give
/// another number of pictures, one line says so.
pub(super) fn read_other_instead(
    plan: &mut PassPlan,
    reason: Ineligible,
) -> Result<(), Ineligible> {
    if plan
        .others
        .is_empty()
    {
        if plan.other_lengths > 0 {
            info!(
                set_id = %plan.set_id,
                stream_id = %plan.stream_id,
                %reason,
                other_lengths = plan.other_lengths,
                "trick play: no previews for this play: no stream of matching length was found"
            );
        }
        return Err(reason);
    }
    let first = plan
        .others
        .remove(0);
    info!(
        set_id = %plan.set_id,
        stream_id = %first.stream_id,
        %reason,
        "trick play: this version is not read; reading another version of this title for its pictures"
    );
    plan.read_other(first);
    Ok(())
}

/// Puts the streams the addons kept for seek previews only gave (`found`)
/// in front of `plan`'s other sources, at most [`MAX_PREVIEW_READS`], with
/// the version played behind them when its addon allows. True when `plan`
/// is then to move on to the first of them (see
/// [`TrickplayPasses::next_source`]). With none found, `plan` reads as it
/// would have without them: the version played, or, when its addon refused
/// it, the first of its other versions, or nothing.
pub(super) fn put_preview_sources(
    plan: &mut PassPlan,
    found: Found,
) -> Result<bool, Ineligible> {
    let refused = plan
        .played_refused
        .take();
    plan.other_lengths += found.other_lengths;
    let mut order: Vec<PassSource> = found
        .sources
        .into_iter()
        .take(MAX_PREVIEW_READS)
        .collect();
    if order.is_empty() {
        return match refused {
            Some(reason) => read_other_instead(plan, reason).map(|()| false),
            None => Ok(false),
        };
    }
    if refused.is_none() {
        order.push(PassSource {
            stream_id: plan.stream_id,
            input: plan
                .input
                .clone(),
            info: plan
                .played
                .clone(),
            kind: SourceKind::Played,
        });
    }
    order.append(&mut plan.others);
    plan.others = order;
    Ok(true)
}

/// [`other_versions`] of a stored stream, from its title's stream rows as
/// last listed, ready to be read, and how many rows would have stood in but
/// for their length (see [`other_lengths`]). A user limited to some addons is
/// never read another addon's file for.
pub(super) async fn other_sources(
    ctx: &AppContext,
    root: &Path,
    playing: &db::Media,
    user_id: Uuid,
    rules: &AddonRules,
) -> (Vec<PassSource>, usize) {
    let Some(parent) = playing.parent_id else {
        return (Vec::new(), 0);
    };
    let Ok(limited_to) = crate::addons::user_addon_override(&ctx.db, user_id).await
    else {
        return (Vec::new(), 0);
    };
    let Some(mut title) = db::Media::get_by_id(&ctx.db, &parent)
        .await
        .ok()
        .flatten()
    else {
        return (Vec::new(), 0);
    };
    let versions = match title
        .streams(&ctx.db)
        .await
    {
        Ok(versions) => versions,
        Err(error) => {
            warn!(%error, "trick play: could not list the other versions");
            return (Vec::new(), 0);
        }
    };
    let has_own_folder = |id: Uuid| set_dir(root, id).is_dir();
    let lengths =
        other_lengths(playing, &versions, title.runtime, rules, has_own_folder);
    let sources =
        other_versions(playing, &versions, title.runtime, rules, has_own_folder)
            .into_iter()
            .filter_map(|row| {
                let info = row
                    .stream_info
                    .as_ref()?;
                if let Some(ids) = &limited_to
                    && !info
                        .addon_id
                        .is_some_and(|id| ids.contains(&id))
                {
                    return None;
                }
                Some(PassSource {
                    stream_id: row.id,
                    input: pass_input(
                        info,
                        row.id,
                        ctx.config
                            .port,
                    ),
                    info: info.clone(),
                    kind: SourceKind::Other,
                })
            })
            .collect();
    (sources, lengths)
}

/// What a source of a pass is to the play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceKind {
    /// The version played, behind the preview sources: it waits
    /// [`FAILURE_WAIT`] after a failure, like any read of it.
    Played,
    /// Another version of the title (see [`other_versions`]).
    Other,
    /// A stream of an addon kept for seek previews only (see
    /// [`preview_versions`]).
    Preview,
}

/// A source a pass may read in place of the one it reads.
#[derive(Debug, Clone)]
pub(crate) struct PassSource {
    pub stream_id: Uuid,
    pub input: String,
    /// Its row as listed: its addon, and its size for the size check.
    pub info: StreamInfo,
    pub kind: SourceKind,
}

/// Everything a queued pass needs.
#[derive(Debug, Clone)]
pub(crate) struct PassPlan {
    /// The stream read, for its proxy input, the logs and the failure wait:
    /// the version played, or another source (see `other_version`).
    pub stream_id: Uuid,
    /// The set its pictures belong to (see [`set_for`]).
    pub set_id: Uuid,
    pub input: String,
    pub root: PathBuf,
    /// The picture count the set is keyed by.
    pub pictures: u64,
    /// The file's size in bytes, when the source listed it.
    pub size: Option<u64>,
    /// The disk this set is expected to take, kept free of other sets. The
    /// cap and the keep time themselves are read when the pass takes the
    /// slot, so a save after the play started counts.
    pub estimate: u64,
    /// The listing of the version played: its addon and service, checked
    /// against the addon rules as they are at slot entry and while it is
    /// read (see [`addon_ok`]).
    pub played: StreamInfo,
    /// Start at the viewer's sheet and follow seeks (see
    /// [`start_at_position`]).
    pub from_position: bool,
    /// Where the settings are read again when the pass takes the slot, since
    /// a queued pass may wait long after its play started, and again while it
    /// runs, so a switch turned off stops it. With `None` the plan's own
    /// settings stand and nothing is pruned.
    pub db: Option<sqlx::SqlitePool>,
    /// Other sources still to try, in order, when the stream read cannot be:
    /// preview sources, the version played, other versions (see
    /// [`put_preview_sources`] and [`other_versions`]).
    pub others: Vec<PassSource>,
    /// The listing of the stream in `stream_id` while it is a source read in
    /// place of the one played; `None` while that one is read.
    pub other_version: Option<StreamInfo>,
    /// What asking the addons kept for seek previews only needs, until they
    /// are asked (see [`TrickplayPasses::begin_with`]); `None` when they are
    /// not to be asked.
    pub preview: Option<PreviewAsk>,
    /// Why the version played may not be read, kept while the addons kept
    /// for seek previews only are still to be asked.
    pub played_refused: Option<Ineligible>,
    /// Streams that would have stood in but for their length (see
    /// [`other_lengths`]), for the log when nothing can be read.
    pub other_lengths: usize,
    /// How much of the start delay was spent before the plan was settled,
    /// waiting for the length of the version played (see
    /// [`TrickplayPasses::wait_for_length`]).
    pub delay_spent: Duration,
}

impl PassPlan {
    /// Reads `source` from now on: another version, a preview source, or the
    /// version played again behind the preview sources. The set and its
    /// picture count stay the played version's.
    pub(super) fn read_other(&mut self, source: PassSource) {
        self.stream_id = source.stream_id;
        self.input = source.input;
        self.size = source
            .info
            .size
            .and_then(|s| u64::try_from(s).ok())
            .filter(|s| *s > 0);
        self.other_version = (source.kind != SourceKind::Played).then_some(source.info);
    }

    /// Reads what the plan of a play that joined this pass reads, from now
    /// on (see [`TrickplayPasses::next_joined`]): that play's version, or the
    /// version standing in for it. The set and its picture count are the
    /// same, since that play joined this set.
    pub(super) fn read_joined(&mut self, joined: PassPlan) {
        self.stream_id = joined.stream_id;
        self.input = joined.input;
        self.size = joined.size;
        self.played = joined.played;
        self.other_version = joined.other_version;
    }
}
