//! Reading another version when the one played cannot be read.

use super::*;

#[tokio::test]
async fn a_failed_pass_goes_on_with_another_version() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let set_id = Uuid::new_v4();
    let (a, b) = (other_source("other-a"), other_source("other-b"));
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        dead: strings(&[PLAYED, "other-a"]),
        ..Default::default()
    });
    let plan = plan_with(tmp.path(), set_id, &[a.clone(), b.clone()]);
    let played = plan.stream_id;
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin("play-a", plan, runner.clone(), always())
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| !passes.is_queued(set_id)).await;

    assert_eq!(runner.inputs(), strings(&[PLAYED, "other-a", "other-b"]));
    assert_eq!(
        runner.sized(),
        strings(&["other-a", "other-b"]),
        "never the version played"
    );
    let dir = set_dir(tmp.path(), set_id);
    assert!(is_done(&dir));
    assert_eq!(sheets_in(&dir), (0..29).collect::<Vec<u32>>());
    // The waits are in memory, by stream; the folder holds no failure.
    let kept: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .map(str::to_string)
        })
        .filter(|name| {
            parse_sheet(name).is_none() && name != DONE_MARKER && name != END_MARKER
        })
        .collect();
    assert!(kept.is_empty(), "{kept:?}");
    let now = Instant::now();
    assert!(passes.failed_recently(played, now));
    assert!(
        !passes.failed_recently(played, now + FAILURE_WAIT + Duration::from_secs(1))
    );
    assert!(
        passes
            .failed_recently(a.stream_id, now + FAILURE_WAIT + Duration::from_secs(1))
    );
    assert!(!passes.failed_recently(
        a.stream_id,
        now + OTHER_FAILURE_WAIT + Duration::from_secs(1)
    ));
    assert!(!passes.failed_recently(b.stream_id, now));
}

#[tokio::test]
async fn a_pass_reads_three_sources_then_gives_up() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let set_id = Uuid::new_v4();
    let dir = set_dir(tmp.path(), set_id);
    write_sheets(&dir, 0..5);
    let others = [
        other_source("other-a"),
        other_source("other-b"),
        other_source("other-c"),
    ];
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        dead: strings(&[PLAYED, "other-a", "other-b", "other-c"]),
        ..Default::default()
    });
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin(
            "play-a",
            plan_with(tmp.path(), set_id, &others),
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.inputs(), strings(&[PLAYED, "other-a", "other-b"]));
    assert!(
        !passes.failed_recently(others[2].stream_id, Instant::now()),
        "never opened"
    );
    assert!(!is_done(&dir));
    assert_eq!(
        sheets_in(&dir),
        (0..5).collect::<Vec<u32>>(),
        "kept as they were"
    );
}

#[tokio::test]
async fn the_next_play_goes_to_the_next_version() {
    let tmp = tempfile::tempdir().unwrap();
    let failure_wait = Duration::from_millis(300);
    let passes = TrickplayPasses::with_limits(Limits {
        failure_wait,
        ..fast()
    });
    let set_id = Uuid::new_v4();
    let others = [
        other_source("other-a"),
        other_source("other-b"),
        other_source("other-c"),
    ];
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        dead: strings(&[PLAYED, "other-a", "other-b"]),
        ..Default::default()
    });
    let first = plan_with(tmp.path(), set_id, &others);
    let played = first.stream_id;
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin("play-a", first, runner.clone(), always())
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.inputs(), strings(&[PLAYED, "other-a", "other-b"]));

    // Once the version played may be read again, it is read first, then
    // the next version not resting: never the two that failed.
    tokio::time::sleep(failure_wait + Duration::from_millis(100)).await;
    passes.register("play-b", Uuid::new_v4(), "device-b");
    let begun = passes
        .begin(
            "play-b",
            PassPlan {
                stream_id: played,
                ..plan_with(tmp.path(), set_id, &others)
            },
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(
        runner.inputs(),
        strings(&[PLAYED, "other-a", "other-b", PLAYED, "other-c"])
    );
    assert!(is_done(&set_dir(tmp.path(), set_id)));
}

#[tokio::test]
async fn a_recent_failure_starts_with_another_version() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let set_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        ..Default::default()
    });
    let plan = plan_with(tmp.path(), set_id, &[other_source("other-a")]);
    passes.mark_failed(plan.stream_id, Instant::now() + FAILURE_WAIT);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin("play-a", plan, runner.clone(), always())
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.inputs(), strings(&["other-a"]));
    assert!(is_done(&set_dir(tmp.path(), set_id)));

    // With none to read in its place, nothing is queued, as before.
    let other_set = Uuid::new_v4();
    let plan = plan_with(tmp.path(), other_set, &[]);
    passes.mark_failed(plan.stream_id, Instant::now() + FAILURE_WAIT);
    passes.register("play-b", Uuid::new_v4(), "device-b");
    let begun = passes
        .begin("play-b", plan, runner.clone(), always())
        .await;
    assert_eq!(begun, Begin::FailedRecently);
}

#[tokio::test]
async fn another_version_builds_on_what_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let set_id = Uuid::new_v4();
    let dir = set_dir(tmp.path(), set_id);
    write_sheets(&dir, 0..10);
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        dead: strings(&[PLAYED]),
        ..Default::default()
    });
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            plan_with(tmp.path(), set_id, &[other_source("other-a")]),
            runner.clone(),
            always(),
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.inputs(), strings(&[PLAYED, "other-a"]));
    assert_eq!(runner.starts(), vec![10, 10]);
    assert!(is_done(&dir));
    assert_eq!(sheets_in(&dir), (0..29).collect::<Vec<u32>>());
}

#[tokio::test]
async fn another_version_stops_with_its_viewer() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(Limits {
        quiet_alone: Duration::from_millis(150),
        ..fast()
    });
    let reports = Reports::default();
    reports.fresh("device-a");
    let set_id = Uuid::new_v4();
    let dir = set_dir(tmp.path(), set_id);
    let other = other_source("other-a");
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        pace: Duration::from_millis(20),
        dead: strings(&[PLAYED]),
        ..Default::default()
    });
    let plan = plan_with(tmp.path(), set_id, std::slice::from_ref(&other));
    let played = plan.stream_id;
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin("play-a", plan, runner.clone(), reports.watch())
        .await;
    until(|| sheets_in(&dir).len() >= 2).await;

    // The viewer goes silent: the other version's run gives way.
    reports.silent("device-a", Duration::from_secs(1));
    until(|| {
        runner
            .killed
            .load(Ordering::SeqCst)
    })
    .await;
    assert!(passes.is_queued(set_id), "waiting to be watched again");
    // The play ends: the pass ends with it.
    passes.release("play-a");
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.inputs(), strings(&[PLAYED, "other-a"]));
    let made = sheets_in(&dir);
    assert!(!made.is_empty() && made.len() < 29, "{made:?}");
    assert!(!is_done(&dir));
    assert!(passes.failed_recently(played, Instant::now()));
    assert!(
        !passes.failed_recently(other.stream_id, Instant::now()),
        "giving way is not failing"
    );
}

#[tokio::test]
async fn another_version_of_another_size_is_not_read() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let set_id = Uuid::new_v4();
    let others = [other_source("other-a"), other_source("other-b")];
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        dead: strings(&[PLAYED]),
        wrong_size: strings(&["other-a"]),
        ..Default::default()
    });
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            plan_with(tmp.path(), set_id, &others),
            runner.clone(),
            always(),
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.sized(), strings(&["other-a", "other-b"]));
    assert_eq!(runner.inputs(), strings(&[PLAYED, "other-b"]));
    assert!(passes.failed_recently(others[0].stream_id, Instant::now() + FAILURE_WAIT));
    assert!(is_done(&set_dir(tmp.path(), set_id)));
}

#[tokio::test]
async fn another_version_longer_than_its_count_is_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    // 940 pictures fill sheets 0 to 37; other-a holds sheet 38 too.
    let set_id = Uuid::new_v4();
    let others = [other_source("other-a"), other_source("other-b")];
    let runner = Arc::new(FakeRunner {
        sheets: 38,
        dead: strings(&[PLAYED]),
        long: strings(&["other-a"]),
        ..Default::default()
    });
    let plan = PassPlan {
        pictures: 940,
        ..plan_with(tmp.path(), set_id, &others)
    };
    let played = plan.stream_id;
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin("play-a", plan, runner.clone(), always())
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.inputs(), strings(&[PLAYED, "other-a", "other-b"]));
    let own = set_dir(tmp.path(), others[0].stream_id);
    assert!(
        own.join("38.jpg")
            .exists(),
        "other-a's pictures are in a folder of its own"
    );
    assert!(passes.failed_recently(others[0].stream_id, Instant::now() + FAILURE_WAIT));
    let dir = set_dir(tmp.path(), set_id);
    assert!(is_done(&dir));
    assert_eq!(
        sheets_in(&dir),
        (0..38).collect::<Vec<u32>>(),
        "made again from other-b"
    );

    // After a restart forgets every wait, other-a's own folder still
    // keeps it from being read in the version played's place.
    let restarted = TrickplayPasses::with_limits(fast());
    let other_set = Uuid::new_v4();
    let plan = PassPlan {
        stream_id: played,
        pictures: 940,
        ..plan_with(tmp.path(), other_set, &others[..1])
    };
    restarted.mark_failed(played, Instant::now() + FAILURE_WAIT);
    restarted.register("play-b", Uuid::new_v4(), "device-b");
    let begun = restarted
        .begin("play-b", plan, runner.clone(), always())
        .await;
    assert_eq!(begun, Begin::FailedRecently);
}

#[tokio::test]
async fn a_version_that_is_not_read_is_never_opened() {
    let tmp = tempfile::tempdir().unwrap();
    let set_id = Uuid::new_v4();
    let (a, b) = (other_source("other-a"), other_source("other-b"));

    // Read: the plan is left as it was.
    let plan = read_other_if_refused(plan_with(tmp.path(), set_id, &[a.clone()]), None)
        .unwrap();
    assert_eq!(plan.input, PLAYED);
    assert!(
        plan.other_version
            .is_none()
    );
    assert_eq!(
        plan.others
            .len(),
        1
    );

    // Not read, with no other version: no pass.
    let refused = Some(Ineligible::AddonExcluded);
    let none = read_other_if_refused(plan_with(tmp.path(), set_id, &[]), refused);
    assert_eq!(none.err(), refused);

    // Not read, with other versions: the first is read, the rest follow.
    let plan = read_other_if_refused(
        plan_with(tmp.path(), set_id, &[a.clone(), b.clone()]),
        Some(Ineligible::Iptv),
    )
    .unwrap();
    assert_eq!(plan.stream_id, a.stream_id);
    assert_eq!(plan.input, "other-a");
    assert_eq!(plan.size, Some(LISTED));
    assert!(
        plan.other_version
            .is_some()
    );
    assert_eq!(plan.set_id, set_id, "the set stays the version played's");
    assert_eq!(
        plan.others
            .iter()
            .map(|s| s.stream_id)
            .collect::<Vec<_>>(),
        vec![b.stream_id]
    );

    let passes = TrickplayPasses::with_limits(fast());
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        dead: strings(&["other-a"]),
        ..Default::default()
    });
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin("play-a", plan, runner.clone(), always())
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.inputs(), strings(&["other-a", "other-b"]));
    assert!(
        !runner
            .sized()
            .contains(&PLAYED.to_string())
    );
    assert!(is_done(&set_dir(tmp.path(), set_id)));
}

#[tokio::test]
async fn a_resumed_version_whose_source_ends_at_once_goes_on_to_another() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let reports = Reports::default();
    reports.at("device-a", 20 * SHEET_SECS);
    let set_id = Uuid::new_v4();
    let dir = set_dir(tmp.path(), set_id);
    write_sheets(&dir, 0..10);
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        empty: strings(&[PLAYED]),
        ..Default::default()
    });
    let plan = plan_with(tmp.path(), set_id, &[other_source("other-a")]);
    let played = plan.stream_id;
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin("play-a", plan, runner.clone(), reports.watch())
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| !passes.is_queued(set_id)).await;
    // Both runs of the version played start above sheet 0 and end empty.
    let inputs = runner.inputs();
    assert_eq!(
        inputs[..2].to_vec(),
        strings(&[PLAYED, PLAYED]),
        "{inputs:?}"
    );
    assert_eq!(runner.starts()[..2].to_vec(), vec![20, 10]);
    assert!(
        inputs.len() > 2
            && inputs[2..]
                .iter()
                .all(|i| i == "other-a"),
        "{inputs:?}"
    );
    assert!(passes.failed_recently(played, Instant::now()));
    assert!(is_done(&dir));
    assert_eq!(sheets_in(&dir), (0..29).collect::<Vec<u32>>());

    // With no other version, the pass ends as it did, and the version
    // played does not rest.
    let set_id = Uuid::new_v4();
    let dir = set_dir(tmp.path(), set_id);
    write_sheets(&dir, 0..10);
    let plan = plan_with(tmp.path(), set_id, &[]);
    let played = plan.stream_id;
    passes.register("play-b", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin("play-b", plan, runner.clone(), reports.watch())
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| !passes.is_queued(set_id)).await;
    assert!(!passes.failed_recently(played, Instant::now()));
    assert!(!is_done(&dir));
    assert_eq!(sheets_in(&dir), (0..10).collect::<Vec<u32>>());
}

#[tokio::test]
async fn another_version_that_ends_short_leaves_none_of_its_pictures() {
    // A clean end short of the count: a shorter file. Its sheets are
    // deleted and the next version makes every one of them.
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let set_id = Uuid::new_v4();
    let others = [other_source("other-a"), other_source("other-b")];
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        dead: strings(&[PLAYED]),
        short: strings(&["other-a"]),
        ..Default::default()
    });
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            plan_with(tmp.path(), set_id, &others),
            runner.clone(),
            always(),
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.inputs(), strings(&[PLAYED, "other-a", "other-b"]));
    assert_eq!(runner.starts(), vec![0, 0, 0], "other-b made every sheet");
    assert!(passes.failed_recently(others[0].stream_id, Instant::now() + FAILURE_WAIT));
    let dir = set_dir(tmp.path(), set_id);
    assert!(is_done(&dir));
    assert_eq!(sheets_in(&dir), (0..29).collect::<Vec<u32>>());

    // A source that fails part way: its sheets are kept and built on.
    let set_id = Uuid::new_v4();
    let others = [other_source("other-c"), other_source("other-d")];
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        dead: strings(&[PLAYED]),
        cut: strings(&["other-c"]),
        ..Default::default()
    });
    passes.register("play-b", Uuid::new_v4(), "device-b");
    passes
        .begin(
            "play-b",
            plan_with(tmp.path(), set_id, &others),
            runner.clone(),
            always(),
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.inputs(), strings(&[PLAYED, "other-c", "other-d"]));
    // other-c made sheets 0 to 13; the last, maybe partly filled, went.
    assert_eq!(runner.starts(), vec![0, 0, 13]);
    assert!(is_done(&set_dir(tmp.path(), set_id)));
}

#[test]
fn an_unclean_end_drops_the_last_sheet_and_writes_no_end_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let stream_id = Uuid::new_v4();
    let plan = plan(tmp.path(), stream_id);
    let mut dir = set_dir(tmp.path(), stream_id);
    // 720 pictures: sheets 0 to 28, all made by one run.
    write_sheets(&dir, 0..29);
    let run = Run {
        start: 0,
        stop_after: None,
    };
    let result = RunResult {
        exit_ok: true,
        exit_code: Some(0),
        made_sheets: 29,
        stderr: UNKNOWN_LINE.to_string(),
        ..Default::default()
    };
    let end = run_end(&result, run, plan.pictures);
    assert_eq!(end, RunEnd::Unclean);
    assert!(!passes.after_run(&plan, &mut dir, run, &result, end));
    assert!(
        !dir.join(END_MARKER)
            .exists()
    );
    assert_eq!(sheets_in(&dir), (0..28).collect::<Vec<u32>>());
    assert!(
        !passes.failed_recently(stream_id, Instant::now()),
        "not a failure: the next play reads the end of this version again"
    );

    // A file longer than its stored length, in a set keyed by its count:
    // the key check still moves its pictures to a folder of its own.
    let stream_id = Uuid::new_v4();
    let set_id = Uuid::new_v4();
    let plan = set_plan(tmp.path(), stream_id, set_id);
    let shared = set_dir(tmp.path(), set_id);
    let mut dir = shared.clone();
    // Sheet 29 is past the count's last, 28.
    write_sheets(&dir, 0..30);
    let result = RunResult {
        made_sheets: 30,
        ..result
    };
    let end = run_end(&result, run, plan.pictures);
    assert_eq!(end, RunEnd::Unclean);
    assert!(passes.after_run(&plan, &mut dir, run, &result, end));
    assert_eq!(dir, set_dir(tmp.path(), stream_id));
    assert!(!shared.exists(), "nothing of it is left in the shared set");
    assert!(
        !dir.join(END_MARKER)
            .exists()
    );
    assert_eq!(sheets_in(&dir), (0..29).collect::<Vec<u32>>());
}

#[tokio::test]
async fn a_run_that_stalls_frees_the_slot_and_counts_as_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(Limits {
        stall_wait: Duration::from_millis(100),
        ..fast()
    });
    // Makes sheets 0 to 4, then the source stops sending without closing.
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        stall_at: Some(5),
        ..Default::default()
    });
    let stream_id = Uuid::new_v4();
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin(
            "play-a",
            plan(tmp.path(), stream_id),
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    until_named("the stalled run stopped", || {
        runner
            .killed
            .load(Ordering::SeqCst)
    })
    .await;
    until(|| !passes.is_queued(stream_id)).await;
    assert_eq!(
        runner
            .runs
            .load(Ordering::SeqCst),
        1,
        "no run after it"
    );
    assert!(passes.failed_recently(stream_id, Instant::now()));
    let dir = set_dir(tmp.path(), stream_id);
    assert_eq!(
        sheets_in(&dir),
        (0..5).collect::<Vec<u32>>(),
        "the sheets made are kept"
    );
    assert!(!is_done(&dir));
    assert!(
        passes
            .inner
            .slots
            .try_acquire()
            .is_ok(),
        "the slot is free"
    );
}

#[tokio::test]
async fn a_play_that_joined_has_its_version_read_when_the_first_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let set_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        dead: strings(&["version-a"]),
        ..Default::default()
    });
    let b = join_two_versions(
        &passes,
        tmp.path(),
        set_id,
        &runner,
        version_b(tmp.path(), set_id),
    )
    .await;
    assert_eq!(runner.inputs(), strings(&["version-a", "version-b"]));
    let dir = set_dir(tmp.path(), set_id);
    assert!(is_done(&dir));
    assert_eq!(sheets_in(&dir), (0..29).collect::<Vec<u32>>());
    assert!(!passes.failed_recently(b, Instant::now()));
}

#[tokio::test]
async fn a_play_that_joined_has_its_version_read_when_the_first_moves_away() {
    // 940 pictures fill sheets 0 to 37; version-a holds sheet 38 too, so
    // its pictures move to a folder of its own and the set is left to
    // make.
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let set_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner {
        sheets: 38,
        long: strings(&["version-a"]),
        ..Default::default()
    });
    let joined = PassPlan {
        pictures: 940,
        ..version_b(tmp.path(), set_id)
    };
    join_two_versions(&passes, tmp.path(), set_id, &runner, joined).await;
    assert_eq!(runner.inputs(), strings(&["version-a", "version-b"]));
    let dir = set_dir(tmp.path(), set_id);
    assert!(is_done(&dir));
    assert_eq!(
        sheets_in(&dir),
        (0..38).collect::<Vec<u32>>(),
        "made from version-b"
    );
}

#[tokio::test]
async fn a_joined_version_whose_addon_was_switched_off_is_not_read() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let mut source = addon("stremio", false);
    passes.set_addon_rules(std::slice::from_ref(&source));
    let set_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        dead: strings(&["version-a"]),
        ..Default::default()
    });
    let joined = PassPlan {
        played: StreamInfo {
            descriptor: StreamDescriptor::http(URL),
            addon_id: Some(source.id),
            ..Default::default()
        },
        ..version_b(tmp.path(), set_id)
    };
    let held = passes
        .inner
        .slots
        .acquire()
        .await
        .unwrap();
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let first = passes
        .begin(
            "play-a",
            PassPlan {
                input: "version-a".to_string(),
                ..set_plan(tmp.path(), Uuid::new_v4(), set_id)
            },
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(first, Begin::Queued);
    passes.register("play-b", Uuid::new_v4(), "device-b");
    let second = passes
        .begin("play-b", joined, runner.clone(), always())
        .await;
    assert_eq!(second, Begin::Joined);
    // Switched off once version-b's play has joined, before the slot is
    // free.
    source.resources = vec![ResourceType::Stream];
    passes.set_addon_rule(&source);
    drop(held);
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(
        runner.inputs(),
        strings(&["version-a"]),
        "version-b is never opened"
    );
    assert!(!is_done(&set_dir(tmp.path(), set_id)));
}

#[tokio::test]
async fn a_joined_play_that_ended_has_its_version_left_unread() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let set_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        dead: strings(&["version-a"]),
        ..Default::default()
    });
    let held = passes
        .inner
        .slots
        .acquire()
        .await
        .unwrap();
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let first = passes
        .begin(
            "play-a",
            PassPlan {
                input: "version-a".to_string(),
                ..set_plan(tmp.path(), Uuid::new_v4(), set_id)
            },
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(first, Begin::Queued);
    passes.register("play-b", Uuid::new_v4(), "device-b");
    let second = passes
        .begin(
            "play-b",
            version_b(tmp.path(), set_id),
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(second, Begin::Joined);
    // Version-b's play ends before the slot is free.
    passes.release("play-b");
    drop(held);
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(
        runner.inputs(),
        strings(&["version-a"]),
        "nobody plays version-b any more"
    );
    assert!(!is_done(&set_dir(tmp.path(), set_id)));
}

#[tokio::test]
async fn a_stand_in_refused_while_it_reads_leaves_none_of_its_sheets() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = settings_db(true, 1024).await;
    let cfg = api::ServerConfiguration {
        trickplay_while_streaming: Some(true),
        trickplay_cache_size_mb: Some(1024),
        ..Default::default()
    };
    db::Settings::set_config(&pool, &cfg)
        .await
        .unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let mut source = addon("stremio", false);
    passes.set_addon_rules(std::slice::from_ref(&source));
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        wait_for_cancel: true,
        ..Default::default()
    });
    let set_id = Uuid::new_v4();
    let mut stand_in = other_source("other-a");
    stand_in
        .info
        .addon_id = Some(source.id);
    let plan = PassPlan {
        db: Some(pool.clone()),
        ..plan_with(tmp.path(), set_id, std::slice::from_ref(&stand_in))
    };
    // The version played failed lately, so other-a is read first.
    passes.mark_failed(plan.stream_id, Instant::now() + FAILURE_WAIT);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin("play-a", plan, runner.clone(), always())
        .await;
    assert_eq!(begun, Begin::Queued);
    let dir = set_dir(tmp.path(), set_id);
    until_named("other-a made its sheets", || sheets_in(&dir).len() == 29).await;
    // Its addon's previews are turned off while its run waits at the end.
    source.resources = vec![ResourceType::Stream];
    passes.set_addon_rule(&source);
    until_named("the run stopped", || {
        runner
            .killed
            .load(Ordering::SeqCst)
    })
    .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.inputs(), strings(&["other-a"]));
    assert!(
        sheets_in(&dir).is_empty(),
        "its length was never checked: {:?}",
        sheets_in(&dir)
    );
    assert!(
        !passes.failed_recently(stand_in.stream_id, Instant::now()),
        "not a failure"
    );
}
