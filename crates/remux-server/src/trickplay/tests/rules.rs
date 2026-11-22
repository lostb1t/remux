//! Settings, sets, eligibility, the `Trickplay` field and the stand-in rules.

use super::*;

#[test]
fn off_by_default() {
    let cfg = api::ServerConfiguration::default();
    assert_eq!(cfg.trickplay_while_streaming, Some(false));
    assert_eq!(cfg.trickplay_cache_size_mb, Some(1024));
    assert_eq!(cfg.trickplay_keep_days, Some(30));
    assert!(!enabled(&cfg));
    assert_eq!(cfg.trickplay_start_at_position, Some(false));
    assert!(!start_at_position(&cfg));
    let start = |main: bool, on: bool| api::ServerConfiguration {
        trickplay_while_streaming: Some(main),
        trickplay_start_at_position: Some(on),
        ..Default::default()
    };
    assert!(!start_at_position(&start(true, false)));
    assert!(
        !start_at_position(&start(false, true)),
        "read only while the main switch is on"
    );
    assert!(start_at_position(&start(true, true)));
    let saved: api::ServerConfiguration =
        serde_json::from_value(serde_json::json!({ "TrickplayWhileStreaming": true }))
            .unwrap();
    assert!(
        !start_at_position(&saved),
        "a saved document without it reads as off"
    );
    let small = api::ServerConfiguration {
        trickplay_cache_size_mb: Some(1),
        ..Default::default()
    };
    assert_eq!(cap_bytes(&small), 100 * 1024 * 1024);
}

#[test]
fn keep_for_reads_days_and_zero_means_never() {
    let day = Duration::from_secs(24 * 60 * 60);
    let with = |days: Option<i64>| {
        keep_for(&api::ServerConfiguration {
            trickplay_keep_days: days,
            ..Default::default()
        })
    };
    assert_eq!(with(None), Some(30 * day));
    assert_eq!(with(Some(7)), Some(7 * day));
    assert_eq!(with(Some(0)), None);
    assert_eq!(with(Some(-5)), None);
    assert_eq!(with(Some(99_999)), Some(3_650 * day));

    // A saved document from before the setting reads as 30 days.
    let saved: api::ServerConfiguration =
        serde_json::from_value(serde_json::json!({ "TrickplayWhileStreaming": true }))
            .unwrap();
    assert_eq!(saved.trickplay_keep_days, Some(30));
}

#[test]
fn picture_count_rounds_up_per_ten_seconds() {
    assert_eq!(picture_count(7_200 * TICKS_PER_SEC), 720);
    assert_eq!(picture_count(7_205 * TICKS_PER_SEC), 721);
    assert_eq!(picture_count(1), 1);
    assert_eq!(picture_count(0), 0);
    assert_eq!(picture_count(-5), 0);
}

#[test]
fn set_id_shares_one_count_under_one_title() {
    let film = Uuid::new_v4();
    let ffprobe = Some(api::ProbeOrigin::Ffprobe);
    let remuxdb = Some(api::ProbeOrigin::RemuxDb);
    let a = measured(Some(film), 9_391.756, ffprobe);
    let b = measured(Some(film), 9_391.776, remuxdb);
    assert_ne!(a.id, b.id);
    assert_eq!(set_id(&a), set_id(&b), "same picture count, same title");
    assert_ne!(set_id(&a), a.id);

    let shorter = measured(Some(film), 9_382.4, ffprobe);
    assert_ne!(set_id(&a), set_id(&shorter), "939 pictures, not 940");

    let other_title = measured(Some(Uuid::new_v4()), 9_391.756, ffprobe);
    assert_ne!(set_id(&a), set_id(&other_title));

    // Anything not measured, or not a stream under a title, keeps its own.
    let guessed =
        measured(Some(film), 9_391.756, Some(api::ProbeOrigin::FilenameGuess));
    assert_eq!(set_id(&guessed), guessed.id);
    let unknown_origin = measured(Some(film), 9_391.756, None);
    assert_eq!(set_id(&unknown_origin), unknown_origin.id);
    let no_probe = db::Media {
        probe_data: None,
        ..measured(Some(film), 9_391.756, ffprobe)
    };
    assert_eq!(set_id(&no_probe), no_probe.id);
    let no_parent = measured(None, 9_391.756, ffprobe);
    assert_eq!(set_id(&no_parent), no_parent.id);
    let not_a_stream = db::Media {
        kind: db::MediaKind::Movie,
        ..measured(Some(film), 9_391.756, ffprobe)
    };
    assert_eq!(set_id(&not_a_stream), not_a_stream.id);
    let zero = measured(Some(film), 0.0, ffprobe);
    assert_eq!(set_id(&zero), zero.id);

    // One episode, played from its own file and from a file holding two.
    let episode = Uuid::new_v4();
    let single = measured(Some(episode), 720.0, ffprobe);
    let combined = measured(Some(episode), 1_407.0, ffprobe);
    assert_ne!(set_id(&single), set_id(&combined));
}

#[test]
fn a_streams_own_folder_wins() {
    let tmp = tempfile::tempdir().unwrap();
    let stream = measured(
        Some(Uuid::new_v4()),
        9_391.756,
        Some(api::ProbeOrigin::Ffprobe),
    );
    assert_eq!(set_for(tmp.path(), &stream), set_id(&stream));
    std::fs::create_dir_all(set_dir(tmp.path(), stream.id)).unwrap();
    assert_eq!(set_for(tmp.path(), &stream), stream.id);
}

#[test]
fn manifest_counts_each_source_by_its_own_length() {
    let item_id = Uuid::new_v4();
    let combined = Uuid::new_v4();
    let unmeasured = Uuid::new_v4();
    let key = |id: Uuid| {
        id.as_simple()
            .to_string()
    };
    let source = |id: Uuid, secs: Option<i64>| api::MediaSourceInfo {
        id,
        run_time_ticks: secs.map(|s| s * TICKS_PER_SEC),
        ..Default::default()
    };

    // A title listed at 720 s, with a file holding two episodes and a
    // source of no known length.
    let dto = api::BaseItemDto {
        id: item_id,
        type_: api::MediaType::Episode,
        run_time_ticks: Some(720 * TICKS_PER_SEC),
        media_sources: Some(vec![
            source(combined, Some(1_407)),
            source(unmeasured, None),
        ]),
        ..Default::default()
    };
    let out = manifest(&dto, |_| true);
    assert_eq!(out[&key(combined)]["320"].thumbnail_count, 141);
    assert_eq!(out[&key(unmeasured)]["320"].thumbnail_count, 72);

    // No listed runtime: a source of known length is still advertised.
    let dto = api::BaseItemDto {
        run_time_ticks: None,
        media_sources: Some(vec![source(combined, Some(1_407))]),
        ..dto
    };
    let out = manifest(&dto, |_| true);
    assert_eq!(out[&key(combined)]["320"].thumbnail_count, 141);

    // The first source carries the item id and the first row's length;
    // another source is told its own.
    let second = Uuid::new_v4();
    let dto = api::BaseItemDto {
        id: item_id,
        type_: api::MediaType::Movie,
        run_time_ticks: Some(9_391 * TICKS_PER_SEC),
        media_sources: Some(vec![
            source(item_id, Some(9_391)),
            source(second, Some(7_800)),
        ]),
        ..Default::default()
    };
    let out = manifest(&dto, |_| true);
    assert_eq!(out[&key(item_id)]["320"].thumbnail_count, 940);
    assert_eq!(out[&key(second)]["320"].thumbnail_count, 780);

    // Each source is judged by its own length: under a title listed at
    // 4 min, a source of no known length gets nothing, a 2 h file does.
    let short = Uuid::new_v4();
    let dto = api::BaseItemDto {
        id: item_id,
        type_: api::MediaType::Movie,
        run_time_ticks: Some(240 * TICKS_PER_SEC),
        media_sources: Some(vec![source(short, None), source(second, Some(7_200))]),
        ..Default::default()
    };
    let out = manifest(&dto, |_| true);
    assert!(!out.contains_key(&key(short)));
    assert_eq!(out[&key(second)]["320"].thumbnail_count, 720);
}

#[test]
fn manifest_counts_from_runtime() {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let refused = Uuid::new_v4();
    let dto = item(
        api::MediaType::Movie,
        Some(7_200),
        &[first, second, refused],
    );
    let refused_key = refused
        .as_simple()
        .to_string();
    let out = manifest(&dto, |id| id != refused_key);
    assert_eq!(out.len(), 2);
    let entry = &out[&first
        .as_simple()
        .to_string()]["320"];
    assert_eq!(entry.thumbnail_count, 720);
    assert_eq!((entry.width, entry.height), (320, 180));
    assert_eq!((entry.tile_width, entry.tile_height), (5, 5));
    assert_eq!(entry.interval, 10_000);
    assert!(
        out.contains_key(
            &second
                .as_simple()
                .to_string()
        )
    );
    assert!(!out.contains_key(&refused_key));
    assert_eq!(
        dto.id
            .as_simple()
            .to_string(),
        first
            .as_simple()
            .to_string()
    );

    let longer = item(api::MediaType::Episode, Some(7_205), &[first]);
    let out = manifest(&longer, |_| true);
    assert_eq!(
        out[&first
            .as_simple()
            .to_string()]["320"]
            .thumbnail_count,
        721
    );

    let json = serde_json::to_value(&out).unwrap();
    let value = &json[first
        .as_simple()
        .to_string()]["320"];
    assert_eq!(value["ThumbnailCount"], 721);
    assert_eq!(value["TileWidth"], 5);
    assert_eq!(value["Bandwidth"], 10_400);
}

#[test]
fn manifest_empty_for_live_short_long_unknown() {
    let id = Uuid::new_v4();
    for dto in [
        item(api::MediaType::TvChannel, Some(7_200), &[id]),
        item(api::MediaType::Movie, None, &[id]),
        item(api::MediaType::Movie, Some(0), &[id]),
        item(api::MediaType::Movie, Some(299), &[id]),
        item(api::MediaType::Movie, Some(4 * 3_600 + 1), &[id]),
        item(api::MediaType::Audio, Some(7_200), &[id]),
    ] {
        assert!(manifest(&dto, |_| true).is_empty(), "{:?}", dto.type_);
    }
    let mut infinite = item(api::MediaType::Video, Some(7_200), &[id]);
    if let Some(sources) = infinite
        .media_sources
        .as_mut()
    {
        sources[0].is_infinite_stream = true;
    }
    assert!(manifest(&infinite, |_| true).is_empty());
    let stream = item(api::MediaType::Video, Some(300), &[id]);
    assert!(!manifest(&stream, |_| true).is_empty());
}

#[test]
fn manifest_without_sources_is_keyed_by_the_item() {
    let id = Uuid::new_v4();
    let dto = api::BaseItemDto {
        media_sources: None,
        ..item(api::MediaType::Episode, Some(1_800), &[id])
    };
    let out = manifest(&dto, |_| true);
    let keys: Vec<&String> = out
        .keys()
        .collect();
    assert_eq!(
        keys,
        vec![
            &id.as_simple()
                .to_string()
        ]
    );
    assert!(manifest(&dto, |_| false).is_empty());
}

#[test]
fn eligible_row_rejects_each_reason() {
    let ok = Some(RUNTIME);
    let (mut live, info) = row(StreamDescriptor::http(URL));
    live.kind = db::MediaKind::TvChannel;
    assert_eq!(eligible_row(&live, &info, ok), Err(Ineligible::LiveTv));

    let (mut under_channel, info) = row(StreamDescriptor::http(URL));
    under_channel.parent = Some(Arc::new(db::Media {
        kind: db::MediaKind::TvChannel,
        ..Default::default()
    }));
    assert_eq!(
        eligible_row(&under_channel, &info, ok),
        Err(Ineligible::LiveTv)
    );

    let (media, mut info) = row(StreamDescriptor::http(URL));
    info.catchup_source = Some("http://example.invalid/{utc}".to_string());
    assert_eq!(eligible_row(&media, &info, ok), Err(Ineligible::Catchup));

    let (media, info) = row(StreamDescriptor::rtsp("rtsp://example.invalid/live"));
    assert_eq!(eligible_row(&media, &info, ok), Err(Ineligible::Rtsp));

    let (media, info) = row(StreamDescriptor::Torrent {
        info_hash: "0123456789abcdef0123456789abcdef01234567".to_string(),
        file_hint: None,
        file_idx: None,
        trackers: vec![],
    });
    assert_eq!(eligible_row(&media, &info, ok), Err(Ineligible::Torrent));

    let (mut media, info) = row(StreamDescriptor::http("http://example.invalid/a"));
    media.probe_data = Some(api::MediaSourceInfo {
        container: Some(api::VideoContainer::Other("hls".to_string())),
        ..Default::default()
    });
    assert_eq!(eligible_row(&media, &info, ok), Err(Ineligible::Hls));

    let (media, info) = row(StreamDescriptor::http("http://example.invalid/list.m3u8"));
    assert_eq!(eligible_row(&media, &info, ok), Err(Ineligible::Hls));

    let (media, info) = row(StreamDescriptor::http(URL));
    assert_eq!(
        eligible_row(&media, &info, None),
        Err(Ineligible::NoRuntime)
    );
    assert_eq!(
        eligible_row(&media, &info, Some(299)),
        Err(Ineligible::TooShort)
    );
    assert_eq!(
        eligible_row(&media, &info, Some(4 * 3_600 + 1)),
        Err(Ineligible::TooLong)
    );
}

#[test]
fn eligible_pass_rejects_iptv_kind_and_redirect() {
    assert_eq!(
        eligible_pass(Some(&addon("iptv", false))),
        Err(Ineligible::Iptv)
    );
    assert_eq!(
        eligible_pass(Some(&addon("iptv-m3u", false))),
        Err(Ineligible::Iptv)
    );
    assert_eq!(
        eligible_pass(Some(&addon("iptv-xtream", false))),
        Err(Ineligible::Iptv)
    );
    assert_eq!(
        eligible_pass(Some(&addon("stremio", true))),
        Err(Ineligible::Redirect)
    );
}

/// The rule comes from the addon's own switches: "seek previews" lets its
/// streams be read, "stream" decides whether viewers also see them, and
/// a disabled addon is never read.
#[test]
fn the_rule_follows_the_four_switch_combinations_and_enabled() {
    let with = |resources: Vec<ResourceType>| {
        AddonRule::new(&Addon {
            resources,
            ..addon("stremio", false)
        })
    };
    let both = with(vec![ResourceType::Stream, ResourceType::SeekPreviews]);
    assert!(both.reads && !both.preview_only && both.allows());
    let only_previews = with(vec![ResourceType::SeekPreviews]);
    assert!(only_previews.reads && only_previews.preview_only);
    assert!(only_previews.allows());
    let only_stream = with(vec![ResourceType::Stream]);
    assert!(!only_stream.reads && !only_stream.preview_only);
    assert!(!only_stream.allows());
    let neither = with(vec![ResourceType::Meta]);
    assert!(!neither.reads && !neither.preview_only);

    let disabled = AddonRule::new(&Addon {
        enabled: false,
        ..preview_addon()
    });
    assert!(!disabled.reads && !disabled.preview_only);
    let iptv = AddonRule::new(&Addon {
        resources: vec![ResourceType::SeekPreviews],
        ..addon("iptv-m3u", false)
    });
    assert!(!iptv.reads && !iptv.preview_only);

    // "Bypass Remux proxy" acts only while "stream" is on: its control is
    // hidden with it off.
    let bypass = |resources: Vec<ResourceType>| {
        AddonRule::new(&Addon {
            resources,
            ..addon("stremio", true)
        })
    };
    assert!(!bypass(vec![ResourceType::Stream, ResourceType::SeekPreviews]).allows());
    assert!(bypass(vec![ResourceType::SeekPreviews]).allows());

    assert_eq!(
        eligible_pass(Some(&switched_off(&addon("stremio", false)))),
        Err(Ineligible::AddonExcluded)
    );
    assert_eq!(
        eligible_pass(Some(&Addon {
            enabled: false,
            ..addon("stremio", false)
        })),
        Err(Ineligible::AddonExcluded)
    );
    assert!(previews_only(&[ResourceType::SeekPreviews]));
    assert!(!previews_only(&[
        ResourceType::Stream,
        ResourceType::SeekPreviews
    ]));
    assert!(!previews_only(&[]));
}

#[test]
fn addon_ok_follows_the_snapshot() {
    let passes = TrickplayPasses::new();
    let off = switched_off(&addon("stremio", false));
    let iptv = addon("iptv-m3u", false);
    let redirect = addon("stremio", true);
    let on = addon("stremio", false);
    let disabled = Addon {
        enabled: false,
        ..addon("stremio", false)
    };
    passes.set_addon_rules(&[
        off.clone(),
        iptv.clone(),
        redirect.clone(),
        on.clone(),
        disabled.clone(),
    ]);
    let rules = passes.addon_rules();
    let from = |addon_id: Option<Uuid>| StreamInfo {
        descriptor: StreamDescriptor::http(URL),
        addon_id,
        ..Default::default()
    };
    assert!(!addon_ok(&rules, &from(Some(off.id))));
    assert!(!addon_ok(&rules, &from(Some(iptv.id))));
    assert!(!addon_ok(&rules, &from(Some(redirect.id))));
    assert!(!addon_ok(&rules, &from(Some(disabled.id))));
    assert!(addon_ok(&rules, &from(Some(on.id))));
    // A deleted addon's leftovers are not read; a stream that names no
    // addon is judged by its row alone.
    assert!(!addon_ok(&rules, &from(Some(Uuid::new_v4()))));
    assert!(addon_ok(&rules, &from(None)));
    assert!(!has_preview_source(&rules));

    passes.set_addon_rules(&[on.clone()]);
    assert!(!addon_ok(&passes.addon_rules(), &from(Some(off.id))));
    assert!(addon_ok(&passes.addon_rules(), &from(Some(on.id))));

    let dedicated = preview_addon();
    passes.set_addon_rules(&[on, dedicated.clone()]);
    assert!(has_preview_source(&passes.addon_rules()));
    assert!(addon_ok(&passes.addon_rules(), &from(Some(dedicated.id))));
    passes.set_addon_rules(&[Addon {
        enabled: false,
        ..dedicated
    }]);
    assert!(
        !has_preview_source(&passes.addon_rules()),
        "a disabled addon is not a preview source"
    );
}

/// A user is asked, and read, only the addons a viewer's lookup would ask
/// for them.
#[test]
fn the_scope_follows_the_users_list_default_addons_and_system_ones() {
    let listed = preview_addon();
    let not_default = Addon {
        is_default: false,
        ..preview_addon()
    };
    let system = Addon {
        is_default: false,
        system: true,
        ..preview_addon()
    };
    let rules = rules_of(&[&listed, &not_default, &system]);
    assert!(in_scope(&rules, listed.id, None));
    assert!(!in_scope(&rules, not_default.id, None));
    assert!(in_scope(&rules, system.id, None));
    let own = [not_default.id];
    assert!(!in_scope(&rules, listed.id, Some(&own)));
    assert!(in_scope(&rules, not_default.id, Some(&own)));
    assert!(in_scope(&rules, system.id, Some(&own)));
    assert!(!in_scope(&rules, Uuid::new_v4(), None), "deleted");
}

#[test]
fn eligible_accepts_http_local_opendal() {
    for descriptor in [
        StreamDescriptor::http(URL),
        StreamDescriptor::Local("/media/invented-film.mkv".into()),
        StreamDescriptor::Opendal {
            addon_id: Uuid::new_v4(),
            path: "films/invented-film.mkv".to_string(),
        },
    ] {
        let (media, info) = row(descriptor);
        assert_eq!(eligible_row(&media, &info, Some(RUNTIME)), Ok(()));
    }
    assert_eq!(eligible_pass(None), Ok(()));
    assert_eq!(eligible_pass(Some(&addon("stremio", false))), Ok(()));
}

#[test]
fn runtime_prefers_probe_then_item_then_listed() {
    let (mut media, _) = row(StreamDescriptor::http(URL));
    if let Some(info) = media
        .stream_info
        .as_mut()
    {
        info.duration = Some(1_000);
    }
    assert_eq!(runtime_s(&media, None), Some(1_000));
    assert_eq!(runtime_s(&media, Some(2_000)), Some(2_000));
    media.probe_data = Some(api::MediaSourceInfo {
        run_time_ticks: Some(3_000 * TICKS_PER_SEC),
        ..Default::default()
    });
    assert_eq!(runtime_s(&media, Some(2_000)), Some(3_000));
}

#[test]
fn parse_sheet_accepts_only_plain_numbers() {
    assert_eq!(parse_sheet("0.jpg"), Some(0));
    assert_eq!(parse_sheet("12.jpg"), Some(12));
    assert_eq!(parse_sheet("999999.jpg"), Some(999_999));
    for bad in [
        "x.jpg",
        "-1.jpg",
        "+1.jpg",
        "01.jpg",
        "1234567.jpg",
        "1.png",
        "1.jpg.tmp",
        "../0.jpg",
        ".jpg",
        "",
    ] {
        assert_eq!(parse_sheet(bad), None, "{bad}");
    }
}

#[tokio::test]
async fn short_pass_is_failure_and_drops_last_sheet() {
    let tmp = tempfile::tempdir().unwrap();
    let passes = TrickplayPasses::with_limits(fast());
    let stream_id = Uuid::new_v4();
    let runner = Arc::new(FakeRunner {
        sheets: 10,
        ..Default::default()
    });
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
    // A stream's own set: its count is a guess, so the route is not told it.
    assert_eq!(passes.pictures_of(stream_id), None);
    until(|| !passes.is_queued(stream_id)).await;

    let dir = set_dir(tmp.path(), stream_id);
    assert!(
        !dir.join(DONE_MARKER)
            .exists()
    );
    assert!(
        !dir.join("9.jpg")
            .exists()
    );
    assert!(
        dir.join("8.jpg")
            .exists()
    );
    assert!(passes.failed_recently(stream_id, Instant::now()));
}

#[test]
fn other_versions_are_only_those_certain_to_fit_and_cheap_to_ask_for() {
    let title = Uuid::new_v4();
    let on = addon("stremio", false);
    let rules = rules_of(&[&on]);
    let none_own = |_: Uuid| false;
    let playing = version(title, 7_200.0, on.id, 0);
    let fits = version(title, 7_195.0, on.id, 1);
    let ids = |found: Vec<&db::Media>| -> Vec<Uuid> {
        found
            .iter()
            .map(|m| m.id)
            .collect()
    };
    assert_eq!(
        ids(other_versions(
            &playing,
            &[playing.clone(), fits.clone()],
            None,
            &rules,
            none_own
        )),
        vec![fits.id],
        "the same count, cached at a named service; never the version played"
    );

    // Each row below breaks one rule.
    let mut refused: Vec<(&str, db::Media)> = vec![
        ("another count", version(title, 7_300.0, on.id, 2)),
        (
            "a length from the file name",
            db::Media {
                probe_data: measured(
                    Some(title),
                    7_200.0,
                    Some(api::ProbeOrigin::FilenameGuess),
                )
                .probe_data,
                ..version(title, 7_200.0, on.id, 2)
            },
        ),
        (
            "no probe data",
            db::Media {
                probe_data: None,
                ..version(title, 7_200.0, on.id, 2)
            },
        ),
    ];
    let mut row = version(title, 7_200.0, on.id, 2);
    info_of(&mut row).service_cached = None;
    refused.push(("not marked cached", row));
    let mut row = version(title, 7_200.0, on.id, 2);
    info_of(&mut row).service_cached = Some(false);
    refused.push(("marked not cached", row));
    let mut row = version(title, 7_200.0, on.id, 2);
    info_of(&mut row).service_id = None;
    refused.push(("cached with no service, as IPTV on demand is listed", row));
    let mut row = version(title, 7_200.0, on.id, 2);
    info_of(&mut row).descriptor = StreamDescriptor::Torrent {
        info_hash: "0123456789abcdef0123456789abcdef01234567".to_string(),
        file_hint: None,
        file_idx: None,
        trackers: vec![],
    };
    refused.push(("a torrent", row));
    let mut row = version(title, 7_200.0, on.id, 2);
    info_of(&mut row).size = None;
    refused.push(("a service's file that lists no size", row));
    let mut row = version(title, 7_200.0, on.id, 2);
    row.probe_data
        .as_mut()
        .unwrap()
        .size = Some(LISTED as i64 * 2);
    refused.push(("measured from another file than it lists", row));
    let iptv = addon("iptv-m3u", false);
    let redirect = addon("stremio", true);
    let off = switched_off(&addon("stremio", false));
    let disabled = Addon {
        enabled: false,
        ..addon("stremio", false)
    };
    let all = rules_of(&[&on, &iptv, &redirect, &off, &disabled]);
    for (why, from) in [
        ("an IPTV addon", iptv.id),
        ("an addon that bypasses the proxy", redirect.id),
        ("an addon with previews off", off.id),
        ("a disabled addon", disabled.id),
        ("an addon not in the rules", Uuid::new_v4()),
    ] {
        refused.push((why, version(title, 7_200.0, from, 2)));
    }
    let mut row = version(title, 7_200.0, on.id, 2);
    info_of(&mut row).addon_id = None;
    refused.push(("no addon", row));
    for (why, row) in &refused {
        assert!(
            other_versions(&playing, std::slice::from_ref(row), None, &all, none_own)
                .is_empty(),
            "{why}"
        );
    }
    let owned = version(title, 7_200.0, on.id, 2);
    let owned_id = owned.id;
    assert!(
        other_versions(&playing, &[owned], None, &all, |id| id == owned_id).is_empty(),
        "a version with a folder of its own"
    );

    // A version played with no measured length has none.
    let unmeasured = db::Media {
        probe_data: None,
        ..version(title, 7_200.0, on.id, 0)
    };
    assert!(
        other_versions(&unmeasured, &[fits.clone()], None, &rules, none_own).is_empty()
    );

    // One episode from its own file and from a file holding two.
    let episode = Uuid::new_v4();
    let single = version(episode, 720.0, on.id, 0);
    let combined = version(episode, 1_407.0, on.id, 1);
    let both = [single.clone(), combined.clone()];
    assert!(other_versions(&single, &both, None, &rules, none_own).is_empty());
    assert!(other_versions(&combined, &both, None, &rules, none_own).is_empty());
}

#[test]
fn other_versions_cheapest_first_then_smallest_then_listed_first() {
    let title = Uuid::new_v4();
    let on = addon("stremio", false);
    let rules = rules_of(&[&on]);
    let playing = version(title, 7_200.0, on.id, 0);
    let mut usenet = version(title, 7_200.0, on.id, 1);
    info_of(&mut usenet).usenet_guid = Some("guid".to_string());
    let mut cached_big = version(title, 7_200.0, on.id, 2);
    info_of(&mut cached_big).size = Some(LISTED as i64 * 4);
    let cached_late = version(title, 7_200.0, on.id, 4);
    let cached_early = version(title, 7_200.0, on.id, 3);
    let mut webdav = version(title, 7_200.0, on.id, 5);
    info_of(&mut webdav).descriptor = StreamDescriptor::Opendal {
        addon_id: on.id,
        path: "films/invented-film.mkv".to_string(),
    };
    let mut local_unsized = version(title, 7_200.0, on.id, 6);
    info_of(&mut local_unsized).descriptor =
        StreamDescriptor::Local("/media/invented-film-a.mkv".into());
    info_of(&mut local_unsized).size = None;
    let mut local = version(title, 7_200.0, on.id, 7);
    info_of(&mut local).descriptor =
        StreamDescriptor::Local("/media/invented-film-b.mkv".into());
    let rows = [
        playing.clone(),
        usenet.clone(),
        cached_big.clone(),
        cached_late.clone(),
        cached_early.clone(),
        webdav.clone(),
        local_unsized.clone(),
        local.clone(),
    ];
    let order: Vec<Uuid> = other_versions(&playing, &rows, None, &rules, |_| false)
        .iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(
        order,
        vec![
            local.id,
            local_unsized.id,
            webdav.id,
            cached_early.id,
            cached_late.id,
            cached_big.id,
            usenet.id,
        ]
    );
}

#[test]
fn the_size_check_reads_only_the_file_listed() {
    let listed = StreamInfo {
        size: Some(LISTED as i64),
        ..Default::default()
    };
    assert!(is_listed_file(&listed, Some(LISTED)));
    assert!(!is_listed_file(&listed, Some(LISTED * 3)), "another file");
    assert!(
        !is_listed_file(&listed, None),
        "a source that gives no size"
    );
    assert!(!is_listed_file(&listed, Some(0)));
    let no_size = StreamInfo::default();
    assert!(
        is_listed_file(&no_size, Some(LISTED)),
        "nothing to compare with"
    );
}
