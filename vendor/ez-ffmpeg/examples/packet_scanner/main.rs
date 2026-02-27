use ez_ffmpeg::packet_scanner::PacketScanner;
use ez_ffmpeg::stream_info::StreamInfo;

fn main() {
    // =========================================================
    // Example 1: Sequential scan — iterate over all packets
    // =========================================================
    println!("=== Example 1: Sequential Packet Scan ===");
    let mut scanner = PacketScanner::open("test.mp4").unwrap();

    let mut total_packets = 0;
    let mut keyframe_count = 0;
    let mut total_size: usize = 0;

    for packet in scanner.packets() {
        let info = packet.unwrap();
        total_packets += 1;
        total_size += info.size();
        if info.is_keyframe() {
            keyframe_count += 1;
        }
    }

    println!("  Total packets : {}", total_packets);
    println!("  Keyframes     : {}", keyframe_count);
    println!("  Total size    : {} bytes", total_size);

    // =========================================================
    // Example 2: Keyframe detection — find all keyframes and
    //            print their timestamps
    // =========================================================
    println!();
    println!("=== Example 2: Keyframe Detection ===");
    let mut scanner = PacketScanner::open("test.mp4").unwrap();

    let mut keyframe_index = 0;
    for packet in scanner.packets() {
        let info = packet.unwrap();
        // Use is_keyframe() to filter only keyframe packets
        if info.is_keyframe() {
            println!(
                "  Keyframe #{:<3} stream={} pts={:>8?} dts={:>8?} size={:>6} pos={:>6}",
                keyframe_index,
                info.stream_index(),
                info.pts(),
                info.dts(),
                info.size(),
                info.pos(),
            );
            keyframe_index += 1;
        }
    }

    // =========================================================
    // Example 3: Per-stream packet statistics
    // =========================================================
    println!();
    println!("=== Example 3: Per-Stream Statistics ===");
    let mut scanner = PacketScanner::open("test.mp4").unwrap();

    // Collect per-stream stats using a simple Vec
    let mut stream_packets: Vec<usize> = Vec::new();
    let mut stream_keyframes: Vec<usize> = Vec::new();
    let mut stream_bytes: Vec<usize> = Vec::new();

    for packet in scanner.packets() {
        let info = packet.unwrap();
        let idx = info.stream_index();

        // Grow vectors if needed
        while stream_packets.len() <= idx {
            stream_packets.push(0);
            stream_keyframes.push(0);
            stream_bytes.push(0);
        }

        stream_packets[idx] += 1;
        stream_bytes[idx] += info.size();
        if info.is_keyframe() {
            stream_keyframes[idx] += 1;
        }
    }

    for i in 0..stream_packets.len() {
        println!(
            "  Stream {}: {} packets, {} keyframes, {} bytes",
            i, stream_packets[i], stream_keyframes[i], stream_bytes[i],
        );
    }

    // =========================================================
    // Example 4: Seek to a specific position and read packets
    // =========================================================
    println!();
    println!("=== Example 4: Seek to 1 second ===");
    let mut scanner = PacketScanner::open("test.mp4").unwrap();

    // seek() takes microseconds — 1_000_000 = 1 second.
    // It seeks to the nearest keyframe before the given timestamp.
    scanner.seek(1_000_000).unwrap();

    // Read a few packets after seeking
    for i in 0..5 {
        match scanner.next_packet().unwrap() {
            Some(info) => {
                println!(
                    "  Packet #{}: stream={} pts={:?} size={} keyframe={}",
                    i,
                    info.stream_index(),
                    info.pts(),
                    info.size(),
                    info.is_keyframe(),
                );
            }
            None => {
                println!("  (EOF reached)");
                break;
            }
        }
    }

    // =========================================================
    // Example 5: Multiple seeks — jump reading pattern
    // =========================================================
    println!();
    println!("=== Example 5: Multiple Seeks (jump reading) ===");
    let mut scanner = PacketScanner::open("test.mp4").unwrap();

    // Jump to several positions and read the first packet at each
    let seek_points_us: &[i64] = &[0, 500_000, 1_000_000, 2_000_000, 3_000_000];

    for &ts in seek_points_us {
        scanner.seek(ts).unwrap();
        match scanner.next_packet().unwrap() {
            Some(info) => {
                println!(
                    "  Seek to {}us -> stream={} pts={:?} keyframe={} size={}",
                    ts,
                    info.stream_index(),
                    info.pts(),
                    info.is_keyframe(),
                    info.size(),
                );
            }
            None => {
                println!("  Seek to {}us -> (EOF)", ts);
            }
        }
    }

    // =========================================================
    // Example 6: Detect corrupt packets
    // =========================================================
    println!();
    println!("=== Example 6: Corrupt Packet Detection ===");
    let mut scanner = PacketScanner::open("test.mp4").unwrap();

    let mut corrupt_count = 0;
    for packet in scanner.packets() {
        let info = packet.unwrap();
        if info.is_corrupt() {
            corrupt_count += 1;
            println!(
                "  Corrupt packet: stream={} pts={:?} size={}",
                info.stream_index(),
                info.pts(),
                info.size(),
            );
        }
    }
    if corrupt_count == 0 {
        println!("  No corrupt packets found.");
    } else {
        println!("  Total corrupt packets: {}", corrupt_count);
    }

    // =========================================================
    // Example 7: Find first keyframe per stream
    // =========================================================
    println!();
    println!("=== Example 7: First Keyframe Per Stream ===");
    let mut scanner = PacketScanner::open("test.mp4").unwrap();

    let mut found: Vec<bool> = Vec::new();

    for packet in scanner.packets() {
        let info = packet.unwrap();
        let idx = info.stream_index();

        while found.len() <= idx {
            found.push(false);
        }

        if info.is_keyframe() && !found[idx] {
            found[idx] = true;
            println!(
                "  Stream {} first keyframe: pts={:?} dts={:?} size={} pos={}",
                idx,
                info.pts(),
                info.dts(),
                info.size(),
                info.pos(),
            );
        }
    }

    // =========================================================
    // Example 8: Stream info overview — list all streams
    // =========================================================
    println!();
    println!("=== Example 8: Stream Info Overview ===");
    let scanner = PacketScanner::open("test.mp4").unwrap();

    // streams() returns all stream info cached at open time
    for stream in scanner.streams() {
        println!(
            "  Stream #{}: type={}, is_video={}, is_audio={}",
            stream.index(),
            stream.stream_type(),
            stream.is_video(),
            stream.is_audio(),
        );
        // Print type-specific details via Debug
        match stream {
            StreamInfo::Video { codec_name, width, height, fps, .. } => {
                println!("    codec={}, {}x{}, {:.2} fps", codec_name, width, height, fps);
            }
            StreamInfo::Audio { codec_name, sample_rate, nb_channels, .. } => {
                println!("    codec={}, {} Hz, {} ch", codec_name, sample_rate, nb_channels);
            }
            _ => {}
        }
    }

    // =========================================================
    // Example 9: Quick video/audio stream access
    // =========================================================
    println!();
    println!("=== Example 9: Video & Audio Stream Access ===");
    let scanner = PacketScanner::open("test.mp4").unwrap();

    // video_stream() and audio_stream() return the first match
    if let Some(video) = scanner.video_stream() {
        println!("  Video stream index: {}", video.index());
        if let StreamInfo::Video { codec_name, width, height, fps, bit_rate, .. } = video {
            println!(
                "    {} {}x{} {:.2}fps bitrate={}",
                codec_name, width, height, fps, bit_rate,
            );
        }
    } else {
        println!("  No video stream found.");
    }

    if let Some(audio) = scanner.audio_stream() {
        println!("  Audio stream index: {}", audio.index());
        if let StreamInfo::Audio { codec_name, sample_rate, nb_channels, bit_rate, .. } = audio {
            println!(
                "    {} {}Hz {}ch bitrate={}",
                codec_name, sample_rate, nb_channels, bit_rate,
            );
        }
    } else {
        println!("  No audio stream found.");
    }

    // =========================================================
    // Example 10: Stream-aware packet processing
    // =========================================================
    println!();
    println!("=== Example 10: Stream-Aware Packet Processing ===");
    let mut scanner = PacketScanner::open("test.mp4").unwrap();

    // PacketInfo carries is_video()/is_audio() — no pre-build lookup needed
    let mut video_packets = 0u64;
    let mut audio_packets = 0u64;
    let mut video_bytes = 0u64;
    let mut audio_bytes = 0u64;

    for packet in scanner.packets() {
        let pkt = packet.unwrap();
        if pkt.is_video() {
            video_packets += 1;
            video_bytes += pkt.size() as u64;
        } else if pkt.is_audio() {
            audio_packets += 1;
            audio_bytes += pkt.size() as u64;
        }
    }

    println!("  Video: {} packets, {} bytes", video_packets, video_bytes);
    println!("  Audio: {} packets, {} bytes", audio_packets, audio_bytes);

    // =========================================================
    // Example 11: stream_for_packet — correlate after reading
    // =========================================================
    println!();
    println!("=== Example 11: stream_for_packet Usage ===");
    let mut scanner = PacketScanner::open("test.mp4").unwrap();

    // Read a packet, then look up its full stream info
    if let Some(pkt) = scanner.next_packet().unwrap() {
        println!(
            "  First packet: stream #{} video={} audio={} pts={:?} size={}",
            pkt.stream_index(),
            pkt.is_video(),
            pkt.is_audio(),
            pkt.pts(),
            pkt.size(),
        );
        // stream_for_packet() is still useful when you need full stream details
        if let Some(stream) = scanner.stream_for_packet(&pkt) {
            println!("  Stream detail: {:?}", stream);
        }
    }
}
