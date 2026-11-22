//! ffmpeg's arguments, and how a run's end is judged from what it wrote.

use super::*;

#[test]
fn pass_args_reads_key_frames_from_the_proxy() {
    let stream_id = Uuid::new_v4();
    let info = StreamInfo {
        descriptor: StreamDescriptor::http("https://source.invalid/secret/a.mkv"),
        ..Default::default()
    };
    let input = pass_input(&info, stream_id, 8096);
    assert_eq!(
        input,
        format!(
            "http://127.0.0.1:8096/stream/{stream_id}?token={}",
            crate::stream::stream_link_token(stream_id)
        )
    );
    assert!(!input.contains("source.invalid"));

    let out = Path::new("/data/trickplay/abc");
    let args = pass_args(&input, out, 0);
    let at = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("{flag} missing"))
    };
    assert!(
        !args
            .iter()
            .any(|a| a == "-ss"),
        "a run from the start does not seek"
    );
    assert_eq!(args[at("-skip_frame") + 1], "nokey");
    assert!(at("-skip_frame") < at("-i"));
    assert_eq!(args[at("-i") + 1], input);
    assert_eq!(args[at("-threads") + 1], "1");
    assert!(at("-reconnect") < at("-i"));
    let filter = &args[at("-vf") + 1];
    assert!(filter.contains("fps=1/10:start_time=0"));
    assert!(filter.contains("pad=320:180"));
    assert!(filter.contains("tile=5x5"));
    assert_eq!(args[at("-atomic_writing") + 1], "1");
    assert_eq!(args[at("-start_number") + 1], "0");
    assert_eq!(args[at("-progress") + 1], "pipe:1");
    assert!(
        args.iter()
            .any(|a| a.ends_with("%d.jpg"))
    );

    let local = StreamInfo {
        descriptor: StreamDescriptor::Local("/media/invented-film.mkv".into()),
        ..Default::default()
    };
    assert_eq!(
        pass_input(&local, stream_id, 8096),
        "/media/invented-film.mkv"
    );
}

#[test]
fn pass_args_part_way_seek_before_the_input_and_keep_the_grid() {
    let args = pass_args("/media/invented-film.mkv", Path::new("/data/t/abc"), 3);
    let at = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("{flag} missing"))
    };
    assert_eq!(args[at("-ss") + 1], "750");
    assert!(at("-skip_frame") < at("-ss"));
    assert!(at("-ss") < at("-i"));
    assert_eq!(args[at("-start_number") + 1], "3");
    assert!(args[at("-vf") + 1].starts_with("fps=1/10:start_time=0,"));
    assert!(
        !args
            .iter()
            .any(|a| a == "-frames:v")
    );
}

#[test]
fn pass_args_keep_a_percent_in_the_folder_plain() {
    let args = pass_args(URL, Path::new("/srv/100%/trickplay/abc"), 0);
    let output = args
        .iter()
        .find(|a| a.ends_with(".jpg"))
        .unwrap();
    let expected = Path::new("/srv/100%%/trickplay/abc")
        .join("%d.jpg")
        .to_string_lossy()
        .into_owned();
    assert_eq!(output, &expected);
}

#[test]
fn run_end_judges_each_way_a_run_ends() {
    let run = |start: u32| Run {
        start,
        stop_after: None,
    };
    let ended = |made: u64| RunResult {
        exit_ok: true,
        made_sheets: made,
        ..Default::default()
    };
    // 940 pictures: sheets 0 to 37.
    let goal = RunResult {
        reached_goal: true,
        made_sheets: 5,
        ..Default::default()
    };
    assert_eq!(run_end(&goal, run(3), 940), RunEnd::Goal);
    let stopped = RunResult {
        cancelled: true,
        made_sheets: 5,
        ..Default::default()
    };
    assert_eq!(run_end(&stopped, run(3), 940), RunEnd::Cancelled);
    assert_eq!(run_end(&ended(38), run(0), 940), RunEnd::End);
    assert_eq!(run_end(&ended(19), run(19), 940), RunEnd::End);
    // Up to the sheet before the last: a last sheet may never come.
    assert_eq!(run_end(&ended(18), run(19), 940), RunEnd::End);
    // The sheet before the last or the last itself from a source ffmpeg
    // gave up on: a source cut off there (a partly filled sheet flushed),
    // not the end of the file.
    for sign in GAVE_UP_ERRORS {
        let cut = |made: u64| RunResult {
            stderr: format!("[in#0 @ 0x7] {sign}\n"),
            ..ended(made)
        };
        assert_eq!(run_end(&cut(18), run(19), 940), RunEnd::Failed, "{sign}");
        assert_eq!(run_end(&cut(37), run(0), 940), RunEnd::Failed, "{sign}");
        assert_eq!(run_end(&cut(19), run(19), 940), RunEnd::Failed, "{sign}");
        assert_eq!(run_end(&cut(38), run(0), 940), RunEnd::Failed, "{sign}");
    }
    // The end-to-end cut: exit 0 with what ffmpeg wrote when it gave up.
    let dropped = |made: u64| RunResult {
        stderr: SOURCE_DROPPED.to_string(),
        ..ended(made)
    };
    assert_eq!(run_end(&dropped(37), run(0), 940), RunEnd::Failed);
    assert_eq!(run_end(&dropped(38), run(0), 940), RunEnd::Failed);
    let cut_off = |made: u64| RunResult {
        stderr: SOURCE_CUT.to_string(),
        ..ended(made)
    };
    assert_eq!(run_end(&cut_off(37), run(0), 940), RunEnd::Failed);
    assert_eq!(run_end(&cut_off(38), run(0), 940), RunEnd::Failed);
    // A drop ffmpeg's reconnect fixed leaves only this line (its "Will
    // reconnect" is a hidden warning): the end of the file all the same.
    let recovered = |made: u64| RunResult {
        stderr:
            "[http @ 0x6] Stream ends prematurely at 52428800, should be 70027876\n"
                .to_string(),
        ..ended(made)
    };
    assert_eq!(run_end(&recovered(18), run(19), 940), RunEnd::End);
    assert_eq!(run_end(&recovered(38), run(0), 940), RunEnd::End);
    // A damaged end of the file itself is no give-up either.
    let damaged = RunResult {
        stderr: "[in#0 @ 0x7] Error during demuxing: Invalid data found when processing input\n"
            .to_string(),
        ..ended(37)
    };
    assert_eq!(run_end(&damaged, run(0), 940), RunEnd::End);
    // A source that closed a request at the file's very end is no failure.
    let closed = RunResult {
        stderr: PAST_END_HTTP.to_string(),
        ..ended(18)
    };
    assert_eq!(run_end(&closed, run(19), 940), RunEnd::End);
    // Short of that by itself: a source cut off part way also exits 0.
    assert_eq!(run_end(&ended(17), run(19), 940), RunEnd::Failed);
    assert_eq!(run_end(&ended(10), run(0), 940), RunEnd::Failed);
    // From sheet 36 with no sheet the file is over before it: not its end.
    assert_eq!(run_end(&ended(0), run(36), 940), RunEnd::Nothing);
    // From the last sheet with no sheet: the file ended at a sheet's edge.
    assert_eq!(run_end(&ended(0), run(37), 940), RunEnd::End);
    // Part way, ended with an error.
    let error = RunResult {
        made_sheets: 2,
        ..Default::default()
    };
    assert_eq!(run_end(&error, run(10), 940), RunEnd::Failed);
    assert_eq!(run_end(&RunResult::default(), run(10), 940), RunEnd::Failed);
    // Part way, no sheet within the wait.
    let no_jump = RunResult {
        no_first_sheet: true,
        ..Default::default()
    };
    assert_eq!(run_end(&no_jump, run(10), 940), RunEnd::NoJump);
    // From the start, nothing at all.
    assert_eq!(run_end(&ended(0), run(0), 940), RunEnd::Failed);
}

#[test]
fn a_run_past_the_end_is_told_from_a_failing_source() {
    let run = |start: u32| Run {
        start,
        stop_after: None,
    };
    let exited = |code: i32, stderr: String| RunResult {
        exit_ok: code == 0,
        exit_code: Some(code),
        stderr,
        ..Default::default()
    };
    let past_end = exited(234, PAST_END.to_string());
    let past_end_http = exited(234, format!("{PAST_END_HTTP}{PAST_END}"));
    // 940 pictures: sheets 0 to 37. ffmpeg started past the end: 234
    // (AVERROR(EINVAL) on Unix), -22 on Windows, no sheet.
    assert_eq!(run_end(&past_end, run(36), 940), RunEnd::Nothing);
    assert_eq!(run_end(&past_end_http, run(36), 940), RunEnd::Nothing);
    assert_eq!(
        run_end(&exited(-22, PAST_END.to_string()), run(36), 940),
        RunEnd::Nothing
    );
    // From the last sheet: the file ended at a sheet's edge.
    assert_eq!(run_end(&past_end, run(37), 940), RunEnd::End);
    assert_eq!(run_end(&past_end_http, run(37), 940), RunEnd::End);
    // From the start, nothing at all is a failure whatever the status.
    assert_eq!(run_end(&past_end, run(0), 940), RunEnd::Failed);
    // An encoder that opened but muxed nothing: only the muxer's line.
    assert_eq!(
        run_end(
            &exited(
                234,
                "[out#0/image2 @ 0x5] Nothing was written into output file, because \
                 at least one of its streams received no packets.\n"
                    .to_string()
            ),
            run(36),
            940
        ),
        RunEnd::Nothing
    );

    // The same status after the source dropped, or with any other sign
    // of a failing source: a failure, never the end of the file.
    let dropped = exited(234, format!("{SOURCE_DROPPED}{PAST_END}"));
    assert_eq!(run_end(&dropped, run(36), 940), RunEnd::Failed);
    assert_eq!(run_end(&dropped, run(37), 940), RunEnd::Failed);
    for sign in SOURCE_ERRORS {
        let failed = exited(234, format!("[in#0 @ 0x7] {sign}\n{PAST_END}"));
        assert_eq!(run_end(&failed, run(36), 940), RunEnd::Failed, "{sign}");
        assert_eq!(run_end(&failed, run(37), 940), RunEnd::Failed, "{sign}");
    }
    // Exit 0 with no sheet and a source error: a failure too.
    let quiet = exited(
        0,
        "[in#0 @ 0x7] Error during demuxing: Input/output error\n".to_string(),
    );
    assert_eq!(run_end(&quiet, run(36), 940), RunEnd::Failed);
    // 234 without the encoder's line, or another status: a failure.
    assert_eq!(
        run_end(&exited(234, String::new()), run(36), 940),
        RunEnd::Failed
    );
    let refused = exited(
        8,
        "[http @ 0x6] HTTP error 404 Not Found\n\
         [in#0 @ 0x7] Error opening input: Server returned 404 Not Found\n"
            .to_string(),
    );
    assert_eq!(run_end(&refused, run(36), 940), RunEnd::Failed);
    assert_eq!(run_end(&refused, run(37), 940), RunEnd::Failed);
    // A run with sheets is judged by them, whatever the status.
    let some = RunResult {
        made_sheets: 1,
        ..past_end.clone()
    };
    assert_eq!(run_end(&some, run(36), 940), RunEnd::Failed);
}

#[test]
fn a_line_it_does_not_know_at_the_end_of_the_file_marks_no_end() {
    let run = |start: u32| Run {
        start,
        stop_after: None,
    };
    let ended = |made: u64, stderr: &str| RunResult {
        exit_ok: true,
        exit_code: Some(0),
        made_sheets: made,
        stderr: stderr.to_string(),
        ..Default::default()
    };
    // 940 pictures: sheets 0 to 37.
    assert_eq!(
        run_end(&ended(38, UNKNOWN_LINE), run(0), 940),
        RunEnd::Unclean
    );
    assert_eq!(
        run_end(&ended(18, UNKNOWN_LINE), run(19), 940),
        RunEnd::Unclean
    );
    assert_eq!(
        run_end(&ended(0, UNKNOWN_LINE), run(37), 940),
        RunEnd::Unclean
    );
    // Lines the tables know still end it, as does no line at all.
    assert_eq!(
        run_end(&ended(18, PAST_END_HTTP), run(19), 940),
        RunEnd::End
    );
    assert_eq!(run_end(&ended(38, ""), run(0), 940), RunEnd::End);
    // With the status of a run that found no picture, its own lines are
    // expected.
    let past_end = RunResult {
        exit_code: Some(234),
        stderr: PAST_END.to_string(),
        ..Default::default()
    };
    assert_eq!(run_end(&past_end, run(37), 940), RunEnd::End);
    // Short of the end it is a failure, as before.
    assert_eq!(
        run_end(&ended(10, UNKNOWN_LINE), run(0), 940),
        RunEnd::Failed
    );
}

#[tokio::test]
async fn an_early_source_error_survives_a_flood_and_the_kept_text_is_bounded() {
    let mut text =
        String::from("[http @ 0x6] Server returned 503 Service Unavailable\n");
    for n in 0..20_000 {
        text.push_str(&format!(
            "[h264 @ 0x9] error while decoding MB {n} 0, bytestream -5\n"
        ));
    }
    text.push_str("[in#0 @ 0x7] the last line\n");
    let kept = read_stderr(text.as_bytes()).await;
    assert!(
        kept.len() <= STDERR_TAIL_BYTES + 4 * STDERR_LINE_BYTES,
        "{}",
        kept.len()
    );
    assert!(
        kept.contains("the last line"),
        "the tail is kept for the log"
    );
    let result = RunResult {
        exit_ok: true,
        stderr: kept,
        ..Default::default()
    };
    assert!(source_failed(&result), "the early line is kept");
    assert!(unknown_line(&result), "and one line no table knows");

    // A line that never ends keeps only its start.
    let endless = "x".repeat(1_000_000);
    let kept = read_stderr(endless.as_bytes()).await;
    assert!(kept.len() <= STDERR_LINE_BYTES + 1, "{}", kept.len());

    // Lines end at a carriage return too, and the repeats of a known
    // line fill only the tail.
    let repeated =
        "[http @ 0x6] Stream ends prematurely at 1, should be 2\r".repeat(5_000);
    let kept = read_stderr(repeated.as_bytes()).await;
    assert!(
        kept.len() <= STDERR_TAIL_BYTES + STDERR_LINE_BYTES + 1,
        "{}",
        kept.len()
    );
    assert!(
        kept.lines()
            .all(|line| line.ends_with("should be 2"))
    );
}

#[tokio::test]
async fn a_stop_that_comes_with_the_last_progress_block_counts_its_sheet() {
    let tmp = tempfile::tempdir().unwrap();
    let job = progress_job(tmp.path());
    // The stop is already there when ffmpeg's last block is read: the
    // lines written before it are read first.
    let (stop, mut cancel) = oneshot::channel();
    stop.send(())
        .unwrap();
    let block: &[u8] = b"frame=3\nprogress=continue\nframe=4\nprogress=end\n";
    let seen = read_progress(block, &job, &mut cancel).await;
    assert_eq!(
        seen,
        RunProgress {
            made_sheets: 4,
            ended: true,
            ..Default::default()
        }
    );
    assert!(
        !seen.ended_before_stop(),
        "it ended by itself, before the stop"
    );

    // The last block read and the pipe still open: the stop is seen after
    // it, and the run is known to have ended its output.
    let (mut writer, reader) = tokio::io::duplex(1024);
    tokio::io::AsyncWriteExt::write_all(&mut writer, b"frame=4\nprogress=end\n")
        .await
        .unwrap();
    let (stop, mut cancel) = oneshot::channel();
    stop.send(())
        .unwrap();
    let seen =
        read_progress(tokio::io::BufReader::new(reader), &job, &mut cancel).await;
    assert_eq!(
        seen,
        RunProgress {
            made_sheets: 4,
            cancelled: true,
            ended: true,
            ..Default::default()
        }
    );
    drop(writer);
    // So the run's last sheet, which may be partly filled, goes (see
    // `after_run`'s `Cancelled` arm and its test).
    assert!(seen.ended_before_stop());
    // A stop before the last block keeps every sheet.
    assert!(
        !RunProgress {
            made_sheets: 4,
            cancelled: true,
            ..Default::default()
        }
        .ended_before_stop()
    );
}

/// The tables against the ffmpeg this build runs: a whole read ends
/// cleanly, with no line the tables do not know, and a run started past
/// the end of the file ends empty. Upstream's workflows install no
/// ffmpeg, so it is skipped there.
#[tokio::test]
async fn the_real_ffmpeg_ends_a_clip_as_the_tables_expect() {
    let tmp = tempfile::tempdir().unwrap();
    let clip = tmp
        .path()
        .join("clip.mkv");
    if !make_clip(&clip) {
        eprintln!("ffmpeg unavailable, skipping");
        return;
    }
    let input = clip
        .to_string_lossy()
        .into_owned();
    let job = |start_sheet: u32, out_dir: &Path| PassJob {
        input: input.clone(),
        out_dir: out_dir.to_path_buf(),
        pid: Arc::new(AtomicU32::new(0)),
        min_free_bytes: 0,
        start_sheet,
        stop_after: None,
        first_sheet_within: None,
        made: Arc::new(AtomicU32::new(0)),
    };
    // 300 s: 30 pictures, sheets 0 and 1.
    let pictures = 30;

    // Read whole. The stop sender is held: a dropped one stops the run.
    let whole = tmp
        .path()
        .join("whole");
    std::fs::create_dir_all(&whole).unwrap();
    let (_stop, cancel) = oneshot::channel();
    let result = run_ffmpeg(job(0, &whole), cancel).await;
    assert!(result.exit_ok, "{}", result.stderr);
    assert_eq!(result.made_sheets, 2, "{}", result.stderr);
    assert!(
        !unknown_line(&result),
        "a clean end writes no line the tables do not know: {}",
        result.stderr
    );
    let from_start = Run {
        start: 0,
        stop_after: None,
    };
    assert_eq!(run_end(&result, from_start, pictures), RunEnd::End);
    assert!(sheet_whole(&sheet_file(&whole, 0)));
    assert!(sheet_whole(&sheet_file(&whole, 1)));

    // Started at sheet 2, 500 s in: past the end of the clip.
    let past = tmp
        .path()
        .join("past");
    std::fs::create_dir_all(&past).unwrap();
    let (_stop, cancel) = oneshot::channel();
    let result = run_ffmpeg(job(2, &past), cancel).await;
    assert!(
        ended_empty(&result),
        "exit {:?}: {}",
        result.exit_code,
        result.stderr
    );
    let past_end = Run {
        start: 2,
        stop_after: None,
    };
    // With the clip's own count, the end of the file.
    assert_eq!(run_end(&result, past_end, pictures), RunEnd::End);
    // With the count of a longer stored length (sheets 0 to 3), the file
    // ends before that place.
    assert_eq!(run_end(&result, past_end, 100), RunEnd::Nothing);
}
