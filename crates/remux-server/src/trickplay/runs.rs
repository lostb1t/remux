//! Running a pass: waiting for the slot, its runs, and what each run leaves.

use super::*;

/// One run of ffmpeg in a pass: the sheet it starts at, and how many sheets
/// it makes before it stops, or `None` to read on to the end of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Run {
    pub(super) start: u32,
    pub(super) stop_after: Option<u64>,
}

/// The next run of a pass, or `None` when nothing is missing. A source that
/// cannot jump is read from its start to its end. Otherwise the run starts at
/// the first missing sheet at or after the viewer's, or the first missing
/// sheet when there is none, and covers the unbroken stretch of missing
/// sheets from there, so nothing already on disk is read again; it reads on
/// to the end when that stretch reaches the last expected sheet before any
/// run has reached the end. The viewer's sheet is 0 unless
/// [`start_at_position`] is on, so a run then starts at the lowest missing
/// sheet and a set never gets a gap.
pub(super) fn next_run(
    missing: &[u32],
    viewer: u32,
    can_jump: bool,
    end: bool,
    expected: u32,
) -> Option<Run> {
    if missing.is_empty() {
        return None;
    }
    if !can_jump {
        return Some(Run {
            start: 0,
            stop_after: None,
        });
    }
    let at = missing
        .iter()
        .position(|n| *n >= viewer)
        .unwrap_or(0);
    let start = missing[at];
    let stretch = missing[at..]
        .iter()
        .zip(start..)
        .take_while(|(n, want)| **n == *want)
        .count();
    let stretch = u32::try_from(stretch).unwrap_or(u32::MAX);
    let to_the_end = !end && start.saturating_add(stretch) >= expected;
    Some(Run {
        start,
        stop_after: (!to_the_end).then_some(u64::from(stretch)),
    })
}

/// How long a run that starts part way waits for its first sheet: one
/// sheet's share of the file at [`SEEK_READ_BYTES_PER_SEC`], at least
/// `at_least`.
pub(super) fn first_sheet_wait(
    size: Option<u64>,
    expected: u32,
    at_least: Duration,
) -> Duration {
    let share = size.unwrap_or(0) / u64::from(expected.max(1));
    at_least.max(Duration::from_secs(share / SEEK_READ_BYTES_PER_SEC))
}

/// Whether a run should stop so the pass starts again at `viewer`, the sheet
/// the viewer is now on: the source can jump, that sheet is not on disk and
/// is not one of the next two this run makes (`next` is the next), the run
/// is at least `follow_every` old, and the pass has moved fewer than
/// `max_follows` times.
pub(super) fn follows_to(
    viewer: u32,
    on_disk: bool,
    next: u32,
    run_age: Duration,
    follows: u32,
    can_jump: bool,
    limits: &Limits,
) -> bool {
    can_jump
        && !on_disk
        && !(next..next.saturating_add(2)).contains(&viewer)
        && run_age >= limits.follow_every
        && follows < limits.max_follows
}

/// Estimated disk use of a runtime's pictures (about 330 KB a sheet). The
/// admin page quotes it for a two-hour movie (about 9 MB).
pub(super) fn estimate_bytes(runtime_s: u64) -> u64 {
    runtime_s * 330 * 1024 / 250
}

impl TrickplayPasses {
    /// The version played, looked at again every [`LENGTH_CHECK`] during the
    /// start delay until it has a measured length: one read from its header
    /// as the play's first bytes pass (see [`store_length`]). Returns it as
    /// read last, at the end of the delay at the latest, or at once when the
    /// play was released meanwhile, with the time it took: the plan keeps
    /// that (see [`PassPlan::delay_spent`]), and it is taken off the start
    /// delay [`Self::begin_with`] waits, so no wait runs past it. `look`
    /// reads the row again by its id.
    pub(crate) async fn wait_for_length<L, F>(
        &self,
        play_session_id: &str,
        mut stream: db::Media,
        look: L,
    ) -> (db::Media, Duration)
    where
        L: Fn(Uuid) -> F,
        F: Future<Output = Option<db::Media>>,
    {
        let limits = self
            .inner
            .limits;
        let started = Instant::now();
        while measured_ticks(&stream).is_none() {
            let left = limits
                .start_delay
                .saturating_sub(started.elapsed());
            if left.is_zero() {
                break;
            }
            tokio::time::sleep(
                limits
                    .length_check
                    .min(left),
            )
            .await;
            if self
                .play_of(play_session_id)
                .is_none()
            {
                break;
            }
            if let Some(read) = look(stream.id)
                .await
                .filter(|read| measured_ticks(read).is_some())
            {
                stream = read;
            }
        }
        (stream, started.elapsed())
    }

    /// From a resolved file to a queued pass: attaches the play, waits the
    /// start delay, then queues a pass unless the play ended or is no longer
    /// on this item, the set is complete or the file failed recently and no
    /// other version can be read in its place.
    ///
    /// `watch` tells, for a device and an item, whether that device's latest
    /// reported session is on that item, how long ago it reported and where
    /// it was. The pass asks it while it waits and while it runs.
    ///
    /// `finder` is asked for the streams of addons kept for seek previews
    /// only once the start delay has passed, the play is still watched and
    /// its set is not complete, and only when the plan wants them (see
    /// [`PreviewAsk`]), the switch is still on and such an addon is still
    /// enabled. So a play that ended in its first seconds, a finished set, or
    /// a server with no such addon asks nothing. The version played is read
    /// after them, when its addon allows (see [`put_preview_sources`]).
    pub(crate) async fn begin_with<R, W, F>(
        &self,
        play_session_id: &str,
        mut plan: PassPlan,
        runner: Arc<R>,
        watch: W,
        finder: &F,
    ) -> Begin
    where
        R: PassRunner,
        W: Fn(&str, Uuid) -> Option<Watched> + Send + Sync + 'static,
        F: PreviewFinder,
    {
        if !self.attach(play_session_id, plan.set_id) {
            return Begin::Released;
        }
        if let Some(pass) = self
            .lock()
            .by_set
            .get_mut(&plan.set_id)
        {
            // Only a set keyed by its count knows it, as in `set_for_stream`;
            // a stream's own set may run past a count guessed from its runtime.
            pass.pictures = (plan.set_id != plan.stream_id).then_some(plan.pictures);
        }
        // Time spent waiting for the length of the version played counts.
        tokio::time::sleep(
            self.inner
                .limits
                .start_delay
                .saturating_sub(plan.delay_spent),
        )
        .await;
        let Some((device_id, item_id)) = self.play_of(play_session_id) else {
            return Begin::Released;
        };
        if watch(&device_id, item_id).is_none() {
            self.release(play_session_id);
            return Begin::Released;
        }
        if is_done(&set_dir(&plan.root, plan.set_id)) {
            return Begin::Done;
        }
        if let Some(ask) = plan
            .preview
            .take()
        {
            // The switch may have been turned off during the start delay.
            let switched_on = match &plan.db {
                Some(pool) => db::Settings::get_config(pool)
                    .await
                    .map_or(true, |cfg| enabled(&cfg)),
                None => true,
            };
            let found = if switched_on && has_preview_source(&self.addon_rules()) {
                finder
                    .find(&ask)
                    .await
            } else {
                Found::default()
            };
            match put_preview_sources(&mut plan, found) {
                Ok(false) => {}
                Ok(true) => {
                    if !self.next_source(&mut plan) {
                        return Begin::FailedRecently;
                    }
                }
                Err(reason) => {
                    debug!(
                        item_id = %item_id,
                        set_id = %plan.set_id,
                        %reason,
                        "no trick play pictures for this play"
                    );
                    self.release(play_session_id);
                    return Begin::NoSource;
                }
            }
        }
        // Nothing is opened here, so a version passed over does not count
        // towards `MAX_SOURCES`.
        if self.failed_recently(plan.stream_id, Instant::now())
            && !self.next_source(&mut plan)
        {
            return Begin::FailedRecently;
        }
        self.queue(play_session_id, plan, runner, watch)
    }

    pub(super) fn queue<R, W>(
        &self,
        play_session_id: &str,
        plan: PassPlan,
        runner: Arc<R>,
        watch: W,
    ) -> Begin
    where
        R: PassRunner,
        W: Fn(&str, Uuid) -> Option<Watched> + Send + Sync + 'static,
    {
        let mut st = self.lock();
        let job_id = st.next_job;
        st.next_job += 1;
        let Some(pass) = st
            .by_set
            .get_mut(&plan.set_id)
        else {
            return Begin::Released;
        };
        if pass
            .cancel
            .is_some()
        {
            // Kept, so the job can read this play's version should its own
            // source fail, until this play leaves the set. One entry a play.
            pass.joined
                .retain(|(session, _)| session != play_session_id);
            pass.joined
                .push((play_session_id.to_string(), plan));
            return Begin::Joined;
        }
        let (cancel_tx, cancel_rx) = oneshot::channel();
        pass.cancel = Some(cancel_tx);
        pass.job_id = job_id;
        pass.joined
            .clear();
        let pid = pass
            .pid
            .clone();
        let passes = self.clone();
        pass.job = Some(tokio::spawn(async move {
            passes
                .run_job(plan, runner, cancel_rx, pid, job_id, watch)
                .await;
        }));
        Begin::Queued
    }

    /// A queued pass, until it ends: it waits for the slot while one of its
    /// plays is watched, runs, gives way when nobody has watched it lately,
    /// and waits to be watched again. A cancel (the grace period, `expire`,
    /// `kill_all`) ends it at any point, and so does a run that ended by
    /// itself. A source that could not be read is followed by the version a
    /// play that joined would read (see [`Self::next_joined`]), then by the
    /// next of the plan's other versions, each waiting for the slot like any
    /// pass, until [`MAX_SOURCES`] were read. A version played that was moved
    /// to a folder of its own is followed by the joined plays' versions too.
    pub(super) async fn run_job<R, W>(
        &self,
        mut plan: PassPlan,
        runner: Arc<R>,
        mut cancel: oneshot::Receiver<()>,
        pid: Arc<AtomicU32>,
        job_id: u64,
        watch: W,
    ) where
        R: PassRunner,
        W: Fn(&str, Uuid) -> Option<Watched> + Send + Sync + 'static,
    {
        let set_id = plan.set_id;
        let limits = self
            .inner
            .limits;
        let mut stood_down = false;
        let mut size_checked = None;
        let mut read = 0;
        loop {
            if stood_down {
                // Stood down: look again now and then until a play reports.
                let cancelled = loop {
                    tokio::select! {
                        _ = &mut cancel => break true,
                        _ = tokio::time::sleep(limits.watch_check) => {}
                    }
                    if self.heard_within(set_id, &watch, limits.quiet_waiting) {
                        break false;
                    }
                };
                if cancelled {
                    break;
                }
            }
            // Waiting for the slot, only while watched, keeping its place.
            if !self.heard_within(set_id, &watch, limits.quiet_waiting) {
                stood_down = true;
                continue;
            }
            self.set_waiting(set_id, job_id, true);
            let turn = {
                let acquire = self
                    .inner
                    .slots
                    .acquire();
                tokio::pin!(acquire);
                loop {
                    tokio::select! {
                        permit = &mut acquire => break Turn::Slot(permit),
                        _ = &mut cancel => break Turn::Cancelled,
                        _ = tokio::time::sleep(limits.watch_check) => {
                            if !self.heard_within(set_id, &watch, limits.quiet_waiting) {
                                break Turn::Unwatched;
                            }
                        }
                    }
                }
            };
            self.set_waiting(set_id, job_id, false);
            let permit = match turn {
                Turn::Slot(Ok(permit)) => permit,
                Turn::Slot(Err(_)) | Turn::Cancelled => break,
                Turn::Unwatched => {
                    stood_down = true;
                    continue;
                }
            };
            let end = self
                .run_in_slot(
                    &plan,
                    runner.as_ref(),
                    &mut cancel,
                    pid.clone(),
                    &watch,
                    &mut size_checked,
                )
                .await;
            drop(permit);
            match end {
                SlotEnd::StoodDown => {
                    stood_down = true;
                    continue;
                }
                SlotEnd::Failed => {
                    read += 1;
                    // The key check moved the version played to a folder of
                    // its own: its plays no longer show the set. A play that
                    // joined still does, moved or not: the version it would
                    // read comes first.
                    let moved = moved_away(&plan);
                    if read < MAX_SOURCES
                        && (self.next_joined(&mut plan, job_id)
                            || (!moved && self.next_source(&mut plan)))
                    {
                        continue;
                    }
                }
                // Nothing was opened: a source refused after it was opened
                // ends as `Failed` (see `refused_after_open`).
                SlotEnd::Skipped => {
                    if self.next_joined(&mut plan, job_id)
                        || self.next_source(&mut plan)
                    {
                        continue;
                    }
                }
                // The version played was longer than its stored length and
                // was made in a folder of its own: the set itself is still
                // to make for the plays that joined. That read counts.
                SlotEnd::Over => {
                    if moved_away(&plan) {
                        read += 1;
                        if read < MAX_SOURCES && self.next_joined(&mut plan, job_id) {
                            continue;
                        }
                    }
                }
                SlotEnd::Cancelled => {}
            }
            break;
        }
        self.job_ended(set_id, job_id);
    }

    /// Runs a pass while holding a slot, so prune never races a pass, with
    /// `busy` in the set's folder until it ends (see [`Self::run_pass`]).
    /// Another version's size is checked once a job before it is read (see
    /// [`is_listed_file`]); `size_checked` holds the stream last checked.
    pub(super) async fn run_in_slot<R, W>(
        &self,
        plan: &PassPlan,
        runner: &R,
        cancel: &mut oneshot::Receiver<()>,
        pid: Arc<AtomicU32>,
        watch: &W,
        size_checked: &mut Option<Uuid>,
    ) -> SlotEnd
    where
        R: PassRunner,
        W: Fn(&str, Uuid) -> Option<Watched>,
    {
        // After the key check moved the set to the stream's own folder, a pass
        // that stood down and comes back carries on there. That folder exists
        // only after a move, or `set_for` would have chosen it.
        let own = set_dir(&plan.root, plan.stream_id);
        let dir = if plan.set_id != plan.stream_id && own.is_dir() {
            own
        } else {
            set_dir(&plan.root, plan.set_id)
        };
        if is_done(&dir) {
            return SlotEnd::Over;
        }
        if self.failed_recently(plan.stream_id, Instant::now()) {
            return SlotEnd::Skipped;
        }
        // Switched off, or its addon's previews turned off, while this pass
        // waited: start nothing. Another version and the start position
        // follow their own switches as they are now, too, and the disk limits
        // are the ones saved now, not those of when the play started. A
        // change to the start position while the pass reads counts from its
        // next turn.
        let cfg = match &plan.db {
            Some(pool) => Some(db::Settings::get_config_or_default(pool).await),
            None => None,
        };
        if let Some(end) = self.refusal(plan, cfg.as_ref()) {
            return end;
        }
        let (from_position, cap, keep) = match &cfg {
            Some(cfg) => (
                start_at_position(cfg),
                cap_bytes(cfg).saturating_sub(plan.estimate),
                keep_for(cfg),
            ),
            // No settings to read: nothing is pruned by size or age.
            None => (plan.from_position, u64::MAX, None),
        };
        let plan = &PassPlan {
            from_position,
            ..plan.clone()
        };
        let mut active: HashSet<Uuid> = self
            .lock()
            .by_set
            .keys()
            .copied()
            .collect();
        // Where the key check may have moved this set.
        active.insert(plan.stream_id);
        let root = plan
            .root
            .clone();
        // `active` is a snapshot: a play that attaches to an old finished set
        // just after it was taken can see that set deleted. That play's pass
        // then normally makes it again, since `begin` looks for `done` only
        // after its start delay.
        let _ = tokio::task::spawn_blocking(move || {
            prune(&root, cap, keep, &active, SystemTime::now())
        })
        .await;
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            warn!(error = %e, "could not create the trick play directory");
            self.mark_failed(
                plan.stream_id,
                Instant::now()
                    + self
                        .inner
                        .limits
                        .failure_wait,
            );
            return SlotEnd::Over;
        }
        let min_free_bytes = self
            .inner
            .limits
            .min_free_bytes;
        if free_bytes(&dir).is_some_and(|b| b < min_free_bytes) {
            warn!(
                "not making trick play pictures: less than {} GB of disk is free",
                min_free_bytes / (1024 * 1024 * 1024)
            );
            return SlotEnd::Over;
        }
        // Another version is read only if its source still holds the file it
        // listed: a resolver can hand over another release, which no later
        // check would tell from the right one.
        if let Some(listed) = plan
            .other_version
            .as_ref()
            && *size_checked != Some(plan.stream_id)
        {
            *size_checked = Some(plan.stream_id);
            let found = tokio::select! {
                found = runner.size(&plan.input) => found,
                _ = &mut *cancel => return SlotEnd::Cancelled,
            };
            if !is_listed_file(listed, found) {
                self.mark_plan_failed(plan);
                info!(
                    set_id = %plan.set_id,
                    stream_id = %plan.stream_id,
                    listed = ?listed.size,
                    found = ?found,
                    "trick play: another version's source does not hold the file it listed"
                );
                return SlotEnd::Failed;
            }
        }
        // `busy` tells the start-up prune that a pass was at work here, should
        // the server stop without removing it.
        if let Err(e) = std::fs::write(dir.join(BUSY_MARKER), b"") {
            warn!(error = %e, "could not mark trick play pictures as being made");
        }
        info!(
            set_id = %plan.set_id,
            stream_id = %plan.stream_id,
            "making trick play pictures"
        );
        let mut dir = dir;
        let end = self
            .run_pass(plan, runner, cancel, pid, watch, &mut dir)
            .await;
        let _ = std::fs::remove_file(dir.join(BUSY_MARKER));
        end
    }

    /// The runs of one pass (see [`next_run`]), the first at the sheet the
    /// viewer is on with [`start_at_position`], else at the lowest missing
    /// sheet, until nothing is missing, a run fails, the pass is
    /// stopped or [`MAX_RUNS`] runs were made. The set is marked `done` once a
    /// run has reached the end of the file and no sheet is missing, so never
    /// before its beginning is made. `dir` follows the set if the key check
    /// moves it.
    pub(super) async fn run_pass<R, W>(
        &self,
        plan: &PassPlan,
        runner: &R,
        cancel: &mut oneshot::Receiver<()>,
        pid: Arc<AtomicU32>,
        watch: &W,
        dir: &mut PathBuf,
    ) -> SlotEnd
    where
        R: PassRunner,
        W: Fn(&str, Uuid) -> Option<Watched>,
    {
        let expected = expected_sheets(plan.pictures);
        let mut viewer = if plan.from_position {
            self.quiet(plan.set_id, watch)
                .map_or(0, |heard| sheet_at(heard.position_s, expected))
        } else {
            0
        };
        let mut follows = 0;
        let mut nothing = 0;
        // The sheets another version made in this slot. They are on trial: a
        // read that ends by itself short of the end, with no sign that the
        // source failed, is the sign of a shorter file, and they are deleted
        // for the next version to make (see `drop_on_trial`). Sheets it made
        // before standing down in an earlier slot are not counted.
        let mut on_trial: Vec<u32> = Vec::new();
        for made_runs in 0..MAX_RUNS {
            let state = read_set(dir);
            let can_jump = self.can_jump(plan.stream_id);
            let needed = missing(&state, expected);
            let Some(run) = next_run(&needed, viewer, can_jump, state.end, expected)
            else {
                break;
            };
            // A stop that came between two runs: start no ffmpeg for it.
            if !matches!(cancel.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
                return SlotEnd::Cancelled;
            }
            // Switched off, or its addon's previews turned off, since the last
            // run. The first run follows the check at slot entry.
            if made_runs > 0
                && let Some(end) = self
                    .refused_now(plan)
                    .await
            {
                drop_stand_in(plan, dir, &on_trial);
                return refused_after_open(end);
            }
            let (result, stopped) = self
                .one_run(plan, runner, cancel, pid.clone(), watch, dir, run, follows)
                .await;
            let end = run_end(&result, run, plan.pictures);
            let moved = self.after_run(plan, dir, run, &result, end);
            if stopped == Some(Stopped::Cancelled) {
                return SlotEnd::Cancelled;
            }
            // A source that stopped sending without closing: it is not read
            // again for its failure wait, and the slot is free for others.
            if stopped == Some(Stopped::Stalled) {
                self.mark_plan_failed(plan);
                return SlotEnd::Failed;
            }
            // Another version longer than its stored length: its pictures are
            // in a folder of its own, which nobody plays, and the set is made
            // from the next version.
            if moved
                && plan
                    .other_version
                    .is_some()
            {
                self.mark_plan_failed(plan);
                return SlotEnd::Failed;
            }
            if plan
                .other_version
                .is_some()
            {
                let made = u32::try_from(result.made_sheets).unwrap_or(u32::MAX);
                on_trial.extend(
                    run.start
                        ..run
                            .start
                            .saturating_add(made),
                );
            }
            // Refused while it read: a stand-in is never read on to check its
            // length, so what it made in this slot goes.
            if let Some(Stopped::Refused(end)) = stopped {
                drop_stand_in(plan, dir, &on_trial);
                return refused_after_open(end);
            }
            match (end, stopped) {
                (RunEnd::Failed, _) => {
                    if result.exit_ok && !source_failed(&result) {
                        drop_on_trial(plan, dir, &on_trial);
                    }
                    return SlotEnd::Failed;
                }
                // It read to the end of the file and passed the key check
                // (see `after_run`): its sheets are kept, but its last one
                // went and the end is not marked, so the next play reads the
                // end of this version again. No other source is opened for
                // one sheet.
                (RunEnd::Unclean, _) => return SlotEnd::Over,
                (_, Some(Stopped::StoodDown)) => return SlotEnd::StoodDown,
                (_, Some(Stopped::Moved(to))) => {
                    viewer = to;
                    follows += 1;
                }
                // Stopped by low disk.
                (RunEnd::Cancelled, _) => return SlotEnd::Over,
                // Twice: the file ends before its stored length, or its source
                // ends every request at once. Another version's file then
                // cannot be the set's; the version played goes on to another
                // version when there is one.
                (RunEnd::Nothing, _) => {
                    nothing += 1;
                    if nothing >= 2 {
                        if plan
                            .other_version
                            .is_none()
                            && plan
                                .others
                                .is_empty()
                        {
                            return SlotEnd::Over;
                        }
                        self.mark_plan_failed(plan);
                        drop_on_trial(plan, dir, &on_trial);
                        info!(
                            set_id = %plan.set_id,
                            stream_id = %plan.stream_id,
                            "trick play: the file ends before its stored length"
                        );
                        return SlotEnd::Failed;
                    }
                    viewer = 0;
                }
                (RunEnd::Goal | RunEnd::End | RunEnd::NoJump, _) => {}
            }
        }
        let state = read_set(dir);
        let needed = missing(&state, expected);
        // The loop ends early only with nothing missing.
        if !needed.is_empty() {
            debug!(
                set_id = %plan.set_id,
                sheets_missing = needed.len(),
                "trick play pass reached its run limit"
            );
        }
        let reached_last = state
            .sheets
            .last()
            .is_some_and(|h| u64::from(*h) + 1 >= last_sheet(plan.pictures));
        if state.end && reached_last && needed.is_empty() {
            if let Err(e) = std::fs::write(dir.join(DONE_MARKER), b"") {
                warn!(error = %e, "could not mark trick play pictures complete");
            }
            info!(
                set_id = %plan.set_id,
                stream_id = %plan.stream_id,
                sheets = state
                    .sheets
                    .len(),
                "trick play pictures complete"
            );
        }
        SlotEnd::Over
    }

    /// One run of a pass. Every `watch_check` it asks whether the stream may
    /// still be read (see [`Self::refusal`]), then whether it has made no new
    /// sheet for [`STALL_WAIT`] (a stalled source), then whether to give way (see
    /// [`Self::gives_way`]), then, with [`start_at_position`] only, whether to
    /// follow the viewer to a part with no pictures (see [`follows_to`]); any
    /// of them stops the run, which then ends as cancelled.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn one_run<R, W>(
        &self,
        plan: &PassPlan,
        runner: &R,
        cancel: &mut oneshot::Receiver<()>,
        pid: Arc<AtomicU32>,
        watch: &W,
        dir: &Path,
        run: Run,
        follows: u32,
    ) -> (RunResult, Option<Stopped>)
    where
        R: PassRunner,
        W: Fn(&str, Uuid) -> Option<Watched>,
    {
        let limits = self
            .inner
            .limits;
        let expected = expected_sheets(plan.pictures);
        let can_jump = self.can_jump(plan.stream_id);
        let made = Arc::new(AtomicU32::new(0));
        let first_sheet_within = (run.start > 0)
            .then(|| first_sheet_wait(plan.size, expected, limits.seek_wait));
        let job = PassJob {
            input: plan
                .input
                .clone(),
            out_dir: dir.to_path_buf(),
            pid,
            min_free_bytes: limits.min_free_bytes,
            start_sheet: run.start,
            stop_after: run.stop_after,
            first_sheet_within,
            made: made.clone(),
        };
        // A run that starts part way has its first sheet's own wait, which
        // may be longer for a large file.
        let stall_after = limits
            .stall_wait
            .max(first_sheet_within.unwrap_or_default());
        let mut last_made = 0;
        let mut made_at = Instant::now();
        let began = Instant::now();
        let (stop_tx, stop_rx) = oneshot::channel();
        let mut stop = Some(stop_tx);
        let mut stopped = None;
        let ran = runner.run(job, stop_rx);
        tokio::pin!(ran);
        let result = loop {
            tokio::select! {
                result = &mut ran => break result,
                _ = &mut *cancel, if stop.is_some() => {
                    stopped = Some(Stopped::Cancelled);
                    if let Some(stop) = stop.take() {
                        let _ = stop.send(());
                    }
                }
                _ = tokio::time::sleep(limits.watch_check), if stop.is_some() => {
                    let made_now = made.load(Ordering::SeqCst);
                    if made_now != last_made {
                        last_made = made_now;
                        made_at = Instant::now();
                    }
                    // Switched off, or its addon's previews turned off, while
                    // it reads: the read stops within one check.
                    let why = if let Some(end) = self.refused_now(plan).await {
                        Some(Stopped::Refused(end))
                    } else if made_at.elapsed() >= stall_after {
                        warn!(
                            set_id = %plan.set_id,
                            stream_id = %plan.stream_id,
                            start = run.start,
                            sheets = made_now,
                            "trick play pass stalled: no new picture from the source for {} minutes; \
                             not retrying for a while",
                            stall_after.as_secs() / 60
                        );
                        Some(Stopped::Stalled)
                    } else if let Some(heard) = self.gives_way(plan.set_id, watch) {
                        info!(
                            set_id = %plan.set_id,
                            stream_id = %plan.stream_id,
                            quiet_s = heard.quiet.as_secs(),
                            position_s = heard.position_s,
                            "trick play pass giving way: nobody has watched it lately"
                        );
                        Some(Stopped::StoodDown)
                    } else if !plan.from_position {
                        // In order from the start: seeks are not followed.
                        None
                    } else {
                        // Only a viewer heard from lately is followed.
                        self.quiet(plan.set_id, watch)
                            .filter(|heard| heard.quiet < limits.quiet_waiting)
                            .map(|heard| sheet_at(heard.position_s, expected))
                            .filter(|to| {
                                let next = run
                                    .start
                                    .saturating_add(made.load(Ordering::SeqCst));
                                // With `end` set, the last sheet is one a run
                                // found beyond the end of the file: it never
                                // comes, so it is not followed to.
                                let on_disk = sheet_whole(&sheet_file(dir, *to))
                                    || (*to >= expected.saturating_sub(1)
                                        && dir
                                            .join(END_MARKER)
                                            .exists());
                                follows_to(
                                    *to,
                                    on_disk,
                                    next,
                                    began.elapsed(),
                                    follows,
                                    can_jump,
                                    &limits,
                                )
                            })
                            .map(|to| {
                                info!(
                                    set_id = %plan.set_id,
                                    stream_id = %plan.stream_id,
                                    sheet = to,
                                    "trick play pass moving to the part being watched"
                                );
                                Stopped::Moved(to)
                            })
                    };
                    if why.is_some() {
                        stopped = why;
                        if let Some(stop) = stop.take() {
                            let _ = stop.send(());
                        }
                    }
                }
            }
        };
        (result, stopped)
    }

    /// Acts on how a run ended (see [`run_end`]). Sheets on disk are kept and
    /// built on; a run deletes only its own last sheet, and only when that
    /// may be partly filled: after a failure, or when it was stopped after
    /// its output had already ended. No failure or stop marks a set finished
    /// or deletes it. True when the key check moved the set (see
    /// [`moved_if_longer`]).
    pub(super) fn after_run(
        &self,
        plan: &PassPlan,
        dir: &mut PathBuf,
        run: Run,
        result: &RunResult,
        end: RunEnd,
    ) -> bool {
        let last_made = (result.made_sheets > 0)
            .then(|| u64::from(run.start) + result.made_sheets - 1);
        let last_made_n = last_made.and_then(|n| u32::try_from(n).ok());
        let mut moved_set = false;
        // The key check, for every run that read to the end of the file: one
        // that wrote a line the tables do not know read the file's length
        // all the same.
        if matches!(end, RunEnd::End | RunEnd::Unclean)
            && let Some(highest) = last_made
        {
            let moved = moved_if_longer(plan, dir.as_path(), highest);
            if moved != *dir {
                // The set's folder is gone: its plays follow their own pick,
                // which finds this stream's folder.
                self.lock()
                    .selection
                    .retain(|_, s| *s != plan.set_id);
                // Sheets below this run's start may be another version's:
                // this pass makes them again from this file.
                for n in 0..run.start {
                    remove_sheet(&moved, n);
                }
                *dir = moved;
                moved_set = true;
            }
        }
        match end {
            RunEnd::Goal => debug!(
                set_id = %plan.set_id,
                start = run.start,
                sheets = result.made_sheets,
                "trick play run made the sheets it was after"
            ),
            RunEnd::End => {
                if let Err(e) = std::fs::write(dir.join(END_MARKER), b"") {
                    warn!(error = %e, "could not mark the end of trick play pictures");
                }
                debug!(
                    set_id = %plan.set_id,
                    start = run.start,
                    sheets = result.made_sheets,
                    exit_code = ?result.exit_code,
                    stderr = %crate::playback::probe::redact_urls_in(&result.stderr),
                    "trick play run reached the end of the file"
                );
            }
            RunEnd::Cancelled => {
                // A stopped run is killed, and each sheet is renamed into
                // place once whole, so every sheet on disk is good, unless
                // ffmpeg had already reached its end and flushed a partly
                // filled one.
                if result.ended_itself
                    && let Some(n) = last_made_n
                {
                    remove_sheet(dir, n);
                }
                debug!(
                    set_id = %plan.set_id,
                    stream_id = %plan.stream_id,
                    sheets = result.made_sheets,
                    "trick play pass stopped"
                );
            }
            RunEnd::NoJump => {
                self.mark_no_jump(plan.stream_id);
                info!(
                    set_id = %plan.set_id,
                    stream_id = %plan.stream_id,
                    start = run.start,
                    "trick play: no picture in time after a jump; reading this source from its start"
                );
            }
            RunEnd::Nothing => debug!(
                set_id = %plan.set_id,
                start = run.start,
                exit_code = ?result.exit_code,
                "trick play: the file ends before that place"
            ),
            RunEnd::Unclean => {
                // No `end` marker: the last sheet may be partly filled, so it
                // goes, and a later run reads the end of the file again. Not
                // a failure: the next play reads the end of this version, not
                // another version.
                if let Some(n) = last_made_n {
                    remove_sheet(dir, n);
                }
                warn!(
                    set_id = %plan.set_id,
                    stream_id = %plan.stream_id,
                    start = run.start,
                    sheets = result.made_sheets,
                    stderr = %crate::playback::probe::redact_urls_in(&result.stderr),
                    "trick play run reached the end of the file with a message it does not know; \
                     not marking the end"
                );
            }
            RunEnd::Failed => {
                // The last sheet this run wrote may be a partly filled one
                // flushed at a false end of input.
                if let Some(n) = last_made_n {
                    remove_sheet(dir, n);
                }
                self.mark_plan_failed(plan);
                if plan
                    .other_version
                    .is_some()
                {
                    info!(
                        set_id = %plan.set_id,
                        stream_id = %plan.stream_id,
                        start = run.start,
                        exit_ok = result.exit_ok,
                        exit_code = ?result.exit_code,
                        sheets = result.made_sheets,
                        stderr = %crate::playback::probe::redact_urls_in(&result.stderr),
                        "trick play: another version could not be read"
                    );
                } else {
                    warn!(
                        set_id = %plan.set_id,
                        stream_id = %plan.stream_id,
                        start = run.start,
                        exit_ok = result.exit_ok,
                        exit_code = ?result.exit_code,
                        sheets = result.made_sheets,
                        stderr = %crate::playback::probe::redact_urls_in(&result.stderr),
                        "trick play pass did not finish; not retrying for 10 minutes"
                    );
                }
            }
        }
        moved_set
    }

    pub(super) fn job_ended(&self, set_id: Uuid, job_id: u64) {
        let mut st = self.lock();
        if let Some(pass) = st
            .by_set
            .get_mut(&set_id)
            && pass.job_id == job_id
        {
            pass.cancel = None;
            pass.job = None;
            pass.waiting = false;
            pass.pid
                .store(0, Ordering::SeqCst);
        }
    }

    /// Server shutdown: cancels every pass with no grace period and waits for
    /// each process to exit, killing it if it does not. Exiting runs no
    /// destructors, so this is what keeps ffmpeg from outliving the server.
    /// The kill after `KILL_WAIT` works on Linux and macOS only (see
    /// `kill_pid`); elsewhere a pass that ignores its stop is left running.
    pub async fn kill_all(&self) {
        let passes: Vec<Pass> = {
            let mut st = self.lock();
            st.by_session
                .clear();
            st.selection
                .clear();
            st.by_set
                .drain()
                .map(|(_, pass)| pass)
                .collect()
        };
        let mut jobs = Vec::new();
        for mut pass in passes {
            if let Some(grace) = pass
                .grace
                .take()
            {
                grace.abort();
            }
            if let Some(cancel) = pass
                .cancel
                .take()
            {
                let _ = cancel.send(());
            }
            if let Some(job) = pass
                .job
                .take()
            {
                jobs.push((job, pass.pid));
            }
        }
        for (job, pid) in jobs {
            if tokio::time::timeout(KILL_WAIT, job)
                .await
                .is_err()
            {
                kill_pid(pid.load(Ordering::SeqCst));
            }
        }
    }
}

/// How a slot ends when its stream is refused (see
/// [`TrickplayPasses::refusal`]) after its source was opened: a stream not to
/// be read any more ends as `Failed`, so it counts towards [`MAX_SOURCES`]
/// like any source opened. A switch turned off ends it as before.
pub(super) fn refused_after_open(end: SlotEnd) -> SlotEnd {
    match end {
        SlotEnd::Skipped => SlotEnd::Failed,
        end => end,
    }
}

/// Whether the key check moved the version `plan` reads to a folder of its
/// own (see [`moved_if_longer`]): that folder exists only after a move, or
/// [`set_for`] would have chosen it.
pub(super) fn moved_away(plan: &PassPlan) -> bool {
    plan.other_version
        .is_none()
        && plan.set_id != plan.stream_id
        && set_dir(&plan.root, plan.stream_id).is_dir()
}
