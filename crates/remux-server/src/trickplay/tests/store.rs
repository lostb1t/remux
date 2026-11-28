//! A set's folder: missing sheets, mending after a crash, and pruning.

use super::*;

#[test]
fn last_sheet_counts_sheets() {
    // 7,200 s: 720 pictures, 29 sheets.
    let pictures = picture_count(RUNTIME as i64 * TICKS_PER_SEC);
    assert_eq!(pictures, 720);
    assert_eq!(last_sheet(pictures), 28);
    assert_eq!(expected_sheets(pictures), 29);
    // 940 pictures fill 38 sheets.
    assert_eq!(expected_sheets(940), 38);
    assert_eq!(last_sheet(940), 37);
    assert_eq!(last_sheet(950), 37);
    assert_eq!(last_sheet(951), 38);
    assert_eq!(last_sheet(0), 0);
    assert_eq!(progress_frame("frame=28"), Some(28));
    assert_eq!(progress_frame("fps=0.00"), None);
}

#[cfg(unix)]
#[test]
fn prune_removes_tmp_old_then_lru_and_spares_active() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp
        .path()
        .join(ROOT_DIR);
    let now = SystemTime::now();
    let day = Duration::from_secs(24 * 60 * 60);
    let set_age = |path: &Path, age: Duration| {
        std::fs::File::open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(now - age))
            .unwrap();
    };
    let make = |files: &[(&str, usize)]| {
        let id = Uuid::new_v4();
        let dir = set_dir(&root, id);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, len) in files {
            std::fs::write(dir.join(name), vec![0u8; *len]).unwrap();
        }
        (id, dir)
    };
    let (active_id, active) = make(&[("0.jpg", 100)]);
    let (_, old) = make(&[("0.jpg", 100)]);
    let (_, lru) = make(&[("0.jpg", 600)]);
    let (_, recent) = make(&[("0.jpg", 600), ("1.jpg.tmp", 10), ("2.jpg.tmp", 10)]);
    set_age(&recent.join("2.jpg.tmp"), Duration::from_secs(20 * 60));
    set_age(&active, 40 * day);
    set_age(&old, 40 * day);
    set_age(&lru, 2 * day);
    set_age(&recent, day);

    let keep = HashSet::from([active_id]);
    prune(&root, 1_000, Some(30 * day), &keep, now);
    assert!(active.exists(), "an active set is never deleted");
    assert!(!old.exists(), "unused for 30 days");
    assert!(!lru.exists(), "least recently used, over the cap");
    assert!(recent.exists());
    assert!(
        recent
            .join("1.jpg.tmp")
            .exists(),
        "a fresh temporary file is being written"
    );
    assert!(
        !recent
            .join("2.jpg.tmp")
            .exists()
    );

    let missing = tmp
        .path()
        .join("missing");
    prune_on_start(&missing, true, 0, None, now);
    assert!(!missing.exists());

    prune_on_start(&root, false, u64::MAX, None, now);
    assert!(!root.exists(), "switched off: every picture goes at start");
}

#[cfg(unix)]
#[test]
fn prune_on_start_mends_only_sets_a_crash_left_busy() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp
        .path()
        .join(ROOT_DIR);
    let now = SystemTime::now();
    let aged = |path: &Path, age: Duration| {
        std::fs::File::open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(now - age))
            .unwrap();
    };
    let minute = Duration::from_secs(60);

    // Unfinished, not busy: a stopped pass left only whole sheets.
    let unfinished = set_dir(&root, Uuid::new_v4());
    write_sheets(&unfinished, 0..3);

    // Busy: a crash. 2.jpg was written after the pass began, 3.jpg is
    // empty, 4.jpg is cut short.
    let crashed = set_dir(&root, Uuid::new_v4());
    write_sheets(&crashed, 0..3);
    std::fs::write(crashed.join("3.jpg"), b"").unwrap();
    std::fs::write(crashed.join("4.jpg"), [0xFF_u8, 0xD8, 0x00, 0x00]).unwrap();
    std::fs::write(crashed.join(BUSY_MARKER), b"").unwrap();
    for n in [0, 1, 3, 4] {
        aged(&crashed.join(format!("{n}.jpg")), 10 * minute);
    }
    aged(&crashed.join(BUSY_MARKER), 5 * minute);
    aged(&crashed.join("2.jpg"), minute);
    aged(&crashed, 3 * 24 * 60 * minute);
    let shown = std::fs::metadata(&crashed)
        .unwrap()
        .modified()
        .unwrap();

    // Finished and busy: its sheets are final.
    let finished = set_dir(&root, Uuid::new_v4());
    write_sheets(&finished, 0..3);
    std::fs::write(finished.join(DONE_MARKER), b"").unwrap();
    std::fs::write(finished.join(BUSY_MARKER), b"").unwrap();

    // Busy with `end`, its highest sheet written last: that sheet may be
    // partly filled, and the end it gave goes with it.
    let ended = set_dir(&root, Uuid::new_v4());
    write_sheets(&ended, 0..6);
    std::fs::write(ended.join(END_MARKER), b"").unwrap();
    std::fs::write(ended.join(BUSY_MARKER), b"").unwrap();
    for n in 0..5 {
        aged(&ended.join(format!("{n}.jpg")), 10 * minute);
    }
    aged(&ended.join(BUSY_MARKER), 5 * minute);
    aged(&ended.join("5.jpg"), minute);

    // Busy with `end`, a sheet below its highest written last: the end
    // stays.
    let resumed = set_dir(&root, Uuid::new_v4());
    write_sheets(&resumed, (0..3).chain(19..22));
    std::fs::write(resumed.join(END_MARKER), b"").unwrap();
    std::fs::write(resumed.join(BUSY_MARKER), b"").unwrap();
    for n in [0, 1] {
        aged(&resumed.join(format!("{n}.jpg")), 10 * minute);
    }
    aged(&resumed.join(BUSY_MARKER), 5 * minute);
    for n in 19..22 {
        aged(&resumed.join(format!("{n}.jpg")), 3 * minute);
    }
    aged(&resumed.join("2.jpg"), minute);

    for _ in 0..2 {
        prune_on_start(&root, true, u64::MAX, None, now);
    }
    assert_eq!(
        sheets_in(&unfinished),
        vec![0, 1, 2],
        "a set that is not busy keeps every sheet"
    );
    assert_eq!(sheets_in(&crashed), vec![0, 1]);
    assert!(
        !crashed
            .join(BUSY_MARKER)
            .exists()
    );
    assert_eq!(
        std::fs::metadata(&crashed)
            .unwrap()
            .modified()
            .unwrap(),
        shown,
        "when it was last shown is kept"
    );
    assert_eq!(sheets_in(&finished), vec![0, 1, 2]);
    assert!(
        !finished
            .join(BUSY_MARKER)
            .exists()
    );
    assert_eq!(sheets_in(&ended), vec![0, 1, 2, 3, 4]);
    assert!(
        !ended
            .join(END_MARKER)
            .exists(),
        "the next pass reads the end again"
    );
    assert_eq!(sheets_in(&resumed), vec![0, 1, 19, 20, 21]);
    assert!(
        resumed
            .join(END_MARKER)
            .exists()
    );
}

#[test]
fn the_start_up_prune_switched_off_deletes_only_its_own_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp
        .path()
        .join(ROOT_DIR);
    let set = set_dir(&root, Uuid::new_v4());
    write_sheets(&set, 0..2);
    let foreign_dir = root.join("notes");
    std::fs::create_dir_all(&foreign_dir).unwrap();
    let foreign_file = root.join("readme.txt");
    std::fs::write(&foreign_file, b"not a set").unwrap();
    let id_file = root.join(
        Uuid::new_v4()
            .as_simple()
            .to_string(),
    );
    std::fs::write(&id_file, b"not a folder").unwrap();

    prune_on_start(&root, false, u64::MAX, None, SystemTime::now());
    assert!(!set.exists());
    assert!(foreign_dir.exists());
    assert!(foreign_file.exists());
    assert!(id_file.exists());

    // With nothing else in it, the folder itself goes too.
    std::fs::remove_dir(&foreign_dir).unwrap();
    std::fs::remove_file(&foreign_file).unwrap();
    std::fs::remove_file(&id_file).unwrap();
    write_sheets(&set, 0..2);
    prune_on_start(&root, false, u64::MAX, None, SystemTime::now());
    assert!(!root.exists());
}

#[tokio::test]
async fn the_start_up_prune_follows_the_settings_handed_in_and_holds_the_slot() {
    let tmp = tempfile::tempdir().unwrap();
    let guard = ctx_in(tmp.path()).await;
    let ctx = &guard.0;
    let root = root_dir(
        &ctx.config
            .data_dir,
    );
    let set = set_dir(&root, Uuid::new_v4());
    write_sheets(&set, 0..2);
    let switch = |on: bool| api::ServerConfiguration {
        trickplay_while_streaming: Some(on),
        ..Default::default()
    };

    // The stored settings say off; the ones handed in say on.
    prune_at_startup(ctx, &switch(true)).await;
    assert!(set.exists(), "switched on: a fresh set stays");

    // While a pass holds the slot, it waits and deletes nothing.
    let held = ctx
        .sessions
        .trickplay
        .inner
        .slots
        .acquire()
        .await
        .unwrap();
    let pruning = tokio::spawn({
        let ctx = ctx.clone();
        let off = switch(false);
        async move {
            prune_at_startup(&ctx, &off).await;
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(set.exists(), "it waits for the slot");
    drop(held);
    pruning
        .await
        .unwrap();
    assert!(!set.exists(), "switched off: the set goes");
}

#[test]
fn missing_and_next_run_follow_the_order() {
    let run = |start: u32, stop_after: Option<u64>| Some(Run { start, stop_after });
    // 940 pictures: sheets 0 to 37.
    let next = |st: &SetState, viewer: u32| {
        next_run(&missing(st, 38), viewer, true, st.end, 38)
    };
    // A new set with the viewer at sheet 19: from there to the end of
    // the file, then the beginning up to where sheets exist, then done.
    assert_eq!(next(&state([], false), 19), run(19, None));
    assert_eq!(next(&state(19..38, true), 19), run(0, Some(19)));
    assert_eq!(next(&state(0..38, true), 19), None);
    // No run has reached the end: the rest, to the end.
    assert_eq!(next(&state(0..10, false), 0), run(10, None));
    // A gap once the end is reached, from a viewer before or after it;
    // a viewer inside it starts there.
    let gap = state((0..11).chain(20..38), true);
    assert_eq!(next(&gap, 0), run(11, Some(9)));
    assert_eq!(next(&gap, 5), run(11, Some(9)));
    assert_eq!(next(&gap, 30), run(11, Some(9)));
    assert_eq!(next(&gap, 15), run(15, Some(5)));
    // Two gaps and no end: the first stretch only, then on to the end,
    // so nothing on disk is read again.
    assert_eq!(
        next(&state((0..5).chain(10..15), false), 0),
        run(5, Some(5))
    );
    assert_eq!(next(&state(0..15, false), 0), run(15, None));
    // A source that cannot jump is read from its start.
    assert_eq!(
        next_run(&missing(&state([], false), 38), 19, false, false, 38),
        run(0, None)
    );
    // Once the end is reached, every sheet up to the one before the last
    // expected is missing until made, even when the run that reached the
    // end started at the last and made none; the last itself may never
    // come. Before the end, the last expected sheet always is missing.
    assert!(missing(&state(0..37, true), 38).is_empty());
    assert_eq!(
        missing(&state(0..30, true), 38),
        (30..37).collect::<Vec<u32>>()
    );
    assert_eq!(missing(&state([], true), 38), (0..37).collect::<Vec<u32>>());
    assert_eq!(missing(&state(0..38, false), 38), vec![37]);
    // The viewer's sheet, at most the last.
    assert_eq!(sheet_at(800, 5), 3);
    assert_eq!(sheet_at(5_000, 5), 4);
    assert_eq!(sheet_at(0, 38), 0);
}

#[test]
fn sheets_that_are_not_whole_count_as_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp
        .path()
        .join("set");
    write_sheets(&dir, 0..7);
    std::fs::write(dir.join("7.jpg"), b"").unwrap();
    std::fs::write(dir.join("8.jpg"), [0xFF_u8, 0xD8, b'j', b'p', b'g']).unwrap();
    let read = read_set(&dir);
    assert_eq!(read, state(0..7, false));
    assert_eq!(missing(&read, 10), vec![7, 8, 9]);
    std::fs::write(dir.join(END_MARKER), b"").unwrap();
    assert!(read_set(&dir).end);
}

#[cfg(unix)]
#[test]
fn prune_with_no_keep_time_goes_by_size_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp
        .path()
        .join(ROOT_DIR);
    let now = SystemTime::now();
    let old = set_dir(&root, Uuid::new_v4());
    let recent = set_dir(&root, Uuid::new_v4());
    for dir in [&old, &recent] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("0.jpg"), vec![0u8; 600]).unwrap();
        std::fs::write(dir.join(DONE_MARKER), b"").unwrap();
    }
    let set_age = |path: &Path, age: Duration| {
        std::fs::File::open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(now - age))
            .unwrap();
    };
    let day = Duration::from_secs(24 * 60 * 60);
    set_age(&old, 400 * day);
    set_age(&recent, day);

    // Under the size limit: a set not shown for 400 days stays.
    prune(&root, u64::MAX, None, &HashSet::new(), now);
    assert!(old.exists());
    assert!(recent.exists());
    prune_on_start(&root, true, u64::MAX, None, now);
    assert!(old.exists(), "a restart keeps it too");

    // Over it: the one shown longest ago goes first.
    prune(&root, 1_000, None, &HashSet::new(), now);
    assert!(!old.exists());
    assert!(recent.exists());
}
