//! Passes in memory: plays, the slot, giving way, starting where the viewer is.

use super::*;

#[tokio::test]
async fn complete_pass_writes_done() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let stream_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        ..Default::default()
    });
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            plan(tmp.path(), stream_id),
            runner.clone(),
            always(),
        )
        .await;
    until(|| !passes.is_queued(stream_id)).await;
    let dir = set_dir(tmp.path(), stream_id);
    assert!(
        dir.join(DONE_MARKER)
            .exists()
    );
    assert!(
        dir.join("28.jpg")
            .exists()
    );

    // A later play of the same file starts nothing.
    passes.register("play-b", Uuid::new_v4(), "device-b");
    let again = passes
        .begin(
            "play-b",
            plan(tmp.path(), stream_id),
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(again, Begin::Done);
    assert_eq!(
        runner
            .runs
            .load(Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn release_with_grace() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let stream_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner {
        wait_for_cancel: true,
        ..Default::default()
    });
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            plan(tmp.path(), stream_id),
            runner.clone(),
            always(),
        )
        .await;
    until(|| {
        runner
            .runs
            .load(Ordering::SeqCst)
            == 1
    })
    .await;

    // A retry within the grace period keeps the pass.
    passes.release("play-a");
    tokio::time::sleep(Duration::from_millis(10)).await;
    passes.register("play-b", Uuid::new_v4(), "device-a");
    assert!(passes.attach("play-b", stream_id));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(passes.has_pass(stream_id));
    assert!(
        !runner
            .killed
            .load(Ordering::SeqCst)
    );

    // Once the grace period passes with nobody watching, it is cancelled.
    passes.release("play-b");
    until(|| {
        runner
            .killed
            .load(Ordering::SeqCst)
    })
    .await;
    assert!(!passes.has_pass(stream_id));
}

#[tokio::test]
async fn release_before_resolve() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let stream_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner::default());
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes.release("play-a");
    assert!(!passes.attach("play-a", stream_id));
    let begun = passes
        .begin(
            "play-a",
            plan(tmp.path(), stream_id),
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(begun, Begin::Released);
    assert!(!passes.has_pass(stream_id));
    assert_eq!(
        runner
            .runs
            .load(Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn queued_pass_cancelled_before_start() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let stream_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner::default());
    let held = passes
        .inner
        .slots
        .acquire()
        .await
        .unwrap();
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
    passes.release("play-a");
    until(|| !passes.has_pass(stream_id)).await;
    drop(held);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        runner
            .runs
            .load(Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn start_delay_and_failure_wait() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let stream_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner::default());

    // The play ended during the start delay: nothing starts.
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin(
            "play-a",
            plan(tmp.path(), stream_id),
            runner.clone(),
            never(),
        )
        .await;
    assert_eq!(begun, Begin::Released);
    assert!(
        passes
            .play_of("play-a")
            .is_none()
    );

    // A failure blocks the file for 10 minutes.
    passes.mark_failed(stream_id, Instant::now() + FAILURE_WAIT);
    passes.register("play-b", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin(
            "play-b",
            plan(tmp.path(), stream_id),
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(begun, Begin::FailedRecently);
    assert_eq!(
        runner
            .runs
            .load(Ordering::SeqCst),
        0
    );
    assert!(!passes.failed_recently(
        stream_id,
        Instant::now() + FAILURE_WAIT + Duration::from_secs(1)
    ));
}

#[tokio::test]
async fn selection_follows_registration() {
    let passes = TrickplayPasses::with_limits(fast());
    let item_id = Uuid::new_v4();
    let stream_id = Uuid::new_v4();
    passes.register("play-a", item_id, "device-a");
    assert!(passes.attach("play-a", stream_id));
    assert_eq!(passes.selected_set(item_id, "device-a"), Some(stream_id));
    assert_eq!(passes.selected_set(item_id, "device-b"), None);
    passes.release("play-a");
    until(|| !passes.has_pass(stream_id)).await;
    assert_eq!(passes.selected_set(item_id, "device-a"), None);
}

#[tokio::test]
async fn stop_all_kills_every_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let mut sessions = crate::playback_session::PlaybackSessionManager::new(
        tmp.path()
            .join("t"),
    );
    sessions.trickplay = TrickplayPasses::with_limits(fast());
    let stream_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner {
        wait_for_cancel: true,
        ..Default::default()
    });
    sessions
        .trickplay
        .register("play-a", Uuid::new_v4(), "device-a");
    sessions
        .trickplay
        .begin(
            "play-a",
            plan(tmp.path(), stream_id),
            runner.clone(),
            always(),
        )
        .await;
    until(|| {
        runner
            .runs
            .load(Ordering::SeqCst)
            == 1
    })
    .await;

    sessions
        .stop_all()
        .await;
    assert_eq!(
        sessions
            .trickplay
            .registration_count(),
        0
    );
    assert!(
        !sessions
            .trickplay
            .has_pass(stream_id)
    );
    assert!(
        runner
            .killed
            .load(Ordering::SeqCst)
    );
}

#[tokio::test]
async fn start_without_play_session_id_is_released() {
    use crate::integration_test::{auth_header_with_token, authenticated_server};
    use http::{StatusCode, header::HeaderValue};
    use serde_json::json;

    let (server, guard, token) = authenticated_server().await;
    let ctx = &guard.0;
    let cfg = api::ServerConfiguration {
        trickplay_while_streaming: Some(true),
        ..db::Settings::get_config_or_default(&ctx.db).await
    };
    db::Settings::set_config(&ctx.db, &cfg)
        .await
        .unwrap();
    let mut stream = db::Media {
        title: "Invented Film".to_string(),
        kind: db::MediaKind::Stream,
        stream_info: Some(StreamInfo {
            descriptor: StreamDescriptor::http(URL),
            ..Default::default()
        }),
        probe_data: Some(api::MediaSourceInfo {
            run_time_ticks: Some(RUNTIME as i64 * TICKS_PER_SEC),
            ..Default::default()
        }),
        ..Default::default()
    };
    stream
        .save(&ctx.db)
        .await
        .unwrap();
    let auth = auth_header_with_token(&token);
    let item_id = stream
        .id
        .simple()
        .to_string();

    server
        .post("/sessions/playing")
        .add_header(
            http::header::AUTHORIZATION,
            HeaderValue::from_str(&auth).unwrap(),
        )
        .json(&json!({ "ItemId": item_id, "PositionTicks": 0 }))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(
        ctx.sessions
            .trickplay
            .registration_count(),
        1
    );

    server
        .post("/sessions/playing/stopped")
        .add_header(
            http::header::AUTHORIZATION,
            HeaderValue::from_str(&auth).unwrap(),
        )
        .json(&json!({ "ItemId": item_id, "PositionTicks": 0 }))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(
        ctx.sessions
            .trickplay
            .registration_count(),
        0
    );
}

#[test]
fn only_movies_episodes_and_streams_have_pictures() {
    for kind in [
        db::MediaKind::Movie,
        db::MediaKind::Episode,
        db::MediaKind::Stream,
        db::MediaKind::StreamGroup,
    ] {
        assert!(kind_has_pictures(&kind), "{kind:?}");
    }
    for kind in [
        db::MediaKind::Track,
        db::MediaKind::Album,
        db::MediaKind::Artist,
        db::MediaKind::Series,
        db::MediaKind::TvChannel,
    ] {
        assert!(!kind_has_pictures(&kind), "{kind:?}");
    }
}

#[tokio::test]
async fn a_music_track_gets_no_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let guard = ctx_in(tmp.path()).await;
    let ctx = &guard.0;
    let mut track = db::Media {
        title: "Invented Song".to_string(),
        kind: db::MediaKind::Track,
        // A track is refused without a music provider id.
        external_ids: db::ExternalIds {
            deezer_track: Some(1),
            ..Default::default()
        },
        runtime: Some(600),
        ..Default::default()
    };
    track
        .save(&ctx.db)
        .await
        .unwrap();
    let play = PlayStart {
        play_session_id: "play-a".to_string(),
        item_id: track.id,
        media_source_id: None,
        device_id: "device-a".to_string(),
        user_id: Uuid::new_v4(),
    };
    assert_eq!(
        resolve_plan(ctx, &play)
            .await
            .err(),
        Some(Ineligible::NotVideo)
    );
}

#[test]
fn a_failed_addon_read_refuses_the_pass() {
    assert_eq!(
        addon_from_read(Err(anyhow::anyhow!("pool timed out"))).err(),
        Some(Ineligible::AddonReadFailed)
    );
    assert!(
        matches!(addon_from_read(Ok(None)), Ok(None)),
        "a deleted addon is no row; its stream is refused by its id"
    );
    let found = addon("stremio", false);
    let id = found.id;
    assert_eq!(
        addon_from_read(Ok(Some(found)))
            .ok()
            .flatten()
            .map(|a| a.id),
        Some(id)
    );
}

#[test]
fn one_addons_rule_is_set_and_dropped_without_a_read() {
    let passes = TrickplayPasses::with_limits(fast());
    let kept = addon("stremio", false);
    let mut changed = addon("stremio", false);
    passes.set_addon_rules(std::slice::from_ref(&kept));
    let handed_out = passes.addon_rules();

    changed.resources = vec![ResourceType::Stream];
    passes.set_addon_rule(&changed);
    let rules = passes.addon_rules();
    assert_eq!(rules.get(&changed.id), Some(&AddonRule::new(&changed)));
    assert!(rules.contains_key(&kept.id), "the other rules stay");
    assert!(
        !handed_out.contains_key(&changed.id),
        "a snapshot already handed out does not change"
    );

    changed.resources = vec![ResourceType::Stream, ResourceType::SeekPreviews];
    passes.set_addon_rule(&changed);
    assert!(
        passes
            .addon_rules()
            .get(&changed.id)
            .is_some_and(|rule| rule.reads)
    );

    passes.drop_addon_rule(changed.id);
    let rules = passes.addon_rules();
    assert!(!rules.contains_key(&changed.id));
    assert!(rules.contains_key(&kept.id));
}

#[test]
fn cap_bytes_saturates() {
    let huge = api::ServerConfiguration {
        trickplay_cache_size_mb: Some(i64::MAX),
        ..Default::default()
    };
    assert_eq!(cap_bytes(&huge), u64::MAX);
}

#[test]
fn a_pass_and_the_field_judge_a_length_alike() {
    let (mut media, info) = row(StreamDescriptor::http(URL));
    let probed = |ticks: i64| {
        Some(api::MediaSourceInfo {
            run_time_ticks: Some(ticks),
            ..Default::default()
        })
    };
    // 14,400.5 s: four hours and half a second.
    let ticks = 144_005_000_000;
    media.probe_data = probed(ticks);
    let runtime = runtime_s(&media, None);
    assert_eq!(runtime, Some(14_400));
    assert_eq!(eligible_row(&media, &info, runtime), Ok(()));
    assert!(manifest_entry(Some(ticks)).is_some());

    // 14,401 s.
    let ticks = 144_010_000_000;
    media.probe_data = probed(ticks);
    assert_eq!(
        eligible_row(&media, &info, runtime_s(&media, None)),
        Err(Ineligible::TooLong)
    );
    assert!(manifest_entry(Some(ticks)).is_none());
}

#[tokio::test]
async fn an_addon_switched_off_while_a_pass_waits_is_not_read() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let mut source = addon("stremio", false);
    passes.set_addon_rules(std::slice::from_ref(&source));
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
    let stream_id = Uuid::new_v4();
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin(
            "play-a",
            addon_plan(tmp.path(), stream_id, &source),
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    source.resources = vec![ResourceType::Stream];
    passes.set_addon_rule(&source);
    drop(held);
    until(|| !passes.is_queued(stream_id)).await;
    assert_eq!(
        runner
            .runs
            .load(Ordering::SeqCst),
        0,
        "the source is never opened"
    );
    assert!(
        !passes.failed_recently(stream_id, Instant::now()),
        "not a failure: the next play is judged afresh"
    );

    // A stream with no addon is judged by its row alone, and is read.
    let loose = Uuid::new_v4();
    passes.register("play-b", Uuid::new_v4(), "device-b");
    passes
        .begin("play-b", plan(tmp.path(), loose), runner.clone(), always())
        .await;
    until(|| !passes.is_queued(loose)).await;
    assert_eq!(
        runner
            .runs
            .load(Ordering::SeqCst),
        1
    );
    assert!(
        set_dir(tmp.path(), loose)
            .join(DONE_MARKER)
            .exists()
    );
}

#[tokio::test]
async fn an_addon_switched_off_during_a_run_stops_it() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let mut source = addon("stremio", false);
    passes.set_addon_rules(std::slice::from_ref(&source));
    let runner = Arc::new(FakeRunner {
        wait_for_cancel: true,
        ..Default::default()
    });
    let stream_id = Uuid::new_v4();
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            addon_plan(tmp.path(), stream_id, &source),
            runner.clone(),
            always(),
        )
        .await;
    until(|| {
        runner
            .runs
            .load(Ordering::SeqCst)
            == 1
    })
    .await;
    source.resources = vec![ResourceType::Stream];
    passes.set_addon_rule(&source);
    until_named("the run stopped", || {
        runner
            .killed
            .load(Ordering::SeqCst)
    })
    .await;
    until(|| !passes.is_queued(stream_id)).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        runner
            .runs
            .load(Ordering::SeqCst),
        1,
        "no run after it"
    );
    assert!(!passes.failed_recently(stream_id, Instant::now()));
}

#[tokio::test]
async fn switching_off_stops_a_running_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = settings_db(true, 1024).await;
    let passes = TrickplayPasses::with_limits(fast());
    let runner = Arc::new(FakeRunner {
        wait_for_cancel: true,
        ..Default::default()
    });
    let stream_id = Uuid::new_v4();
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin(
            "play-a",
            PassPlan {
                db: Some(pool.clone()),
                ..plan(tmp.path(), stream_id)
            },
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| {
        runner
            .runs
            .load(Ordering::SeqCst)
            == 1
    })
    .await;
    save_settings(&pool, false, 1024).await;
    until_named("the run stopped", || {
        runner
            .killed
            .load(Ordering::SeqCst)
    })
    .await;
    until(|| !passes.is_queued(stream_id)).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        runner
            .runs
            .load(Ordering::SeqCst),
        1,
        "no run after it"
    );
    assert!(!passes.failed_recently(stream_id, Instant::now()));
}

#[tokio::test]
async fn the_disk_limits_are_read_when_the_pass_takes_the_slot() {
    for (cache_mb, old_kept) in [(100, false), (2048, true)] {
        let tmp = tempfile::tempdir().unwrap();
        // The cache size when the play starts: all of it this set's.
        let pool = settings_db(true, 100).await;
        let passes = TrickplayPasses::with_limits(fast());
        let old = set_dir(tmp.path(), Uuid::new_v4());
        write_sheets(&old, 0..3);
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
        let stream_id = Uuid::new_v4();
        passes.register("play-a", Uuid::new_v4(), "device-a");
        let begun = passes
            .begin(
                "play-a",
                PassPlan {
                    db: Some(pool.clone()),
                    estimate: 100 * 1024 * 1024,
                    ..plan(tmp.path(), stream_id)
                },
                runner.clone(),
                always(),
            )
            .await;
        assert_eq!(begun, Begin::Queued);
        // Saved after the play started, before its pass takes the slot.
        save_settings(&pool, true, cache_mb).await;
        drop(held);
        until(|| !passes.is_queued(stream_id)).await;
        assert_eq!(old.exists(), old_kept, "cache size {cache_mb} MB");
        assert!(
            set_dir(tmp.path(), stream_id)
                .join(DONE_MARKER)
                .exists()
        );
    }
}

#[tokio::test]
async fn the_start_position_is_read_when_the_pass_takes_the_slot() {
    let settings = |start: bool| api::ServerConfiguration {
        trickplay_while_streaming: Some(true),
        trickplay_cache_size_mb: Some(1024),
        trickplay_start_at_position: Some(start),
        ..Default::default()
    };
    for (planned, saved, starts) in [(true, false, vec![0]), (false, true, vec![19, 0])]
    {
        let tmp = tempfile::tempdir().unwrap();
        // The start position when the play starts.
        let pool = settings_db(true, 1024).await;
        db::Settings::set_config(&pool, &settings(planned))
            .await
            .unwrap();
        let passes = TrickplayPasses::with_limits(fast());
        let reports = Reports::default();
        let runner = Arc::new(FakeRunner {
            sheets: 38,
            ..Default::default()
        });
        let held = passes
            .inner
            .slots
            .acquire()
            .await
            .unwrap();
        let set_id = Uuid::new_v4();
        // 1 h 20 in: sheet 19 of 38.
        reports.at("device-a", 80 * 60);
        passes.register("play-a", Uuid::new_v4(), "device-a");
        let begun = passes
            .begin(
                "play-a",
                PassPlan {
                    db: Some(pool.clone()),
                    pictures: 940,
                    from_position: planned,
                    ..plan(tmp.path(), set_id)
                },
                runner.clone(),
                reports.watch(),
            )
            .await;
        assert_eq!(begun, Begin::Queued);
        // Saved after the play started, before its pass takes the slot.
        db::Settings::set_config(&pool, &settings(saved))
            .await
            .unwrap();
        drop(held);
        until(|| !passes.is_queued(set_id)).await;
        assert_eq!(runner.starts(), starts, "saved {saved}");
        assert!(
            set_dir(tmp.path(), set_id)
                .join(DONE_MARKER)
                .exists()
        );
    }
}

#[tokio::test]
async fn versions_of_one_set_share_one_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let set_id = Uuid::new_v4();
    let (stream_a, stream_b, stream_c) =
        (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        ..Default::default()
    });
    // The slot is busy, so the first pass is still queued when the second
    // version is played.
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
            set_plan(tmp.path(), stream_a, set_id),
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(first, Begin::Queued);
    // The sheet route can tell a sheet past the count from one not made.
    assert_eq!(passes.pictures_of(set_id), Some(RUNTIME / INTERVAL_SECS));
    assert_eq!(passes.pictures_of(Uuid::new_v4()), None);
    passes.register("play-b", Uuid::new_v4(), "device-b");
    let second = passes
        .begin(
            "play-b",
            set_plan(tmp.path(), stream_b, set_id),
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(second, Begin::Joined);
    drop(held);
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(
        runner
            .runs
            .load(Ordering::SeqCst),
        1
    );
    assert!(
        set_dir(tmp.path(), set_id)
            .join(DONE_MARKER)
            .exists()
    );
    assert!(!set_dir(tmp.path(), stream_a).exists());

    // A third version of the same count starts nothing.
    passes.register("play-c", Uuid::new_v4(), "device-c");
    let third = passes
        .begin(
            "play-c",
            set_plan(tmp.path(), stream_c, set_id),
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(third, Begin::Done);
    assert_eq!(
        runner
            .runs
            .load(Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn a_failure_waits_by_stream_not_by_set() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let set_id = Uuid::new_v4();
    let (stream_a, stream_b) = (Uuid::new_v4(), Uuid::new_v4());
    // Makes no sheet: a failure.
    let runner = Arc::new(FakeRunner::default());
    passes.mark_failed(stream_a, Instant::now() + FAILURE_WAIT);

    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin(
            "play-a",
            set_plan(tmp.path(), stream_a, set_id),
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(begun, Begin::FailedRecently);

    passes.register("play-b", Uuid::new_v4(), "device-b");
    let begun = passes
        .begin(
            "play-b",
            set_plan(tmp.path(), stream_b, set_id),
            runner.clone(),
            always(),
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(
        runner
            .runs
            .load(Ordering::SeqCst),
        1
    );
    assert!(passes.failed_recently(stream_b, Instant::now()));
}

#[tokio::test]
async fn a_cancelled_pass_keeps_every_sheet() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let stream_id = Uuid::new_v4();
    let dir = set_dir(tmp.path(), stream_id);
    write_sheets(&dir, 0..10);
    // It makes sheets 10 to 12, then is stopped.
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        per_run: Some(3),
        wait_for_cancel: true,
        ..Default::default()
    });
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            plan(tmp.path(), stream_id),
            runner.clone(),
            always(),
        )
        .await;
    until(|| {
        runner
            .runs
            .load(Ordering::SeqCst)
            == 1
    })
    .await;
    passes.release("play-a");
    until(|| {
        runner
            .killed
            .load(Ordering::SeqCst)
    })
    .await;
    until(|| !passes.has_pass(stream_id)).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(runner.starts(), vec![10], "built on what exists");
    assert_eq!(sheets_in(&dir), (0..13).collect::<Vec<u32>>());
    for marker in [DONE_MARKER, END_MARKER, BUSY_MARKER] {
        assert!(
            !dir.join(marker)
                .exists(),
            "{marker}"
        );
    }
    assert!(!passes.failed_recently(stream_id, Instant::now()));
}

#[tokio::test]
async fn a_failed_pass_drops_only_the_last_sheet_it_wrote() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let stream_id = Uuid::new_v4();
    let dir = set_dir(tmp.path(), stream_id);
    write_sheets(&dir, 0..10);
    // Sheets 10 to 12 of 29, then a false end of input.
    let runner = Arc::new(FakeRunner {
        sheets: 29,
        per_run: Some(3),
        ..Default::default()
    });
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            plan(tmp.path(), stream_id),
            runner.clone(),
            always(),
        )
        .await;
    until(|| !passes.is_queued(stream_id)).await;
    assert_eq!(sheets_in(&dir), (0..12).collect::<Vec<u32>>());
    for marker in [DONE_MARKER, END_MARKER, BUSY_MARKER] {
        assert!(
            !dir.join(marker)
                .exists(),
            "{marker}"
        );
    }
    assert!(passes.failed_recently(stream_id, Instant::now()));
}

#[test]
fn a_pass_stopped_after_its_own_end_drops_its_last_sheet() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::new();
    let stream_id = Uuid::new_v4();
    let mut dir = set_dir(tmp.path(), stream_id);
    write_sheets(&dir, 0..20);
    let plan = plan(tmp.path(), stream_id);
    let run = Run {
        start: 0,
        stop_after: None,
    };

    // Killed while running: nothing is deleted.
    let killed = RunResult {
        made_sheets: 3,
        cancelled: true,
        ..Default::default()
    };
    let end = run_end(&killed, run, plan.pictures);
    assert_eq!(end, RunEnd::Cancelled);
    passes.after_run(&plan, &mut dir, run, &killed, end);
    assert_eq!(sheets_in(&dir), (0..20).collect::<Vec<u32>>());

    // Its output had ended before the stop came: the last sheet it wrote
    // may be partly filled.
    let ended = RunResult {
        made_sheets: 3,
        cancelled: true,
        ended_itself: true,
        ..Default::default()
    };
    let end = run_end(&ended, run, plan.pictures);
    passes.after_run(&plan, &mut dir, run, &ended, end);
    let expected: Vec<u32> = (0..20)
        .filter(|n| *n != 2)
        .collect();
    assert_eq!(sheets_in(&dir), expected);
    assert!(!passes.failed_recently(stream_id, Instant::now()));
}

#[test]
fn the_first_sheet_wait_grows_with_the_file() {
    const GB: u64 = 1024 * 1024 * 1024;
    assert_eq!(first_sheet_wait(None, 38, SEEK_WAIT), SEEK_WAIT);
    assert_eq!(first_sheet_wait(Some(13 * GB), 38, SEEK_WAIT), SEEK_WAIT);
    // 40 GB in 38 sheets is about 1.1 GB a sheet: about 216 s at 5 MB/s.
    let wait = first_sheet_wait(Some(40 * GB), 38, SEEK_WAIT);
    assert!(
        wait > Duration::from_secs(200) && wait < Duration::from_secs(240),
        "{wait:?}"
    );
}

#[test]
fn a_pass_follows_only_to_a_part_with_no_pictures_a_few_times() {
    let limits = Limits::default();
    let old = FOLLOW_EVERY;
    // The viewer moved to sheet 19 while the run is about to make sheet 2.
    assert!(follows_to(19, false, 2, old, 0, true, &limits));
    assert!(
        !follows_to(19, true, 2, old, 0, true, &limits),
        "it has its sheet"
    );
    assert!(!follows_to(2, false, 2, old, 0, true, &limits), "made next");
    assert!(
        !follows_to(3, false, 2, old, 0, true, &limits),
        "made next but one"
    );
    let young = old - Duration::from_secs(1);
    assert!(
        !follows_to(19, false, 2, young, 0, true, &limits),
        "too soon"
    );
    assert!(
        follows_to(19, false, 2, old, 3, true, &limits),
        "a fourth move"
    );
    assert!(
        !follows_to(19, false, 2, old, 4, true, &limits),
        "a fifth move"
    );
    assert!(
        !follows_to(19, false, 2, old, 0, false, &limits),
        "cannot jump"
    );
    // Back to a part not made yet.
    assert!(follows_to(0, false, 25, old, 0, true, &limits));
}

#[tokio::test]
async fn a_play_resumed_part_way_ends_with_the_beginning_made() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let reports = Reports::default();
    let set_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner {
        sheets: 38,
        ..Default::default()
    });
    // 1 h 20 into a film of 940 pictures: sheet 19 of 38.
    reports.at("device-a", 80 * 60);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin(
            "play-a",
            PassPlan {
                pictures: 940,
                ..plan(tmp.path(), set_id)
            },
            runner.clone(),
            reports.watch(),
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| !passes.is_queued(set_id)).await;
    let dir = set_dir(tmp.path(), set_id);
    assert_eq!(
        runner.starts(),
        vec![19, 0],
        "to the end, then the beginning"
    );
    assert_eq!(sheets_in(&dir), (0..38).collect::<Vec<u32>>());
    assert!(
        dir.join(END_MARKER)
            .exists()
    );
    assert!(
        dir.join(DONE_MARKER)
            .exists()
    );
    assert!(
        !dir.join(BUSY_MARKER)
            .exists()
    );
}

#[tokio::test]
async fn a_pass_stopped_after_the_end_leaves_the_beginning_to_the_next() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let reports = Reports::default();
    let set_id = Uuid::new_v4();
    let dir = set_dir(tmp.path(), set_id);
    let plan = PassPlan {
        pictures: 940,
        ..plan(tmp.path(), set_id)
    };
    // Its second run, from the beginning, stalls at sheet 10.
    let runner = Arc::new(FakeRunner {
        sheets: 38,
        stall_at: Some(10),
        ..Default::default()
    });
    reports.at("device-a", 80 * 60);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin("play-a", plan.clone(), runner.clone(), reports.watch())
        .await;
    until(|| {
        runner
            .runs
            .load(Ordering::SeqCst)
            == 2
            && dir
                .join("9.jpg")
                .exists()
    })
    .await;
    passes.release("play-a");
    until(|| {
        runner
            .killed
            .load(Ordering::SeqCst)
    })
    .await;
    until(|| !passes.has_pass(set_id)).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let kept: Vec<u32> = (0..10)
        .chain(19..38)
        .collect();
    assert_eq!(sheets_in(&dir), kept);
    assert!(
        dir.join(END_MARKER)
            .exists()
    );
    assert!(
        !dir.join(DONE_MARKER)
            .exists(),
        "not done before the beginning is made"
    );

    // The next pass makes only what is missing, and finishes the set.
    let rest = Arc::new(FakeRunner {
        sheets: 38,
        ..Default::default()
    });
    passes.register("play-b", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin("play-b", plan, rest.clone(), reports.watch())
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(rest.starts(), vec![10]);
    assert_eq!(sheets_in(&dir), (0..38).collect::<Vec<u32>>());
    assert!(
        dir.join(DONE_MARKER)
            .exists()
    );
}

#[tokio::test]
async fn a_source_that_cannot_jump_is_read_from_its_start() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let reports = Reports::default();
    let set_id = Uuid::new_v4();
    let (stream_a, stream_b) = (Uuid::new_v4(), Uuid::new_v4());
    let dir = set_dir(tmp.path(), set_id);
    let runner = Arc::new(FakeRunner {
        sheets: 38,
        no_jump: true,
        ..Default::default()
    });
    reports.at("device-a", 80 * 60);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            PassPlan {
                pictures: 940,
                ..set_plan(tmp.path(), stream_a, set_id)
            },
            runner.clone(),
            reports.watch(),
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.starts(), vec![19, 0]);
    assert_eq!(sheets_in(&dir), (0..38).collect::<Vec<u32>>());
    assert!(
        dir.join(DONE_MARKER)
            .exists()
    );
    assert!(!passes.can_jump(stream_a));
    assert!(
        !passes.failed_recently(stream_a, Instant::now()),
        "not a failure"
    );

    // Another version of the same set, from another source, still starts
    // part way.
    assert!(passes.can_jump(stream_b));
    for name in [DONE_MARKER, END_MARKER] {
        std::fs::remove_file(dir.join(name)).unwrap();
    }
    for n in 19..38 {
        remove_sheet(&dir, n);
    }
    let jumps = Arc::new(FakeRunner {
        sheets: 38,
        ..Default::default()
    });
    reports.at("device-b", 80 * 60);
    passes.register("play-b", Uuid::new_v4(), "device-b");
    passes
        .begin(
            "play-b",
            PassPlan {
                pictures: 940,
                ..set_plan(tmp.path(), stream_b, set_id)
            },
            jumps.clone(),
            reports.watch(),
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(jumps.starts(), vec![19]);
    assert!(
        dir.join(DONE_MARKER)
            .exists()
    );
}

#[tokio::test]
async fn a_pass_follows_the_viewer_to_a_part_with_no_pictures() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let reports = Reports::default();
    let set_id = Uuid::new_v4();
    let dir = set_dir(tmp.path(), set_id);
    let runner = Arc::new(FakeRunner {
        sheets: 38,
        pace: Duration::from_millis(15),
        ..Default::default()
    });
    reports.at("device-a", 0);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            PassPlan {
                pictures: 940,
                ..plan(tmp.path(), set_id)
            },
            runner.clone(),
            reports.watch(),
        )
        .await;
    until(|| {
        dir.join("1.jpg")
            .exists()
    })
    .await;
    // The viewer jumps to 1 h 20.
    reports.at("device-a", 80 * 60);
    until(|| !passes.is_queued(set_id)).await;
    let starts = runner.starts();
    assert_eq!(starts.len(), 3, "{starts:?}");
    assert_eq!(starts[..2], [0_u32, 19], "{starts:?}");
    assert_eq!(sheets_in(&dir), (0..38).collect::<Vec<u32>>());
    assert!(
        dir.join(DONE_MARKER)
            .exists()
    );
}

#[tokio::test]
async fn in_order_a_play_resumed_part_way_starts_at_the_beginning() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let reports = Reports::default();
    let set_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner {
        sheets: 38,
        ..Default::default()
    });
    // 1 h 20 in, on a source that can seek: sheet 19 of 38.
    reports.at("device-a", 80 * 60);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin(
            "play-a",
            in_order(tmp.path(), set_id),
            runner.clone(),
            reports.watch(),
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| !passes.is_queued(set_id)).await;
    let dir = set_dir(tmp.path(), set_id);
    assert_eq!(runner.starts(), vec![0], "one run, from the beginning");
    assert_eq!(sheets_in(&dir), (0..38).collect::<Vec<u32>>());
    assert!(
        dir.join(DONE_MARKER)
            .exists()
    );
}

#[tokio::test]
async fn in_order_an_unfinished_set_carries_on_at_its_first_missing_sheet() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let reports = Reports::default();
    let set_id = Uuid::new_v4();
    let dir = set_dir(tmp.path(), set_id);
    write_sheets(&dir, 0..5);
    let runner = Arc::new(FakeRunner {
        sheets: 38,
        ..Default::default()
    });
    // The viewer far above them, at sheet 19.
    reports.at("device-a", 80 * 60);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            in_order(tmp.path(), set_id),
            runner.clone(),
            reports.watch(),
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.starts(), vec![5], "not at the viewer's sheet");
    assert_eq!(sheets_in(&dir), (0..38).collect::<Vec<u32>>());
    assert!(
        dir.join(DONE_MARKER)
            .exists()
    );
}

#[tokio::test]
async fn in_order_a_gap_left_from_the_viewers_start_is_filled_first() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let reports = Reports::default();
    let set_id = Uuid::new_v4();
    // A pass that started at the viewer made sheets 19 to 37 and reached
    // the end; the beginning is missing.
    const ABOVE: &[u8] = &[0xFF, 0xD8, b'a', 0xFF, 0xD9];
    let dir = set_dir(tmp.path(), set_id);
    std::fs::create_dir_all(&dir).unwrap();
    for n in 19..38 {
        std::fs::write(dir.join(format!("{n}.jpg")), ABOVE).unwrap();
    }
    std::fs::write(dir.join(END_MARKER), b"").unwrap();
    let runner = Arc::new(FakeRunner {
        sheets: 38,
        ..Default::default()
    });
    reports.at("device-a", 100 * 60);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            in_order(tmp.path(), set_id),
            runner.clone(),
            reports.watch(),
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.starts(), vec![0]);
    assert_eq!(sheets_in(&dir), (0..38).collect::<Vec<u32>>());
    for n in 19..38 {
        assert_eq!(
            std::fs::read(dir.join(format!("{n}.jpg"))).unwrap(),
            ABOVE,
            "the run stopped where the sheets on disk begin: {n}"
        );
    }
    assert!(
        dir.join(DONE_MARKER)
            .exists()
    );
}

#[tokio::test]
async fn in_order_a_seek_to_a_part_with_no_pictures_is_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let reports = Reports::default();
    let set_id = Uuid::new_v4();
    let dir = set_dir(tmp.path(), set_id);
    let runner = Arc::new(FakeRunner {
        sheets: 38,
        pace: Duration::from_millis(15),
        ..Default::default()
    });
    reports.at("device-a", 0);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            in_order(tmp.path(), set_id),
            runner.clone(),
            reports.watch(),
        )
        .await;
    until(|| {
        dir.join("1.jpg")
            .exists()
    })
    .await;
    // The viewer seeks to 1 h 20.
    reports.at("device-a", 80 * 60);
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.starts(), vec![0], "no run at the seek");
    assert!(
        !runner
            .killed
            .load(Ordering::SeqCst),
        "the run was not stopped"
    );
    assert_eq!(sheets_in(&dir), (0..38).collect::<Vec<u32>>());
    assert!(
        dir.join(DONE_MARKER)
            .exists()
    );
}

#[tokio::test]
async fn a_file_longer_than_its_count_moves_to_its_own_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());

    // 940 pictures fill sheets 0 to 37; this file gave sheet 38 too.
    let (set_id, stream_id) = (Uuid::new_v4(), Uuid::new_v4());
    let runner = Arc::new(FakeRunner {
        sheets: 39,
        ..Default::default()
    });
    let item_a = Uuid::new_v4();
    passes.register("play-a", item_a, "device-a");
    passes
        .begin(
            "play-a",
            PassPlan {
                pictures: 940,
                ..set_plan(tmp.path(), stream_id, set_id)
            },
            runner.clone(),
            always(),
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert!(!set_dir(tmp.path(), set_id).exists());
    let own = set_dir(tmp.path(), stream_id);
    assert!(
        own.join(DONE_MARKER)
            .exists()
    );
    assert!(
        own.join("38.jpg")
            .exists()
    );
    // The play no longer names the moved set, so the sheet route
    // follows its pick to the stream's own folder.
    assert_eq!(passes.selected_set(item_a, "device-a"), None);

    // Up to the count's last sheet, the pictures stay in the set.
    let (set_id, stream_id) = (Uuid::new_v4(), Uuid::new_v4());
    let runner = Arc::new(FakeRunner {
        sheets: 38,
        ..Default::default()
    });
    let item_b = Uuid::new_v4();
    passes.register("play-b", item_b, "device-b");
    passes
        .begin(
            "play-b",
            PassPlan {
                pictures: 940,
                ..set_plan(tmp.path(), stream_id, set_id)
            },
            runner.clone(),
            always(),
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert!(
        set_dir(tmp.path(), set_id)
            .join(DONE_MARKER)
            .exists()
    );
    assert!(!set_dir(tmp.path(), stream_id).exists());
    assert_eq!(passes.selected_set(item_b, "device-b"), Some(set_id));
}

#[tokio::test]
async fn a_moved_set_stays_in_its_own_folder_and_makes_its_beginning_again() {
    let tmp = tempfile::tempdir().unwrap();
    let quiet_alone = Duration::from_millis(400);
    let passes = TrickplayPasses::with_limits(Limits {
        quiet_alone,
        ..fast()
    });
    let reports = Reports::default();
    let (set_id, stream_id) = (Uuid::new_v4(), Uuid::new_v4());
    // Another version of the same count made sheets 0 to 18.
    const OTHER: &[u8] = &[0xFF, 0xD8, b'o', 0xFF, 0xD9];
    let set = set_dir(tmp.path(), set_id);
    std::fs::create_dir_all(&set).unwrap();
    for n in 0..19 {
        std::fs::write(set.join(format!("{n}.jpg")), OTHER).unwrap();
    }
    // This file gives sheet 38 too, past the count's last. Its run from
    // the beginning stalls at sheet 5, once.
    let runner = Arc::new(FakeRunner {
        sheets: 39,
        stall_at: Some(5),
        ..Default::default()
    });
    reports.at("device-a", 80 * 60);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            PassPlan {
                pictures: 940,
                ..set_plan(tmp.path(), stream_id, set_id)
            },
            runner.clone(),
            reports.watch(),
        )
        .await;
    let own = set_dir(tmp.path(), stream_id);
    until_named("run 2 made sheet 4 in the stream's own folder", || {
        runner
            .runs
            .load(Ordering::SeqCst)
            == 2
            && own
                .join("4.jpg")
                .exists()
    })
    .await;
    assert!(!set.exists());

    // Its play goes quiet: the pass stands down.
    reports.silent("device-a", Duration::ZERO);
    until_named("the stalled run 2 stopped by standing down", || {
        runner
            .killed
            .load(Ordering::SeqCst)
    })
    .await;
    assert!(passes.is_queued(set_id));

    // The play reports again: the pass carries on in the stream's own
    // folder from its first missing sheet, keeping the sheets 0 to 4 it
    // made there, and the count's folder is not made again.
    reports.at("device-a", 80 * 60);
    until_named("the pass ended after the play reported again", || {
        !passes.is_queued(set_id)
    })
    .await;
    assert_eq!(runner.starts(), vec![19, 0, 5]);
    assert!(!set.exists(), "the count's folder is not made again");
    assert!(
        own.join(DONE_MARKER)
            .exists()
    );
    assert_eq!(sheets_in(&own), (0..39).collect::<Vec<u32>>());
    for n in 0..19 {
        assert_eq!(
            std::fs::read(own.join(format!("{n}.jpg"))).unwrap(),
            JPEG,
            "sheet {n} is made from this file"
        );
    }
}

#[tokio::test]
async fn a_run_from_the_last_sheet_that_makes_none_leaves_the_rest_to_make() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let reports = Reports::default();
    let set_id = Uuid::new_v4();
    let dir = set_dir(tmp.path(), set_id);
    write_sheets(&dir, 0..11);
    // 940 pictures fill sheets 0 to 37; this file ends with sheet 36.
    // The run from sheet 37 ends as ffmpeg does there: 234, no sheet.
    let runner = Arc::new(FakeRunner {
        sheets: 37,
        empty_exit: Some((234, PAST_END)),
        ..Default::default()
    });
    // The viewer is in the last sheet.
    reports.at("device-a", 37 * 250);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            PassPlan {
                pictures: 940,
                ..plan(tmp.path(), set_id)
            },
            runner.clone(),
            reports.watch(),
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(
        runner.starts(),
        vec![37, 11],
        "the end, then the sheets before it"
    );
    assert_eq!(sheets_in(&dir), (0..37).collect::<Vec<u32>>());
    assert!(
        dir.join(DONE_MARKER)
            .exists()
    );
    assert!(
        !passes.failed_recently(set_id, Instant::now()),
        "a run past the end is not a failure"
    );
}

#[tokio::test]
async fn a_run_does_not_follow_to_a_last_sheet_past_the_end() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let reports = Reports::default();
    let set_id = Uuid::new_v4();
    let dir = set_dir(tmp.path(), set_id);
    write_sheets(&dir, 0..11);
    // As above, but the run from sheet 11 takes longer than a follow
    // wait, and the viewer is reported in sheet 37 all the while:
    // that sheet never comes, so the run is not stopped to make it.
    let runner = Arc::new(FakeRunner {
        sheets: 37,
        empty_exit: Some((234, PAST_END)),
        pace: Duration::from_millis(20),
        ..Default::default()
    });
    reports.at("device-a", 37 * 250);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            PassPlan {
                pictures: 940,
                ..plan(tmp.path(), set_id)
            },
            runner.clone(),
            reports.watch(),
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.starts(), vec![37, 11], "no follow to sheet 37");
    assert_eq!(sheets_in(&dir), (0..37).collect::<Vec<u32>>());
    assert!(
        dir.join(DONE_MARKER)
            .exists()
    );
}

#[tokio::test]
async fn a_run_started_past_the_end_is_not_a_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let reports = Reports::default();
    let set_id = Uuid::new_v4();
    let dir = set_dir(tmp.path(), set_id);
    write_sheets(&dir, 0..36);
    // 940 pictures fill sheets 0 to 37; this file ends with sheet 35, so
    // a run from 36 finds it over: 234 and no sheet, as from ffmpeg.
    let runner = Arc::new(FakeRunner {
        sheets: 36,
        empty_exit: Some((234, PAST_END)),
        ..Default::default()
    });
    reports.at("device-a", 36 * 250);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            PassPlan {
                pictures: 940,
                ..plan(tmp.path(), set_id)
            },
            runner.clone(),
            reports.watch(),
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(
        runner.starts(),
        vec![36, 36],
        "over twice: the stored length is longer than the file"
    );
    assert_eq!(sheets_in(&dir), (0..36).collect::<Vec<u32>>());
    assert!(
        !passes.failed_recently(set_id, Instant::now()),
        "not held back for the failure wait"
    );
    assert!(
        !dir.join(END_MARKER)
            .exists()
    );
    assert!(
        !dir.join(DONE_MARKER)
            .exists()
    );
    assert!(
        !dir.join(BUSY_MARKER)
            .exists()
    );
}

#[tokio::test]
async fn a_run_whose_source_drops_after_the_seek_is_a_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let reports = Reports::default();
    let set_id = Uuid::new_v4();
    let dir = set_dir(tmp.path(), set_id);
    write_sheets(&dir, 0..11);
    // The same status as a run past the end, after a source error.
    const DROPPED: &str = "\
        [in#0/matroska,webm @ 0x7] Error during demuxing: Input/output error\n\
        [vost#0:0/mjpeg @ 0x2] [enc:mjpeg @ 0x3] Could not open encoder before EOF\n";
    let runner = Arc::new(FakeRunner {
        sheets: 0,
        empty_exit: Some((234, DROPPED)),
        ..Default::default()
    });
    reports.at("device-a", 37 * 250);
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            PassPlan {
                pictures: 940,
                ..plan(tmp.path(), set_id)
            },
            runner.clone(),
            reports.watch(),
        )
        .await;
    until(|| !passes.is_queued(set_id)).await;
    assert_eq!(runner.starts(), vec![37], "one run, then the failure wait");
    assert!(passes.failed_recently(set_id, Instant::now()));
    assert_eq!(sheets_in(&dir), (0..11).collect::<Vec<u32>>());
    assert!(
        !dir.join(END_MARKER)
            .exists(),
        "no sheet is marked as never coming"
    );
}

#[tokio::test]
async fn the_recorded_set_goes_with_its_play() {
    let passes = TrickplayPasses::with_limits(fast());
    let item_id = Uuid::new_v4();
    let (long_set, short_set) = (Uuid::new_v4(), Uuid::new_v4());
    passes.register("play-a", item_id, "device-a");
    assert!(passes.attach("play-a", long_set));
    assert_eq!(passes.selected_set(item_id, "device-a"), Some(long_set));
    passes.release("play-a");
    assert_eq!(
        passes.selected_set(item_id, "device-a"),
        None,
        "forgotten with the play, not after the grace period"
    );

    // A new play forgets the last play's set until its own is known.
    passes.register("play-b", item_id, "device-a");
    assert!(passes.attach("play-b", long_set));
    passes.register("play-c", item_id, "device-a");
    assert_eq!(passes.selected_set(item_id, "device-a"), None);
    assert!(passes.attach("play-c", short_set));
    // The older play's stop leaves the newer play's set alone.
    passes.release("play-b");
    assert_eq!(passes.selected_set(item_id, "device-a"), Some(short_set));

    // So does an older play's stop on the same set.
    passes.register("play-d", item_id, "device-a");
    assert!(passes.attach("play-d", short_set));
    passes.release("play-c");
    assert_eq!(passes.selected_set(item_id, "device-a"), Some(short_set));
    passes.release("play-d");
    assert_eq!(passes.selected_set(item_id, "device-a"), None);
}

#[tokio::test]
async fn an_unwatched_pass_gives_way_to_a_watched_one() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let reports = Reports::default();
    let (set_a, set_b) = (Uuid::new_v4(), Uuid::new_v4());
    let slow = Arc::new(FakeRunner {
        sheets: 2,
        wait_for_cancel: true,
        ..Default::default()
    });
    let quick = Arc::new(FakeRunner {
        sheets: 29,
        ..Default::default()
    });
    reports.fresh("device-a");
    passes.register("play-a", Uuid::new_v4(), "device-a");
    let begun = passes
        .begin(
            "play-a",
            plan(tmp.path(), set_a),
            slow.clone(),
            reports.watch(),
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| {
        slow.runs
            .load(Ordering::SeqCst)
            == 1
    })
    .await;

    // A goes silent. With nobody waiting it keeps the slot.
    reports.silent("device-a", Duration::from_secs(1));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !slow
            .killed
            .load(Ordering::SeqCst)
    );

    // B is watched and waiting: A gives way and B runs.
    reports.fresh("device-b");
    passes.register("play-b", Uuid::new_v4(), "device-b");
    let begun = passes
        .begin(
            "play-b",
            plan(tmp.path(), set_b),
            quick.clone(),
            reports.watch(),
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| {
        slow.killed
            .load(Ordering::SeqCst)
    })
    .await;
    until(|| {
        set_dir(tmp.path(), set_b)
            .join(DONE_MARKER)
            .exists()
    })
    .await;
    // A stood down: its job and its sheets are still there.
    assert!(passes.is_queued(set_a));
    assert_eq!(sheets_in(&set_dir(tmp.path(), set_a)), vec![0, 1]);
    assert_eq!(
        slow.runs
            .load(Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn an_unwatched_waiter_does_not_take_the_slot() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let reports = Reports::default();
    let (set_a, set_b) = (Uuid::new_v4(), Uuid::new_v4());
    let slow = Arc::new(FakeRunner {
        sheets: 2,
        wait_for_cancel: true,
        ..Default::default()
    });
    let quick = Arc::new(FakeRunner {
        sheets: 29,
        ..Default::default()
    });
    reports.fresh("device-a");
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            plan(tmp.path(), set_a),
            slow.clone(),
            reports.watch(),
        )
        .await;
    until(|| {
        slow.runs
            .load(Ordering::SeqCst)
            == 1
    })
    .await;

    // Both silent: B's device still has a session on the item, so B is
    // queued, but it does not wait for the slot and A keeps it.
    reports.silent("device-a", Duration::from_secs(1));
    reports.silent("device-b", Duration::from_secs(1));
    passes.register("play-b", Uuid::new_v4(), "device-b");
    let begun = passes
        .begin(
            "play-b",
            plan(tmp.path(), set_b),
            quick.clone(),
            reports.watch(),
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !slow
            .killed
            .load(Ordering::SeqCst)
    );
    assert!(!passes.is_waiting(set_b));
    assert_eq!(
        quick
            .runs
            .load(Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn alone_a_pass_stands_down_late_and_comes_back_when_watched() {
    let tmp = tempfile::tempdir().unwrap();
    let quiet_alone = Duration::from_millis(400);
    let passes = TrickplayPasses::with_limits(Limits {
        quiet_alone,
        ..fast()
    });
    let reports = Reports::default();
    let set_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner {
        sheets: 3,
        wait_for_cancel: true,
        ..Default::default()
    });
    reports.fresh("device-a");
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            plan(tmp.path(), set_id),
            runner.clone(),
            reports.watch(),
        )
        .await;
    until(|| {
        runner
            .runs
            .load(Ordering::SeqCst)
            == 1
    })
    .await;

    let silent_from = Instant::now();
    reports.silent("device-a", Duration::ZERO);
    // Past the one-minute rule, but nobody is waiting.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !runner
            .killed
            .load(Ordering::SeqCst)
    );
    until(|| {
        runner
            .killed
            .load(Ordering::SeqCst)
    })
    .await;
    assert!(silent_from.elapsed() >= quiet_alone);
    assert_eq!(sheets_in(&set_dir(tmp.path(), set_id)), vec![0, 1, 2]);
    assert!(passes.is_queued(set_id));

    // Its play reports again: the same job runs again, with no new begin.
    reports.fresh("device-a");
    until(|| {
        runner
            .runs
            .load(Ordering::SeqCst)
            == 2
    })
    .await;
}

#[tokio::test]
async fn a_cancel_ends_a_stood_down_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(Limits {
        quiet_alone: Duration::from_millis(150),
        ..fast()
    });
    let reports = Reports::default();
    let set_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner {
        sheets: 1,
        wait_for_cancel: true,
        ..Default::default()
    });
    reports.fresh("device-a");
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            plan(tmp.path(), set_id),
            runner.clone(),
            reports.watch(),
        )
        .await;
    until(|| {
        runner
            .runs
            .load(Ordering::SeqCst)
            == 1
    })
    .await;
    reports.silent("device-a", Duration::ZERO);
    until(|| {
        runner
            .killed
            .load(Ordering::SeqCst)
    })
    .await;

    // The play stops: after the grace period the stood-down job ends, so
    // a report from the device no longer brings it back.
    passes.release("play-a");
    until(|| !passes.has_pass(set_id)).await;
    reports.fresh("device-a");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        runner
            .runs
            .load(Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn a_play_whose_device_moved_to_another_set_gives_way() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let reports = Reports::default();
    let item_id = Uuid::new_v4();
    let (set_a, set_b) = (Uuid::new_v4(), Uuid::new_v4());
    let slow = Arc::new(FakeRunner {
        sheets: 2,
        wait_for_cancel: true,
        ..Default::default()
    });
    let quick = Arc::new(FakeRunner {
        sheets: 29,
        ..Default::default()
    });
    reports.playing("device-a", "play-a");
    passes.register("play-a", item_id, "device-a");
    passes
        .begin(
            "play-a",
            plan(tmp.path(), set_a),
            slow.clone(),
            reports.watch(),
        )
        .await;
    until(|| {
        slow.runs
            .load(Ordering::SeqCst)
            == 1
    })
    .await;

    // The device plays a version of another length of the same title;
    // play-a's stop never came. The device's reports are play-b's.
    reports.playing("device-a", "play-b");
    passes.register("play-b", item_id, "device-a");
    let begun = passes
        .begin(
            "play-b",
            plan(tmp.path(), set_b),
            quick.clone(),
            reports.watch(),
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| {
        slow.killed
            .load(Ordering::SeqCst)
    })
    .await;
    until(|| {
        set_dir(tmp.path(), set_b)
            .join(DONE_MARKER)
            .exists()
    })
    .await;
    assert_eq!(sheets_in(&set_dir(tmp.path(), set_a)), vec![0, 1]);
}

#[tokio::test]
async fn a_waiter_with_no_plays_left_does_not_take_the_slot() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(Limits {
        grace: Duration::from_secs(5),
        ..fast()
    });
    let reports = Reports::default();
    let (set_a, set_b) = (Uuid::new_v4(), Uuid::new_v4());
    let slow = Arc::new(FakeRunner {
        sheets: 2,
        wait_for_cancel: true,
        ..Default::default()
    });
    let quick = Arc::new(FakeRunner {
        sheets: 29,
        ..Default::default()
    });
    reports.fresh("device-a");
    passes.register("play-a", Uuid::new_v4(), "device-a");
    passes
        .begin(
            "play-a",
            plan(tmp.path(), set_a),
            slow.clone(),
            reports.watch(),
        )
        .await;
    until(|| {
        slow.runs
            .load(Ordering::SeqCst)
            == 1
    })
    .await;
    reports.fresh("device-b");
    passes.register("play-b", Uuid::new_v4(), "device-b");
    let begun = passes
        .begin(
            "play-b",
            plan(tmp.path(), set_b),
            quick.clone(),
            reports.watch(),
        )
        .await;
    assert_eq!(begun, Begin::Queued);
    until(|| passes.is_waiting(set_b)).await;

    // B's only play stops while it waits: it stops waiting, so A, quiet
    // for a minute, keeps the slot through B's grace period.
    passes.release("play-b");
    until(|| !passes.is_waiting(set_b)).await;
    reports.silent("device-a", Duration::from_secs(1));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !slow
            .killed
            .load(Ordering::SeqCst)
    );
    assert_eq!(
        quick
            .runs
            .load(Ordering::SeqCst),
        0
    );
}

#[test]
fn a_slow_moment_marks_a_source_only_for_a_while() {
    let passes = TrickplayPasses::with_limits(Limits {
        no_jump_wait: Duration::from_millis(50),
        ..fast()
    });
    let stream_id = Uuid::new_v4();
    passes.mark_no_jump(stream_id);
    assert!(!passes.can_jump(stream_id));
    std::thread::sleep(Duration::from_millis(80));
    assert!(passes.can_jump(stream_id), "a run may start part way again");

    // With the production wait, the mark holds for now.
    let passes = TrickplayPasses::new();
    passes.mark_no_jump(stream_id);
    assert!(!passes.can_jump(stream_id));
}

#[cfg(unix)]
#[tokio::test]
async fn the_keep_time_is_read_when_the_pass_takes_the_slot() {
    let day = Duration::from_secs(24 * 60 * 60);
    let settings = |keep_days: i64| api::ServerConfiguration {
        trickplay_while_streaming: Some(true),
        trickplay_cache_size_mb: Some(1024),
        trickplay_keep_days: Some(keep_days),
        ..Default::default()
    };
    for (keep_days, old_kept) in [(1, false), (30, true)] {
        let tmp = tempfile::tempdir().unwrap();
        // The keep time when the play starts: one day.
        let pool = settings_db(true, 1024).await;
        db::Settings::set_config(&pool, &settings(1))
            .await
            .unwrap();
        let passes = TrickplayPasses::with_limits(fast());
        // A set last shown three days ago.
        let old = set_dir(tmp.path(), Uuid::new_v4());
        write_sheets(&old, 0..3);
        let shown = SystemTime::now() - 3 * day;
        std::fs::File::open(&old)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(shown))
            .unwrap();
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
        let stream_id = Uuid::new_v4();
        passes.register("play-a", Uuid::new_v4(), "device-a");
        let begun = passes
            .begin(
                "play-a",
                PassPlan {
                    db: Some(pool.clone()),
                    ..plan(tmp.path(), stream_id)
                },
                runner.clone(),
                always(),
            )
            .await;
        assert_eq!(begun, Begin::Queued);
        // Saved after the play started, before its pass takes the slot.
        db::Settings::set_config(&pool, &settings(keep_days))
            .await
            .unwrap();
        drop(held);
        until(|| !passes.is_queued(stream_id)).await;
        assert_eq!(old.exists(), old_kept, "keep time {keep_days} days");
        assert!(
            set_dir(tmp.path(), stream_id)
                .join(DONE_MARKER)
                .exists()
        );
    }
}
