//! The passes in memory: plays, sets, the slot, and what was asked or failed.

use super::*;

/// Refills the advertising snapshot from the stored addons. Called at startup
/// and whenever an addon is created, changed or deleted.
pub async fn refresh_addon_rules(ctx: &AppContext) {
    match Addon::list(&ctx.db).await {
        Ok(addons) => ctx
            .sessions
            .trickplay
            .set_addon_rules(&addons),
        Err(error) => warn!(%error, "trick play: could not read the addons"),
    }
}

/// [`prune_on_start`] with `cfg`, the settings start-up has already read
/// strictly: a database error read here as "off" would delete every set. It
/// holds the pass slot while it works, so it never deletes from a set a pass
/// is writing, however long it takes.
pub async fn prune_at_startup(ctx: &AppContext, cfg: &api::ServerConfiguration) {
    let root = root_dir(
        &ctx.config
            .data_dir,
    );
    let on = enabled(cfg);
    let cap = cap_bytes(cfg);
    let keep = keep_for(cfg);
    let _slot = ctx
        .sessions
        .trickplay
        .inner
        .slots
        .acquire()
        .await;
    let _ = tokio::task::spawn_blocking(move || {
        prune_on_start(&root, on, cap, keep, SystemTime::now())
    })
    .await;
}

/// Durations and limits, fixed in production and shortened in tests.
#[derive(Debug, Clone, Copy)]
pub(super) struct Limits {
    pub(super) grace: Duration,
    pub(super) start_delay: Duration,
    pub(super) length_check: Duration,
    pub(super) failure_wait: Duration,
    pub(super) other_failure_wait: Duration,
    pub(super) min_free_bytes: u64,
    pub(super) watch_check: Duration,
    pub(super) quiet_waiting: Duration,
    pub(super) quiet_alone: Duration,
    pub(super) seek_wait: Duration,
    pub(super) no_jump_wait: Duration,
    pub(super) stall_wait: Duration,
    pub(super) follow_every: Duration,
    pub(super) max_follows: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            grace: GRACE,
            start_delay: START_DELAY,
            length_check: LENGTH_CHECK,
            failure_wait: FAILURE_WAIT,
            other_failure_wait: OTHER_FAILURE_WAIT,
            min_free_bytes: MIN_FREE_BYTES,
            watch_check: WATCH_CHECK,
            quiet_waiting: QUIET_WAITING,
            quiet_alone: QUIET_ALONE,
            seek_wait: SEEK_WAIT,
            no_jump_wait: NO_JUMP_WAIT,
            stall_wait: STALL_WAIT,
            follow_every: FOLLOW_EVERY,
            max_follows: MAX_FOLLOWS,
        }
    }
}

/// What a play's device last said about it: how long ago it reported, the
/// position it reported, and the play session that reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Watched {
    pub quiet: Duration,
    pub position_s: u64,
    pub play_session_id: String,
}

/// One play session registered for pictures.
pub(super) struct Registration {
    pub(super) item_id: Uuid,
    pub(super) device_id: String,
    /// `None` until the file is resolved.
    pub(super) set_id: Option<Uuid>,
    /// When the play registered.
    pub(super) at: Instant,
}

/// The pass of one set, shared by every play of it.
#[derive(Default)]
pub(super) struct Pass {
    pub(super) sessions: HashSet<String>,
    /// Present while a pass is queued or running; resolving it stops that.
    pub(super) cancel: Option<oneshot::Sender<()>>,
    pub(super) job: Option<JoinHandle<()>>,
    pub(super) job_id: u64,
    pub(super) grace: Option<JoinHandle<()>>,
    /// The running child's pid, 0 when none.
    pub(super) pid: Arc<AtomicU32>,
    /// Its job is waiting for the slot on behalf of a watched play.
    pub(super) waiting: bool,
    /// The picture count the set is keyed by, once a play of it is resolved;
    /// `None` for a stream's own set.
    pub(super) pictures: Option<u64>,
    /// The play session id and plan of each play that joined the running
    /// job, by the stream it would read: its own version, or the one standing
    /// in for it. The job reads these when its own source fails (see
    /// [`TrickplayPasses::next_joined`]). A play's entry goes when it leaves
    /// the set (see [`TrickplayPasses::leave`]), so no source is read for a
    /// play that ended.
    pub(super) joined: Vec<(String, PassPlan)>,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) by_session: HashMap<String, Registration>,
    pub(super) by_set: HashMap<Uuid, Pass>,
    /// (item id, device id) to the set whose pictures that device gets.
    pub(super) selection: HashMap<(Uuid, String), Uuid>,
    /// When each stream that failed may be read again. By stream, not set: a
    /// dead link on one version does not hold back another version of the
    /// same set, which a play that joined the pass may be playing. Kept in
    /// memory only.
    pub(super) failed_until: HashMap<Uuid, Instant>,
    /// Until when each stream whose source gave no sheet in time after a jump
    /// is read from its start ([`NO_JUMP_WAIT`]). By stream, as
    /// `failed_until`.
    pub(super) no_jump: HashMap<Uuid, Instant>,
    pub(super) touched_at: HashMap<Uuid, Instant>,
    pub(super) next_job: u64,
    /// The last ask of each addon kept for seek previews only for each
    /// title, by (title id, addon id): until when it is not asked again and
    /// how many streams it kept. Pruned as it is written. Kept in memory
    /// only: after a restart a title whose answer was empty or failed can be
    /// asked once more, while one whose answer is stored is not (see
    /// [`db::PreviewSource::has_fresh`]).
    pub(super) asked: HashMap<(Uuid, Uuid), Asked>,
}

/// One ask of an addon kept for seek previews only (see [`State::asked`]).
#[derive(Debug, Clone, Copy)]
pub(super) struct Asked {
    pub(super) until: Instant,
    pub(super) kept: usize,
}

pub(super) struct Inner {
    pub(super) state: Mutex<State>,
    pub(super) slots: tokio::sync::Semaphore,
    pub(super) limits: Limits,
    /// See [`AddonRule`].
    pub(super) addon_rules: Mutex<Arc<AddonRules>>,
}

/// What [`TrickplayPasses::begin_with`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Begin {
    /// The play ended, or was never registered: nothing queued.
    Released,
    /// The set is already complete.
    Done,
    /// The file read failed within its wait ([`FAILURE_WAIT`], or
    /// [`OTHER_FAILURE_WAIT`] for another version), and no other version can
    /// be read in its place.
    FailedRecently,
    /// The version played may not be read, and nothing can be read in its
    /// place.
    NoSource,
    /// A new pass was queued.
    Queued,
    /// The play joined a pass already queued or running. The stream its plan
    /// reads is kept, for the pass to read should its own source fail.
    Joined,
}

/// How a job's wait for the slot ended.
pub(super) enum Turn<P> {
    Slot(P),
    Cancelled,
    Unwatched,
}

/// Why a run was stopped from outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stopped {
    Cancelled,
    StoodDown,
    /// The viewer moved to this sheet, which has no picture yet.
    Moved(u32),
    /// No new sheet for [`STALL_WAIT`]: the source stopped sending.
    Stalled,
    /// The settings or the addon rules no longer let the stream be read (see
    /// [`TrickplayPasses::refusal`]); the slot ends this way, a stream
    /// not to be read as `Failed` (see [`refused_after_open`]).
    Refused(SlotEnd),
}

/// How a job's turn in the slot ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SlotEnd {
    /// The pass ran to its end or had nothing to do.
    Over,
    Cancelled,
    /// Nobody has watched it lately: it waits to be watched again.
    StoodDown,
    /// Its source was opened and could not be read, or may no longer be (see
    /// [`refused_after_open`]).
    Failed,
    /// Its source failed recently and was not opened.
    Skipped,
}

/// The passes of this server, held by the playback session manager.
#[derive(Clone)]
pub struct TrickplayPasses {
    pub(super) inner: Arc<Inner>,
}

impl Default for TrickplayPasses {
    fn default() -> Self {
        Self::new()
    }
}

impl TrickplayPasses {
    pub fn new() -> Self {
        Self::with_limits(Limits::default())
    }

    pub(super) fn with_limits(limits: Limits) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State::default()),
                slots: tokio::sync::Semaphore::new(MAX_PASSES),
                limits,
                addon_rules: Mutex::new(Arc::default()),
            }),
        }
    }

    pub(super) fn lock(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Replaces the advertising snapshot with one rule per addon in `addons`.
    pub fn set_addon_rules(&self, addons: &[Addon]) {
        let rules: AddonRules = addons
            .iter()
            .map(|addon| (addon.id, AddonRule::new(addon)))
            .collect();
        *self
            .inner
            .addon_rules
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Arc::new(rules);
    }

    /// The advertising snapshot: one [`AddonRule`] per stored addon.
    pub fn addon_rules(&self) -> Arc<AddonRules> {
        self.inner
            .addon_rules
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Puts one addon's rule in the snapshot, from its row as just saved, so
    /// a reload or a refill that fails afterwards cannot leave it stale.
    pub fn set_addon_rule(&self, addon: &Addon) {
        let mut rules = self
            .inner
            .addon_rules
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        Arc::make_mut(&mut *rules).insert(addon.id, AddonRule::new(addon));
    }

    /// Takes a deleted addon's rule out of the snapshot.
    pub fn drop_addon_rule(&self, addon_id: Uuid) {
        let mut rules = self
            .inner
            .addon_rules
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        Arc::make_mut(&mut *rules).remove(&addon_id);
    }

    /// Why the stream `plan` reads may not be read now, by the settings `cfg`
    /// and the addon rules as they are now; `None` while it may. The stream
    /// read (the version played, another version or a preview source)
    /// follows [`addon_ok`], so a stream with no addon stays allowed and one
    /// whose addon was deleted does not. The main switch is judged only with
    /// `cfg`: `None` where the settings were not read, so a failed settings
    /// read stops nothing.
    pub(super) fn refusal(
        &self,
        plan: &PassPlan,
        cfg: Option<&api::ServerConfiguration>,
    ) -> Option<SlotEnd> {
        if let Some(cfg) = cfg
            && !enabled(cfg)
        {
            info!(
                set_id = %plan.set_id,
                stream_id = %plan.stream_id,
                "trick play: switched off; not reading"
            );
            return Some(SlotEnd::Over);
        }
        let read = plan
            .other_version
            .as_ref()
            .unwrap_or(&plan.played);
        if !addon_ok(&self.addon_rules(), read) {
            let message = if plan
                .other_version
                .is_some()
            {
                "trick play: another version's addon no longer allows previews"
            } else {
                "trick play: this version's addon no longer allows previews"
            };
            info!(set_id = %plan.set_id, stream_id = %plan.stream_id, "{message}");
            return Some(SlotEnd::Skipped);
        }
        None
    }

    /// [`Self::refusal`] with the settings read now. A failed read judges by
    /// the addon rules alone, so a passing database error stops nothing.
    pub(super) async fn refused_now(&self, plan: &PassPlan) -> Option<SlotEnd> {
        let cfg = match &plan.db {
            Some(pool) => db::Settings::get_config(pool)
                .await
                .ok(),
            None => None,
        };
        self.refusal(plan, cfg.as_ref())
    }

    /// Records a play session, before its file is known. The set an earlier
    /// play of the same item on the same device was given is forgotten, so
    /// the sheet route follows this play's pick, which may be a version of
    /// another length, until this play is resolved.
    pub fn register(&self, play_session_id: &str, item_id: Uuid, device_id: &str) {
        // A play session id reused for another item starts over.
        let other_item = self
            .lock()
            .by_session
            .get(play_session_id)
            .is_some_and(|reg| reg.item_id != item_id);
        if other_item {
            self.release(play_session_id);
        }
        let mut st = self.lock();
        st.selection
            .remove(&(item_id, device_id.to_string()));
        let reg = st
            .by_session
            .entry(play_session_id.to_string())
            .or_insert_with(|| Registration {
                item_id,
                device_id: device_id.to_string(),
                set_id: None,
                at: Instant::now(),
            });
        reg.item_id = item_id;
        reg.device_id = device_id.to_string();
    }

    /// The device and item of a registered play session.
    pub(super) fn play_of(&self, play_session_id: &str) -> Option<(String, Uuid)> {
        self.lock()
            .by_session
            .get(play_session_id)
            .map(|reg| {
                (
                    reg.device_id
                        .clone(),
                    reg.item_id,
                )
            })
    }

    /// Ties a registered play session to its set and records that set for
    /// the session's item and device. False when the session was released
    /// before its file was resolved.
    pub fn attach(&self, play_session_id: &str, set_id: Uuid) -> bool {
        let mut st = self.lock();
        let (old, key) = match st
            .by_session
            .get_mut(play_session_id)
        {
            Some(reg) => (
                reg.set_id
                    .replace(set_id),
                (
                    reg.item_id,
                    reg.device_id
                        .clone(),
                ),
            ),
            None => return false,
        };
        if let Some(old) = old.filter(|old| *old != set_id) {
            self.leave(&mut st, play_session_id, old);
        }
        st.selection
            .insert(key, set_id);
        let pass = st
            .by_set
            .entry(set_id)
            .or_default();
        pass.sessions
            .insert(play_session_id.to_string());
        if let Some(grace) = pass
            .grace
            .take()
        {
            grace.abort();
        }
        true
    }

    /// Ends a play session's part. When it was the last play of its set, the
    /// pass stops after the grace period unless another play joins. The set
    /// it recorded for its device goes with it, so a quick switch to a version
    /// of another length is never answered from this one.
    pub fn release(&self, play_session_id: &str) {
        let mut st = self.lock();
        let Some(reg) = st
            .by_session
            .remove(play_session_id)
        else {
            return;
        };
        if let Some(set_id) = reg.set_id {
            // A later play of the device may have recorded its own set since,
            // or still be on this one.
            let others = st
                .by_session
                .values()
                .any(|r| {
                    r.item_id == reg.item_id
                        && r.device_id == reg.device_id
                        && r.set_id == Some(set_id)
                });
            let key = (reg.item_id, reg.device_id);
            if !others
                && st
                    .selection
                    .get(&key)
                    == Some(&set_id)
            {
                st.selection
                    .remove(&key);
            }
            self.leave(&mut st, play_session_id, set_id);
        }
    }

    pub(super) fn leave(&self, st: &mut State, play_session_id: &str, set_id: Uuid) {
        let Some(pass) = st
            .by_set
            .get_mut(&set_id)
        else {
            return;
        };
        pass.sessions
            .remove(play_session_id);
        pass.joined
            .retain(|(session, _)| session != play_session_id);
        if pass
            .sessions
            .is_empty()
            && pass
                .grace
                .is_none()
        {
            let passes = self.clone();
            let grace = self
                .inner
                .limits
                .grace;
            pass.grace = Some(tokio::spawn(async move {
                tokio::time::sleep(grace).await;
                passes.expire(set_id);
            }));
        }
    }

    /// The grace period ended: drops the pass if nobody rejoined, which
    /// cancels it.
    pub(super) fn expire(&self, set_id: Uuid) {
        let pass = {
            let mut st = self.lock();
            let unused = st
                .by_set
                .get(&set_id)
                .is_some_and(|p| {
                    p.sessions
                        .is_empty()
                });
            if !unused {
                return;
            }
            st.selection
                .retain(|_, s| *s != set_id);
            st.by_set
                .remove(&set_id)
        };
        if let Some(mut pass) = pass
            && let Some(cancel) = pass
                .cancel
                .take()
        {
            let _ = cancel.send(());
        }
    }

    /// The set whose pictures this device gets for this item, recorded when
    /// its play was resolved.
    pub fn selected_set(&self, item_id: Uuid, device_id: &str) -> Option<Uuid> {
        self.lock()
            .selection
            .get(&(item_id, device_id.to_string()))
            .copied()
    }

    /// The picture count of a set that has a pass, so the sheet route can
    /// tell a sheet past it from one not made yet.
    pub fn pictures_of(&self, set_id: Uuid) -> Option<u64> {
        self.lock()
            .by_set
            .get(&set_id)
            .and_then(|pass| pass.pictures)
    }

    /// Notes that a set was just shown, so the cap and the keep time delete
    /// it last. Updates the directory's time at most once a minute.
    pub fn mark_used(&self, set_id: Uuid, dir: &Path) {
        let now = Instant::now();
        {
            let mut st = self.lock();
            st.touched_at
                .retain(|_, at| now.saturating_duration_since(*at) < TOUCH_EVERY);
            if st
                .touched_at
                .contains_key(&set_id)
            {
                return;
            }
            st.touched_at
                .insert(set_id, now);
        }
        if let Ok(file) = std::fs::File::open(dir) {
            let _ = file.set_modified(SystemTime::now());
        }
    }

    /// Records that a stream is not to be read again before `until`.
    pub(super) fn mark_failed(&self, stream_id: Uuid, until: Instant) {
        self.lock()
            .failed_until
            .insert(stream_id, until);
    }

    /// Records that the stream `plan` reads could not be read: the version
    /// played waits [`FAILURE_WAIT`], another version [`OTHER_FAILURE_WAIT`].
    pub(super) fn mark_plan_failed(&self, plan: &PassPlan) {
        let limits = self
            .inner
            .limits;
        let wait = if plan
            .other_version
            .is_some()
        {
            limits.other_failure_wait
        } else {
            limits.failure_wait
        };
        self.mark_failed(plan.stream_id, Instant::now() + wait);
    }

    pub(super) fn failed_recently(&self, stream_id: Uuid, now: Instant) -> bool {
        let mut st = self.lock();
        st.failed_until
            .retain(|_, until| *until > now);
        st.failed_until
            .contains_key(&stream_id)
    }

    /// Whether an addon kept for seek previews only was asked for a title
    /// lately: within [`PREVIEW_ASKED_WAIT`] of an answer, or
    /// [`PREVIEW_RETRY_WAIT`] of a failed or dropped ask.
    pub(crate) fn asked_recently(&self, title_id: Uuid, addon_id: Uuid) -> bool {
        let now = Instant::now();
        self.lock()
            .asked
            .get(&(title_id, addon_id))
            .is_some_and(|asked| asked.until > now)
    }

    /// Records an ask of an addon kept for seek previews only for a title:
    /// `kept` is how many streams its answer kept, `None` for an ask that
    /// failed or was dropped.
    pub(crate) fn mark_asked(
        &self,
        title_id: Uuid,
        addon_id: Uuid,
        kept: Option<usize>,
    ) {
        let now = Instant::now();
        let (wait, kept) = match kept {
            Some(kept) => (PREVIEW_ASKED_WAIT, kept),
            None => (PREVIEW_RETRY_WAIT, 0),
        };
        let mut st = self.lock();
        st.asked
            .retain(|_, asked| asked.until > now);
        st.asked
            .insert(
                (title_id, addon_id),
                Asked {
                    until: now + wait,
                    kept,
                },
            );
    }

    /// Whether the title's last asks of addons kept for seek previews only
    /// all kept nothing: then its item lists no version that only such an
    /// addon could stand in for, until those asks may be made again. False
    /// for a title not asked lately.
    pub fn kept_nothing(&self, title_id: Uuid) -> bool {
        let now = Instant::now();
        let st = self.lock();
        let mut last = st
            .asked
            .iter()
            .filter(|((title, _), asked)| *title == title_id && asked.until > now)
            .peekable();
        last.peek()
            .is_some()
            && last.all(|(_, asked)| asked.kept == 0)
    }

    /// Moves `plan` on to the next of its other versions that has not failed
    /// recently and has no folder of its own, dropping those passed over. The
    /// list was made when the play started: a version given a folder of its
    /// own since then (see [`moved_if_longer`]) would be read into it. False
    /// when none is left.
    pub(super) fn next_source(&self, plan: &mut PassPlan) -> bool {
        let now = Instant::now();
        while !plan
            .others
            .is_empty()
        {
            let source = plan
                .others
                .remove(0);
            if self.failed_recently(source.stream_id, now)
                || set_dir(&plan.root, source.stream_id).is_dir()
            {
                continue;
            }
            match source.kind {
                SourceKind::Preview => info!(
                    set_id = %plan.set_id,
                    stream_id = %source.stream_id,
                    "trick play: reading a preview source's stream for this title's pictures"
                ),
                SourceKind::Played => info!(
                    set_id = %plan.set_id,
                    stream_id = %source.stream_id,
                    "trick play: reading the version played for its pictures"
                ),
                SourceKind::Other => info!(
                    set_id = %plan.set_id,
                    stream_id = %source.stream_id,
                    "trick play: reading another version of this title for its pictures"
                ),
            }
            plan.read_other(source);
            return true;
        }
        false
    }

    /// Moves `plan` on to the stream a play that joined job `job_id` would
    /// read (see [`Self::queue`]): the first that is not the stream `plan`
    /// reads, has not failed recently and has no folder of its own, dropping
    /// those passed over. Only plays still on the set are listed (see
    /// [`Self::leave`]). It is then judged at slot entry like any source
    /// (see [`Self::refusal`]), so an addon not to be read stays unread.
    /// False when none is left.
    pub(super) fn next_joined(&self, plan: &mut PassPlan, job_id: u64) -> bool {
        let now = Instant::now();
        loop {
            let joined = {
                let mut st = self.lock();
                let Some(pass) = st
                    .by_set
                    .get_mut(&plan.set_id)
                    .filter(|pass| pass.job_id == job_id)
                else {
                    return false;
                };
                if pass
                    .joined
                    .is_empty()
                {
                    return false;
                }
                let (_, joined) = pass
                    .joined
                    .remove(0);
                joined
            };
            if joined.stream_id == plan.stream_id
                || self.failed_recently(joined.stream_id, now)
                || set_dir(&plan.root, joined.stream_id).is_dir()
            {
                continue;
            }
            info!(
                set_id = %plan.set_id,
                stream_id = %joined.stream_id,
                "trick play: reading the version another play of this set reads"
            );
            plan.read_joined(joined);
            return true;
        }
    }

    /// Whether a run of this stream may start part way into the file.
    pub(super) fn can_jump(&self, stream_id: Uuid) -> bool {
        let now = Instant::now();
        let mut st = self.lock();
        st.no_jump
            .retain(|_, until| *until > now);
        !st.no_jump
            .contains_key(&stream_id)
    }

    /// Reads this stream from its start for [`NO_JUMP_WAIT`].
    pub(super) fn mark_no_jump(&self, stream_id: Uuid) {
        let until = Instant::now()
            + self
                .inner
                .limits
                .no_jump_wait;
        self.lock()
            .no_jump
            .insert(stream_id, until);
    }

    /// The least quiet of a set's plays, each asked of `watch` outside the
    /// lock. A play counts as quiet since it registered when its device has no
    /// reported session on its item, or when the session reporting is another
    /// play registered here on another set (a version of another length,
    /// played after a stop this play never got). `None` for a set with no
    /// plays, which leaves the decision to the grace period.
    pub(super) fn quiet<W>(&self, set_id: Uuid, watch: &W) -> Option<Watched>
    where
        W: Fn(&str, Uuid) -> Option<Watched>,
    {
        let plays: Vec<(String, Uuid, Instant)> = {
            let st = self.lock();
            let pass = st
                .by_set
                .get(&set_id)?;
            pass.sessions
                .iter()
                .filter_map(|id| {
                    st.by_session
                        .get(id)
                })
                .map(|reg| {
                    (
                        reg.device_id
                            .clone(),
                        reg.item_id,
                        reg.at,
                    )
                })
                .collect()
        };
        let heard: Vec<(Instant, Option<Watched>)> = plays
            .into_iter()
            .map(|(device_id, item_id, at)| (at, watch(&device_id, item_id)))
            .collect();
        let st = self.lock();
        heard
            .into_iter()
            .map(|(at, heard)| {
                let elsewhere = |w: &Watched| {
                    st.by_session
                        .get(&w.play_session_id)
                        .and_then(|reg| reg.set_id)
                        .is_some_and(|other| other != set_id)
                };
                match heard {
                    Some(w) if !elsewhere(&w) => w,
                    _ => Watched {
                        quiet: at.elapsed(),
                        position_s: 0,
                        play_session_id: String::new(),
                    },
                }
            })
            .min_by_key(|heard| heard.quiet)
    }

    /// Whether some play of a set was heard from within `limit`. A set with
    /// no plays is not: a job that stood down, or is waiting for the slot,
    /// stays down until a play reports, or the grace period ends it.
    pub(super) fn heard_within<W>(
        &self,
        set_id: Uuid,
        watch: &W,
        limit: Duration,
    ) -> bool
    where
        W: Fn(&str, Uuid) -> Option<Watched>,
    {
        self.quiet(set_id, watch)
            .is_some_and(|heard| heard.quiet < limit)
    }

    /// Whether a running pass should give way: its plays have all been quiet
    /// for five minutes, or for a minute while another pass is waiting. Gives
    /// what was last heard, for the log.
    pub(super) fn gives_way<W>(&self, set_id: Uuid, watch: &W) -> Option<Watched>
    where
        W: Fn(&str, Uuid) -> Option<Watched>,
    {
        let limits = self
            .inner
            .limits;
        let heard = self.quiet(set_id, watch)?;
        let give_way = heard.quiet >= limits.quiet_alone
            || (heard.quiet >= limits.quiet_waiting && self.other_waiting(set_id));
        give_way.then_some(heard)
    }

    pub(super) fn set_waiting(&self, set_id: Uuid, job_id: u64, waiting: bool) {
        if let Some(pass) = self
            .lock()
            .by_set
            .get_mut(&set_id)
            && pass.job_id == job_id
        {
            pass.waiting = waiting;
        }
    }

    /// Whether a pass other than this set's is waiting for the slot.
    pub(super) fn other_waiting(&self, set_id: Uuid) -> bool {
        self.lock()
            .by_set
            .iter()
            .any(|(id, pass)| *id != set_id && pass.waiting)
    }
}
