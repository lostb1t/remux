//! Addons kept for seek previews only, and lengths read from the header.

use super::*;

#[test]
fn the_addons_are_asked_only_for_a_measured_version_with_one_enabled() {
    let root = Path::new("/srv/trickplay");
    let title = Uuid::new_v4();
    let on = addon("stremio", false);
    let dedicated = preview_addon();
    let with = rules_of(&[&on, &dedicated]);
    let without = rules_of(&[&on]);
    let playing = version(title, 7_200.0, on.id, 0);
    let set = set_id(&playing);
    let user = Uuid::new_v4();

    let ask = preview_ask(&playing, set, root, user, &with).expect("asked");
    assert_eq!(ask.title_id, title);
    assert_eq!(ask.user_id, user);
    assert!(preview_ask(&playing, set, root, user, &without).is_none());
    // Not measured: its set is its own, and no other file can be known
    // to fit.
    let unmeasured = db::Media {
        probe_data: None,
        ..playing.clone()
    };
    assert!(preview_ask(&unmeasured, set_id(&unmeasured), root, user, &with).is_none());
    // A version that keeps a folder of its own.
    assert!(preview_ask(&playing, playing.id, root, user, &with).is_none());
    // Not under a title.
    let loose = db::Media {
        parent_id: None,
        ..playing.clone()
    };
    assert!(preview_ask(&loose, set, root, user, &with).is_none());
}

#[tokio::test]
async fn the_finder_is_not_called_without_a_source_a_play_or_work_to_do() {
    let tmp = tempfile::tempdir().unwrap();
    let dedicated = preview_addon();
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        ..Default::default()
    });

    // No addon kept for seek previews only is enabled any more.
    let passes = TrickplayPasses::with_limits(fast());
    let finder = FakeFinder {
        sources: vec![preview_source("preview-a", &dedicated)],
        ..Default::default()
    };
    let set_id = Uuid::new_v4();
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin_with(
            "play-a",
            preview_plan(tmp.path(), set_id, &[]),
            runner.clone(),
            always(),
            &finder,
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(finder.calls(), 0);
    assert_eq!(runner.inputs(), strings(&[PLAYED]));

    // A play that ended within the start delay.
    let passes = passes_with(&dedicated);
    passes.register("play-b", Uuid::new_v4(), "device-b");
    let begun = passes
        .begin_with(
            "play-b",
            preview_plan(tmp.path(), Uuid::new_v4(), &[]),
            runner.clone(),
            never(),
            &finder,
        )
        .await;
    assert_eq!(begun, Begin::Released);
    assert_eq!(finder.calls(), 0);

    // A complete set.
    let done = Uuid::new_v4();
    let dir = set_dir(tmp.path(), done);
    write_sheets(&dir, 0..29);
    std::fs::write(dir.join(DONE_MARKER), b"").unwrap();
    passes.register("play-c", Uuid::new_v4(), "device-c");
    let begun = passes
        .begin_with(
            "play-c",
            preview_plan(tmp.path(), done, &[]),
            runner.clone(),
            always(),
            &finder,
        )
        .await;
    assert_eq!(begun, Begin::Done);
    assert_eq!(finder.calls(), 0);

    // A plan that does not ask (see `preview_ask`).
    let plain = Uuid::new_v4();
    passes.register("play-d", Uuid::new_v4(), "device-d");
    passes
        .begin_with(
            "play-d",
            plan_with(tmp.path(), plain, &[]),
            runner.clone(),
            always(),
            &finder,
        )
        .await;
    until(|| !passes.is_queued(plain)).await;
    assert_eq!(finder.calls(), 0);
}

#[tokio::test]
async fn a_preview_source_is_read_first_and_the_version_played_is_never_opened() {
    let tmp = tempfile::tempdir().unwrap();
    let dedicated = preview_addon();
    let passes = passes_with(&dedicated);
    let set_id = Uuid::new_v4();
    let finder = FakeFinder {
        sources: vec![preview_source("preview-a", &dedicated)],
        ..Default::default()
    };
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        ..Default::default()
    });
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin_with(
            "play-a",
            preview_plan(tmp.path(), set_id, &[other_source("other-a")]),
            runner.clone(),
            always(),
            &finder,
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(finder.calls(), 1);
    assert_eq!(runner.inputs(), strings(&["preview-a"]));
    assert_eq!(
        runner.sized(),
        strings(&["preview-a"]),
        "its size is checked as another version's is"
    );
    let dir = set_dir(tmp.path(), set_id);
    assert!(is_done(&dir));
    assert_eq!(sheets_in(&dir), (0..29).collect::<Vec<u32>>());
}

#[tokio::test]
async fn the_version_played_follows_two_preview_sources_and_three_files_at_most() {
    let tmp = tempfile::tempdir().unwrap();
    let dedicated = preview_addon();
    let passes = passes_with(&dedicated);
    let set_id = Uuid::new_v4();
    let found = [
        preview_source("preview-a", &dedicated),
        preview_source("preview-b", &dedicated),
        preview_source("preview-c", &dedicated),
    ];
    let finder = FakeFinder {
        sources: found.to_vec(),
        ..Default::default()
    };

    // Two preview sources fail: the version played is read next.
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        dead: strings(&["preview-a", "preview-b"]),
        ..Default::default()
    });
    let plan = preview_plan(tmp.path(), set_id, &[other_source("other-a")]);
    let played = plan.stream_id;
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin_with("play-a", plan, runner.clone(), always(), &finder)
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(
        runner.inputs(),
        strings(&["preview-a", "preview-b", PLAYED]),
        "never a third preview source"
    );
    assert!(is_done(&set_dir(tmp.path(), set_id)));
    let now = Instant::now();
    assert!(
        passes.failed_recently(found[0].stream_id, now + FAILURE_WAIT),
        "a preview source waits as another version does"
    );
    assert!(!passes.failed_recently(played, now));

    // Everything fails: three files are opened, no more.
    let passes = passes_with(&dedicated);
    let set_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        dead: strings(&["preview-a", "preview-b", "preview-c", PLAYED, "other-a"]),
        ..Default::default()
    });
    let plan = preview_plan(tmp.path(), set_id, &[other_source("other-a")]);
    let played = plan.stream_id;
    passes.register("play-b", Uuid::new_v4(), "device-b");
    passes
        .begin_with("play-b", plan, runner.clone(), always(), &finder)
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(
        runner.inputs(),
        strings(&["preview-a", "preview-b", PLAYED])
    );
    let now = Instant::now();
    assert!(passes.failed_recently(played, now));
    assert!(
        !passes.failed_recently(played, now + FAILURE_WAIT + Duration::from_secs(1)),
        "the version played waits as it always does"
    );
}

#[tokio::test]
async fn a_version_played_its_addon_refuses_gets_pictures_only_from_a_preview_source() {
    let tmp = tempfile::tempdir().unwrap();
    let dedicated = preview_addon();
    let passes = passes_with(&dedicated);
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        ..Default::default()
    });
    let refused = |set_id: Uuid| PassPlan {
        played_refused: Some(Ineligible::AddonExcluded),
        ..preview_plan(tmp.path(), set_id, &[])
    };

    let set_id = Uuid::new_v4();
    let finder = FakeFinder {
        sources: vec![
            preview_source("preview-a", &dedicated),
            preview_source("preview-b", &dedicated),
        ],
        ..Default::default()
    };
    let runner_dead = Arc::new(FakeRunner {
        sheets: 29,
        dead: strings(&["preview-a", "preview-b"]),
        ..Default::default()
    });
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin_with(
            "play-a",
            refused(set_id),
            runner_dead.clone(),
            always(),
            &finder,
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(
        runner_dead.inputs(),
        strings(&["preview-a", "preview-b"]),
        "the version played is never opened"
    );

    let set_id = Uuid::new_v4();
    let finder = FakeFinder {
        sources: vec![preview_source("preview-c", &dedicated)],
        ..Default::default()
    };
    passes.register("play-b", Uuid::new_v4(), "device-b");
    passes
        .begin_with("play-b", refused(set_id), runner.clone(), always(), &finder)
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.inputs(), strings(&["preview-c"]));
    assert!(is_done(&set_dir(tmp.path(), set_id)));

    // Nothing found, nothing listed: no pass.
    let finder = FakeFinder {
        other_lengths: 2,
        ..Default::default()
    };
    passes.register("play-c", Uuid::new_v4(), "device-c");
    let begun = passes
        .begin_with(
            "play-c",
            refused(Uuid::new_v4()),
            runner.clone(),
            always(),
            &finder,
        )
        .await;
    assert_eq!(begun, Begin::NoSource);
    assert_eq!(finder.calls(), 1);

    // Nothing found: another listed version stands in, as before.
    let set_id = Uuid::new_v4();
    passes.register("play-d", Uuid::new_v4(), "device-d");
    passes
        .begin_with(
            "play-d",
            PassPlan {
                others: vec![other_source("other-a")],
                ..refused(set_id)
            },
            runner.clone(),
            always(),
            &finder,
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.inputs(), strings(&["preview-c", "other-a"]));
}

#[test]
fn preview_sources_go_first_and_the_version_played_behind_them() {
    let tmp = tempfile::tempdir().unwrap();
    let dedicated = preview_addon();
    let found = Found {
        sources: vec![
            preview_source("preview-a", &dedicated),
            preview_source("preview-b", &dedicated),
            preview_source("preview-c", &dedicated),
        ],
        other_lengths: 1,
    };
    let other = other_source("other-a");
    let mut plan = preview_plan(tmp.path(), Uuid::new_v4(), &[other.clone()]);
    let played = plan.stream_id;
    assert_eq!(put_preview_sources(&mut plan, found.clone()), Ok(true));
    let order: Vec<(String, SourceKind)> = plan
        .others
        .iter()
        .map(|s| {
            (
                s.input
                    .clone(),
                s.kind,
            )
        })
        .collect();
    assert_eq!(
        order,
        vec![
            ("preview-a".to_string(), SourceKind::Preview),
            ("preview-b".to_string(), SourceKind::Preview),
            (PLAYED.to_string(), SourceKind::Played),
            ("other-a".to_string(), SourceKind::Other),
        ]
    );
    assert_eq!(plan.others[2].stream_id, played);
    assert_eq!(plan.other_lengths, 1);

    // Read again behind them, the version played is read as itself.
    let mut read = plan.clone();
    read.read_other(plan.others[0].clone());
    assert!(
        read.other_version
            .is_some()
    );
    read.read_other(plan.others[2].clone());
    assert_eq!(read.stream_id, played);
    assert!(
        read.other_version
            .is_none()
    );

    // Refused: not behind them.
    let mut plan = PassPlan {
        played_refused: Some(Ineligible::AddonExcluded),
        ..preview_plan(tmp.path(), Uuid::new_v4(), &[])
    };
    assert_eq!(put_preview_sources(&mut plan, found), Ok(true));
    assert!(
        plan.others
            .iter()
            .all(|s| s.kind == SourceKind::Preview)
    );
    assert!(
        plan.played_refused
            .is_none()
    );

    // None found: as without them.
    let mut plan = preview_plan(tmp.path(), Uuid::new_v4(), &[other.clone()]);
    assert_eq!(put_preview_sources(&mut plan, Found::default()), Ok(false));
    assert_eq!(plan.input, PLAYED);
    assert_eq!(
        plan.others
            .len(),
        1
    );
    let mut plan = PassPlan {
        played_refused: Some(Ineligible::Redirect),
        ..preview_plan(tmp.path(), Uuid::new_v4(), &[])
    };
    assert_eq!(
        put_preview_sources(&mut plan, Found::default()),
        Err(Ineligible::Redirect)
    );
}

#[test]
fn preview_sources_are_read_in_the_addons_order_not_by_size() {
    let title = Uuid::new_v4();
    let on = addon("stremio", false);
    let first = Addon {
        priority: 1,
        ..preview_addon()
    };
    let second = Addon {
        priority: 2,
        ..preview_addon()
    };
    let rules = rules_of(&[&on, &first, &second]);
    let playing = version(title, 7_200.0, on.id, 0);
    let mut big = version(title, 7_200.0, first.id, 0);
    info_of(&mut big).size = Some(LISTED as i64 * 4);
    let small = version(title, 7_200.0, first.id, 1);
    let mut local = version(title, 7_200.0, second.id, 0);
    info_of(&mut local).descriptor =
        StreamDescriptor::Local("/media/invented-film.mkv".into());
    let rows = [local.clone(), small.clone(), big.clone()];
    let order: Vec<Uuid> = preview_versions(&playing, &rows, None, &rules, |_| false)
        .iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(order, vec![big.id, small.id, local.id]);

    // A version a viewer is offered is never a preview source.
    let listed = version(title, 7_200.0, on.id, 0);
    assert!(preview_versions(&playing, &[listed], None, &rules, |_| false).is_empty());
}

#[test]
fn preview_sources_that_cannot_stand_in_are_never_read() {
    let title = Uuid::new_v4();
    let on = addon("stremio", false);
    let dedicated = preview_addon();
    let rules = rules_of(&[&on, &dedicated]);
    let playing = version(title, 7_200.0, on.id, 0);
    let none_own = |_: Uuid| false;
    let fits = version(title, 7_200.0, dedicated.id, 0);
    assert_eq!(
        preview_versions(
            &playing,
            std::slice::from_ref(&fits),
            None,
            &rules,
            none_own
        )
        .len(),
        1
    );

    let mut refused: Vec<(&str, db::Media)> =
        vec![("another length", version(title, 7_300.0, dedicated.id, 0))];
    let mut row = version(title, 7_200.0, dedicated.id, 0);
    info_of(&mut row).service_cached = None;
    refused.push(("not cached", row));
    let mut row = version(title, 7_200.0, dedicated.id, 0);
    info_of(&mut row).service_id = None;
    refused.push(("no named service", row));
    let mut row = version(title, 7_200.0, dedicated.id, 0);
    info_of(&mut row).size = None;
    refused.push(("no size", row));
    let mut row = version(title, 7_200.0, dedicated.id, 0);
    info_of(&mut row).descriptor = StreamDescriptor::Torrent {
        info_hash: "0123456789abcdef0123456789abcdef01234567".to_string(),
        file_hint: None,
        file_idx: None,
        trackers: vec![],
    };
    refused.push(("a torrent", row));
    refused.push((
        "another title",
        version(Uuid::new_v4(), 7_200.0, dedicated.id, 0),
    ));
    for (why, row) in &refused {
        assert!(
            preview_versions(
                &playing,
                std::slice::from_ref(row),
                None,
                &rules,
                none_own
            )
            .is_empty(),
            "{why}"
        );
    }
    let lengths = other_lengths(
        &playing,
        &[version(title, 7_300.0, dedicated.id, 0), fits.clone()],
        None,
        &rules,
        none_own,
    );
    assert_eq!(lengths, 1, "only the one of another length");

    // Its addon given "stream" again, switched off, or deleted.
    for (why, rules) in [
        (
            "stream on",
            rules_of(&[
                &on,
                &Addon {
                    resources: vec![ResourceType::Stream, ResourceType::SeekPreviews],
                    ..dedicated.clone()
                },
            ]),
        ),
        ("switched off", rules_of(&[&on, &switched_off(&dedicated)])),
        ("deleted", rules_of(&[&on])),
    ] {
        assert!(
            preview_versions(
                &playing,
                std::slice::from_ref(&fits),
                None,
                &rules,
                none_own
            )
            .is_empty(),
            "{why}"
        );
    }

    // One with no length yet is worth measuring; a measured one is not.
    let unmeasured = db::Media {
        probe_data: None,
        ..version(title, 7_200.0, dedicated.id, 1)
    };
    let rows = [fits.clone(), unmeasured.clone()];
    let to_measure: Vec<Uuid> =
        preview_unmeasured(&rows, Some(7_200), &rules, none_own)
            .iter()
            .map(|m| m.id)
            .collect();
    assert_eq!(to_measure, vec![unmeasured.id]);
}

#[test]
fn an_answer_keeps_the_first_five_it_can_read_in_its_order() {
    let film = db::Media {
        id: Uuid::new_v4(),
        title: "Invented Film".to_string(),
        kind: db::MediaKind::Movie,
        runtime: Some(7_200),
        ..Default::default()
    };
    let cached = |name: &str| StreamInfo {
        descriptor: StreamDescriptor::http(format!(
            "http://example.invalid/{name}.mkv"
        )),
        filename: Some(format!("{name}.mkv")),
        service_id: Some("svc-cached".to_string()),
        service_cached: Some(true),
        size: Some(LISTED as i64),
        ..Default::default()
    };
    let answer = vec![
        StreamInfo {
            service_cached: None,
            ..cached("uncached")
        },
        cached("a"),
        StreamInfo {
            descriptor: StreamDescriptor::Torrent {
                info_hash: "0123456789abcdef0123456789abcdef01234567".to_string(),
                file_hint: None,
                file_idx: None,
                trackers: vec![],
            },
            ..cached("torrent")
        },
        cached("b"),
        cached("a"),
        StreamInfo {
            size: None,
            ..cached("unsized")
        },
        StreamInfo {
            service_id: None,
            ..cached("no-service")
        },
        StreamInfo {
            descriptor: StreamDescriptor::Local("/media/invented-film.mkv".into()),
            size: None,
            ..cached("local")
        },
        cached("c"),
        cached("d"),
        cached("e"),
    ];
    let kept = kept_preview_streams(&film, answer);
    let names: Vec<String> = kept
        .iter()
        .filter_map(|s| {
            s.stream_info
                .filename
                .clone()
        })
        .collect();
    assert_eq!(names, ["a.mkv", "b.mkv", "local.mkv", "c.mkv", "d.mkv"]);
    assert_ne!(kept[0].content_key, kept[1].content_key);
}

#[test]
fn an_ask_that_kept_nothing_holds_the_title_back_until_it_may_be_asked_again() {
    let passes = TrickplayPasses::new();
    let dedicated = preview_addon();
    passes.set_addon_rules(std::slice::from_ref(&dedicated));
    let rules = passes.addon_rules();
    let (title, other_addon) = (Uuid::new_v4(), Uuid::new_v4());

    assert!(!passes.kept_nothing(title), "never asked");
    assert!(preview_source_may_help(&rules, &passes, title));
    assert!(!passes.asked_recently(title, dedicated.id));

    passes.mark_asked(title, dedicated.id, Some(0));
    assert!(
        passes.asked_recently(title, dedicated.id),
        "not asked again"
    );
    assert!(passes.kept_nothing(title));
    assert!(
        !preview_source_may_help(&rules, &passes, title),
        "its item lists no version only a preview source could help"
    );
    assert!(!passes.kept_nothing(Uuid::new_v4()));

    passes.mark_asked(title, other_addon, Some(3));
    assert!(!passes.kept_nothing(title), "another addon kept something");

    // A failed ask waits the short time; an answer the long one.
    let failed = Uuid::new_v4();
    passes.mark_asked(failed, dedicated.id, None);
    assert!(passes.asked_recently(failed, dedicated.id));
    assert!(passes.kept_nothing(failed));
    let asked_until = |title: Uuid| {
        passes
            .lock()
            .asked
            .get(&(title, dedicated.id))
            .map(|asked| asked.until)
            .unwrap()
    };
    let now = Instant::now();
    assert!(asked_until(failed) <= now + PREVIEW_RETRY_WAIT);
    assert!(asked_until(failed) > now + PREVIEW_RETRY_WAIT - Duration::from_secs(60));
    assert!(asked_until(title) > now + PREVIEW_ASKED_WAIT - Duration::from_secs(60));

    // With no preview source enabled, nothing is listed for one.
    let none = TrickplayPasses::new();
    assert!(!preview_source_may_help(&none.addon_rules(), &none, title));
}

#[tokio::test]
async fn a_preview_sources_wrong_size_or_short_end_is_handled_as_another_versions() {
    let tmp = tempfile::tempdir().unwrap();
    let dedicated = preview_addon();
    let passes = passes_with(&dedicated);
    let finder = FakeFinder {
        sources: vec![
            preview_source("preview-a", &dedicated),
            preview_source("preview-b", &dedicated),
        ],
        ..Default::default()
    };

    // Its source holds another file: not read.
    let set_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        wrong_size: strings(&["preview-a"]),
        ..Default::default()
    });
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin_with(
            "play-a",
            preview_plan(tmp.path(), set_id, &[]),
            runner.clone(),
            always(),
            &finder,
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.sized(), strings(&["preview-a", "preview-b"]));
    assert_eq!(runner.inputs(), strings(&["preview-b"]));
    assert!(is_done(&set_dir(tmp.path(), set_id)));

    // A shorter file: its sheets go and the next makes every one.
    let passes = passes_with(&dedicated);
    let set_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        short: strings(&["preview-a"]),
        ..Default::default()
    });
    passes.register("play-b", Uuid::new_v4(), "device-b");
    passes
        .begin_with(
            "play-b",
            preview_plan(tmp.path(), set_id, &[]),
            runner.clone(),
            always(),
            &finder,
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.inputs(), strings(&["preview-a", "preview-b"]));
    assert_eq!(runner.starts(), vec![0, 0], "preview-b made every sheet");
    let dir = set_dir(tmp.path(), set_id);
    assert!(is_done(&dir));
    assert_eq!(sheets_in(&dir), (0..29).collect::<Vec<u32>>());
}

#[tokio::test]
async fn a_preview_source_whose_addon_is_switched_off_while_it_waits_is_not_read() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = settings_db(true, 1024).await;
    let dedicated = preview_addon();
    let passes = passes_with(&dedicated);
    let finder = FakeFinder {
        sources: vec![preview_source("preview-a", &dedicated)],
        ..Default::default()
    };
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        ..Default::default()
    });
    let held = passes
        .inner
        .slots
        .acquire()
        .await
        .unwrap();
    let set_id = Uuid::new_v4();
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin_with(
            "play-a",
            PassPlan {
                db: Some(pool.clone()),
                ..preview_plan(tmp.path(), set_id, &[])
            },
            runner.clone(),
            always(),
            &finder,
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    passes.set_addon_rule(&switched_off(&dedicated));
    drop(held);
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(
        runner.inputs(),
        strings(&[PLAYED]),
        "preview-a is never opened; the version played is read"
    );
}

#[tokio::test]
async fn the_finder_is_not_called_once_the_switch_is_off() {
    let tmp = tempfile::tempdir().unwrap();
    // Turned off after the play started, within its start delay.
    let pool = settings_db(false, 1024).await;
    let dedicated = preview_addon();
    let passes = passes_with(&dedicated);
    let finder = FakeFinder {
        sources: vec![preview_source("preview-a", &dedicated)],
        ..Default::default()
    };
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        ..Default::default()
    });
    let set_id = Uuid::new_v4();
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin_with(
            "play-a",
            PassPlan {
                db: Some(pool.clone()),
                ..preview_plan(tmp.path(), set_id, &[])
            },
            runner.clone(),
            always(),
            &finder,
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(finder.calls(), 0, "the addon is not asked");
    assert!(
        runner
            .inputs()
            .is_empty(),
        "nothing is read"
    );
}

#[tokio::test]
async fn a_preview_source_switched_off_while_it_reads_leaves_no_sheets() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = settings_db(true, 1024).await;
    let mut dedicated = preview_addon();
    let passes = passes_with(&dedicated);
    let source = preview_source("preview-a", &dedicated);
    let finder = FakeFinder {
        sources: vec![source.clone()],
        ..Default::default()
    };
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        wait_for_cancel: true,
        ..Default::default()
    });
    let set_id = Uuid::new_v4();
    let plan = PassPlan {
        db: Some(pool.clone()),
        ..preview_plan(tmp.path(), set_id, &[])
    };
    // The version played failed lately, so nothing follows preview-a.
    passes.mark_failed(plan.stream_id, Instant::now() + FAILURE_WAIT);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin_with("play-a", plan, runner.clone(), always(), &finder)
        .await;
    assert_eq!(begun, Begin::Queued);
    let dir = set_dir(tmp.path(), set_id);
    until_named("preview-a made its sheets", || sheets_in(&dir).len() == 29).await;
    // Its addon's seek previews are turned off while its run waits at the
    // end.
    dedicated.resources = vec![];
    passes.set_addon_rule(&dedicated);
    until_named("the run stopped", || {
        runner
            .killed
            .load(Ordering::SeqCst)
    })
    .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.inputs(), strings(&["preview-a"]));
    assert!(sheets_in(&dir).is_empty(), "{:?}", sheets_in(&dir));
    assert!(
        !passes.failed_recently(source.stream_id, Instant::now()),
        "not a failure"
    );
}

#[tokio::test]
async fn a_stored_preview_source_outside_the_users_addons_is_not_read() {
    let (_server, guard) = crate::integration_test::new_test_server()
        .await
        .unwrap();
    let ctx = guard
        .0
        .clone();
    let tmp = tempfile::tempdir().unwrap();
    let mut film = db::Media {
        title: "Invented Film".to_string(),
        kind: db::MediaKind::Movie,
        runtime: Some(7_200),
        external_ids: db::ExternalIds {
            tmdb: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };
    film.save(&ctx.db)
        .await
        .unwrap();
    let played_from = addon("stremio", false);
    let default_one = preview_addon();
    let not_default = Addon {
        is_default: false,
        ..preview_addon()
    };
    ctx.sessions
        .trickplay
        .set_addon_rules(&[
            played_from.clone(),
            default_one.clone(),
            not_default.clone(),
        ]);
    for dedicated in [&default_one, &not_default] {
        let row = version(film.id, 7_200.0, dedicated.id, 0);
        db::PreviewSource::store(
            &ctx.db,
            film.id,
            dedicated.id,
            &[db::NewPreviewSource {
                content_key: "invented".to_string(),
                stream_info: row
                    .stream_info
                    .clone()
                    .unwrap(),
                probe_data: row
                    .probe_data
                    .clone(),
            }],
            chrono::Utc::now().naive_utc(),
        )
        .await
        .unwrap();
    }
    let ask = PreviewAsk {
        title_id: film.id,
        playing: version(film.id, 7_200.0, played_from.id, 0),
        // No addon list of their own: only default addons are theirs.
        user_id: Uuid::new_v4(),
        root: tmp
            .path()
            .to_path_buf(),
    };
    let found = find_preview_sources(&ctx, &ask).await;
    let read_from: Vec<Option<Uuid>> = found
        .sources
        .iter()
        .map(|source| {
            source
                .info
                .addon_id
        })
        .collect();
    assert_eq!(read_from, vec![Some(default_one.id)]);
}

#[tokio::test]
async fn at_most_two_stored_preview_sources_are_measured_and_a_failed_one_waits() {
    let (_server, guard) = crate::integration_test::new_test_server()
        .await
        .unwrap();
    let ctx = guard
        .0
        .clone();
    let tmp = tempfile::tempdir().unwrap();
    let mut film = db::Media {
        title: "Invented Film".to_string(),
        kind: db::MediaKind::Movie,
        runtime: Some(7_200),
        external_ids: db::ExternalIds {
            tmdb: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };
    film.save(&ctx.db)
        .await
        .unwrap();
    let played_from = addon("stremio", false);
    let dedicated = preview_addon();
    ctx.sessions
        .trickplay
        .set_addon_rules(&[played_from.clone(), dedicated.clone()]);
    // Three files with no length; nothing serves them, so no measure
    // succeeds.
    let answer: Vec<db::NewPreviewSource> = ["a", "b", "c"]
        .iter()
        .map(|key| db::NewPreviewSource {
            content_key: key.to_string(),
            stream_info: version(film.id, 7_200.0, dedicated.id, 0)
                .stream_info
                .unwrap(),
            probe_data: None,
        })
        .collect();
    db::PreviewSource::store(
        &ctx.db,
        film.id,
        dedicated.id,
        &answer,
        chrono::Utc::now().naive_utc(),
    )
    .await
    .unwrap();
    let ask = PreviewAsk {
        title_id: film.id,
        playing: version(film.id, 7_200.0, played_from.id, 0),
        user_id: Uuid::new_v4(),
        root: tmp
            .path()
            .to_path_buf(),
    };
    let found = find_preview_sources(&ctx, &ask).await;
    assert!(
        found
            .sources
            .is_empty()
    );
    assert_eq!(
        found.other_lengths, 0,
        "an unknown length is not another length"
    );
    let passes = &ctx
        .sessions
        .trickplay;
    let failed = |now: Instant| {
        answer
            .iter()
            .filter(|source| {
                let id = db::PreviewSource::id_for(
                    film.id,
                    dedicated.id,
                    &source.content_key,
                );
                passes.failed_recently(id, now)
            })
            .count()
    };
    assert_eq!(failed(Instant::now()), MAX_PREVIEW_MEASURES);

    // A second play measures only the one not tried yet.
    find_preview_sources(&ctx, &ask).await;
    assert_eq!(failed(Instant::now()), 3);
    // Last, since a look this far ahead forgets the waits.
    assert_eq!(
        failed(Instant::now() + OTHER_FAILURE_WAIT + Duration::from_secs(1)),
        0,
        "measured again after its wait"
    );
}

#[test]
fn a_length_from_the_header_counts_as_measured_and_keeps_the_rest() {
    let title = Uuid::new_v4();
    let row = unmeasured(title, Uuid::new_v4());
    assert!(wants_length(&row));
    assert_eq!(measured_ticks(&row), None);
    let tracks = vec![api::MediaStream {
        type_: api::MediaStreamType::Video,
        codec: Some("h264".to_string()),
        ..Default::default()
    }];
    let guessed = api::MediaSourceInfo {
        media_streams: tracks.clone(),
        container: Some(api::VideoContainer::Mkv),
        ..row
            .probe_data
            .clone()
            .unwrap()
    };
    let ticks = 62_065_280_000;
    let stored = with_length(Some(guessed), ticks).expect("stored");
    assert_eq!(stored.run_time_ticks, Some(ticks));
    assert_eq!(stored.media_streams, tracks);
    assert_eq!(stored.container, Some(api::VideoContainer::Mkv));
    assert!(stored.is_filename_guess(), "its tracks are still a guess");
    let row = db::Media {
        probe_data: Some(stored.clone()),
        ..row
    };
    assert_eq!(measured_ticks(&row), Some(ticks));
    assert!(!wants_length(&row));
    assert!(set_id(&row) != row.id, "it shares a set by its count");

    // A row with no probe data gets some; a measured one keeps its own.
    let created = with_length(None, ticks).expect("created");
    assert_eq!(created.run_time_ticks, Some(ticks));
    assert!(
        created
            .media_streams
            .is_empty()
    );
    assert!(with_length(Some(stored), ticks + 1).is_none());
    let probed = version(title, 6_206.528, Uuid::new_v4(), 0);
    assert!(with_length(probed.probe_data, ticks).is_none());
    // Not a stream under a title.
    let loose = db::Media {
        parent_id: None,
        ..unmeasured(title, Uuid::new_v4())
    };
    assert!(!wants_length(&loose));
}

#[tokio::test]
async fn a_length_read_during_the_start_delay_gets_the_play_its_ask_and_its_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let dedicated = preview_addon();
    let passes = waiting_passes(&dedicated);
    let playing = unmeasured(Uuid::new_v4(), addon("stremio", false).id);
    let stored = Arc::new(Mutex::new(playing.clone()));
    let arrives = stored.clone();
    let measured_row = learned(&playing);
    // At about 4 of 15 seconds.
    let at = WAIT_DELAY * 4 / 15;
    tokio::spawn(async move {
        tokio::time::sleep(at).await;
        *arrives
            .lock()
            .unwrap() = measured_row;
    });
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let started = Instant::now();
    let (got, spent) = passes
        .wait_for_length("play-a", playing, |_id| {
            let row = stored
                .lock()
                .unwrap()
                .clone();
            std::future::ready(Some(row))
        })
        .await;
    let waited = started.elapsed();
    assert!(spent <= waited, "{spent:?}");
    assert_eq!(measured_ticks(&got), Some(RUNTIME as i64 * TICKS_PER_SEC));
    assert!(waited >= at, "{waited:?}");
    assert!(
        waited < at + SLACK,
        "it stops once the length is there: {waited:?}"
    );

    let set = set_id(&got);
    let ask = preview_ask(&got, set, tmp.path(), Uuid::new_v4(), &passes.addon_rules())
        .expect("asked as a measured version is");
    let finder = FakeFinder {
        sources: vec![preview_source("preview-a", &dedicated)],
        ..Default::default()
    };
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        ..Default::default()
    });
    let plan = PassPlan {
        preview: Some(ask),
        delay_spent: spent,
        ..plan_with(tmp.path(), set, &[])
    };
    let begun = passes
        .begin_with("play-a", plan, runner.clone(), always(), &finder)
        .await;
    let delay = started.elapsed();
    assert_eq!(begun, Begin::Queued);
    assert!(delay + SLACK >= WAIT_DELAY, "{delay:?}");
    assert!(
        delay < WAIT_DELAY + SLACK,
        "the wait is part of the start delay, not added to it: {delay:?}"
    );
    until(|| !passes.is_queued(set)).await;
    assert_eq!(finder.calls(), 1);
    assert_eq!(runner.inputs(), strings(&["preview-a"]));
    assert!(is_done(&set_dir(tmp.path(), set)));
}

#[tokio::test]
async fn a_length_that_never_arrives_ends_the_wait_at_the_delay_with_no_ask() {
    let tmp = tempfile::tempdir().unwrap();
    let dedicated = preview_addon();
    let passes = waiting_passes(&dedicated);
    let playing = unmeasured(Uuid::new_v4(), addon("stremio", false).id);
    let looks = AtomicUsize::new(0);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let started = Instant::now();
    let (got, spent) = passes
        .wait_for_length("play-a", playing.clone(), |_id| {
            looks.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Some(playing.clone()))
        })
        .await;
    let waited = started.elapsed();
    assert!(spent <= waited, "{spent:?}");
    assert_eq!(measured_ticks(&got), None);
    assert!(waited + SLACK >= WAIT_DELAY, "{waited:?}");
    assert!(waited < WAIT_DELAY + SLACK, "{waited:?}");
    assert!(looks.load(Ordering::SeqCst) > 1);
    let set = set_id(&got);
    assert_eq!(set, got.id);
    assert!(
        preview_ask(&got, set, tmp.path(), Uuid::new_v4(), &passes.addon_rules())
            .is_none()
    );
    let finder = FakeFinder {
        sources: vec![preview_source("preview-a", &dedicated)],
        ..Default::default()
    };
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        ..Default::default()
    });
    let begun = passes
        .begin_with(
            "play-a",
            PassPlan {
                delay_spent: spent,
                ..plan_with(tmp.path(), set, &[])
            },
            runner.clone(),
            always(),
            &finder,
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    assert!(
        started.elapsed() < WAIT_DELAY + SLACK,
        "no second start delay"
    );
    until(|| !passes.is_queued(set)).await;
    assert_eq!(finder.calls(), 0);
    assert_eq!(runner.inputs(), strings(&[PLAYED]), "as today");
}

#[tokio::test]
async fn a_measured_version_is_not_kept_waiting() {
    let dedicated = preview_addon();
    let passes = waiting_passes(&dedicated);
    let playing = version(Uuid::new_v4(), RUNTIME as f64, Uuid::new_v4(), 0);
    assert!(!wants_length(&playing));
    let looks = AtomicUsize::new(0);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let started = Instant::now();
    let (got, spent) = passes
        .wait_for_length("play-a", playing.clone(), |_id| {
            looks.fetch_add(1, Ordering::SeqCst);
            std::future::ready(None)
        })
        .await;
    assert!(started.elapsed() < Duration::from_millis(40));
    assert!(spent < Duration::from_millis(40));
    assert_eq!(looks.load(Ordering::SeqCst), 0);
    assert_eq!(got.id, playing.id);
}

#[tokio::test]
async fn a_play_that_goes_away_during_the_wait_leaves_nothing_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let dedicated = preview_addon();
    let passes = waiting_passes(&dedicated);
    let playing = unmeasured(Uuid::new_v4(), addon("stremio", false).id);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let leaving = passes.clone();
    let at = WAIT_DELAY / 5;
    tokio::spawn(async move {
        tokio::time::sleep(at).await;
        leaving.release("play-a");
    });
    let started = Instant::now();
    let (got, _spent) = passes
        .wait_for_length("play-a", playing.clone(), |_id| {
            std::future::ready(Some(playing.clone()))
        })
        .await;
    assert!(started.elapsed() < at + SLACK, "it stops with the play");
    assert_eq!(passes.registration_count(), 0);
    let finder = FakeFinder::default();
    let runner = Arc::new(FakeRunner::default());
    let set = set_id(&got);
    let begun = passes
        .begin_with(
            "play-a",
            plan_with(tmp.path(), set, &[]),
            runner.clone(),
            always(),
            &finder,
        )
        .await;
    assert_eq!(begun, Begin::Released);
    assert!(!passes.has_pass(set));
    assert_eq!(passes.registration_count(), 0);
    assert_eq!(finder.calls(), 0);
    assert_eq!(
        runner
            .runs
            .load(Ordering::SeqCst),
        0
    );
}

/// The plan of a play whose length is read from its header in its first
/// seconds waits for it, counts its pictures from it and asks the addon
/// kept for seek previews only, as for a measured version.
#[tokio::test]
async fn a_plan_waits_for_a_length_read_as_the_play_starts() {
    let tmp = tempfile::tempdir().unwrap();
    let guard = ctx_in(tmp.path()).await;
    let ctx = &guard.0;
    let passes = &ctx
        .sessions
        .trickplay;
    passes.set_addon_rules(std::slice::from_ref(&preview_addon()));
    let (film, stream) = film_and_version(ctx, 1, false).await;
    passes.register("play-a", film.id, "device-a");
    // 6,206.528 s: 621 pictures, where the listed runtime gives 720.
    let ticks = 62_065_280_000;
    let at = Duration::from_millis(300);
    let storing = ctx.clone();
    let stream_id = stream.id;
    tokio::spawn(async move {
        tokio::time::sleep(at).await;
        store_length(
            storing,
            stream_id,
            "Invented.Film.1080p.mkv".to_string(),
            ticks,
            "matroska",
        )
        .await;
    });
    let started = Instant::now();
    let plan = resolve_plan(ctx, &play_of_version(&film, &stream))
        .await
        .expect("planned");
    let took = started.elapsed();
    assert!(
        plan.preview
            .is_some(),
        "asked as a measured version is"
    );
    assert_eq!(plan.pictures, picture_count(ticks));
    assert_ne!(plan.pictures, picture_count(RUNTIME as i64 * TICKS_PER_SEC));
    assert!(plan.delay_spent >= at, "{:?}", plan.delay_spent);
    assert!(plan.delay_spent <= took, "{:?}", plan.delay_spent);
    assert!(took < START_DELAY, "{took:?}");
}

/// Only a version with no measured length, while an addon kept for seek
/// previews only is enabled, keeps its plan waiting.
#[tokio::test]
async fn a_plan_does_not_wait_when_the_length_is_known_or_not_wanted() {
    let tmp = tempfile::tempdir().unwrap();
    let guard = ctx_in(tmp.path()).await;
    let ctx = &guard.0;
    let passes = &ctx
        .sessions
        .trickplay;
    let (film, unmeasured_row) = film_and_version(ctx, 1, false).await;
    let (other_film, measured_row) = film_and_version(ctx, 2, true).await;

    // No addon kept for seek previews only.
    passes.set_addon_rules(&[]);
    passes.register("play-a", film.id, "device-a");
    let started = Instant::now();
    let plan = resolve_plan(ctx, &play_of_version(&film, &unmeasured_row))
        .await
        .expect("planned");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(plan.delay_spent, Duration::ZERO);
    assert!(
        plan.preview
            .is_none()
    );
    passes.release("play-a");

    // A measured version, with one enabled.
    passes.set_addon_rules(std::slice::from_ref(&preview_addon()));
    passes.register("play-a", other_film.id, "device-a");
    let started = Instant::now();
    let plan = resolve_plan(ctx, &play_of_version(&other_film, &measured_row))
        .await
        .expect("planned");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(plan.delay_spent, Duration::ZERO);
    assert!(
        plan.preview
            .is_some()
    );
}
