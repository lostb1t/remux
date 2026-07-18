#!/usr/bin/env python3
"""
Playback verification harness for Remux (Jellyfin-compatible media server).

Proves — programmatically and repeatably — that the server actually serves a
*decodable audio stream*, not merely a 200, by walking the real client flow:

  1. POST /Items/{id}/PlaybackInfo            -> choose a MediaSource, PlaySessionId
  2. sanity-check the source shape            (Protocol/IsRemote/Container/streams)
  3. fetch the stream three ways with a Range:
       - /Audio/{id}/universal  (follow redirects; MUST resolve to audio, not the
         video HLS pipeline — the historical bug)
       - /Audio/{id}/stream?static=true
       - /Items/{id}/File
     asserting HTTP 200/206 + an audio Content-Type + non-empty body.
  4. (--deep) download the whole stream and decode it with ffmpeg to /dev/null,
     proving the bytes are a valid, fully-decodable audio file.

Also verifies *loading*: an album or playlist resolves every child to a playable
source. Prints a PASS/FAIL table and exits non-zero on any hard failure, so it
works as a CI/regression gate.

Usage:
  verify.py --item <id> [--deep]         # one track, deep decode
  verify.py --album <id>                 # every track on an album
  verify.py --playlist <id> [--limit N]  # every member of a playlist
  verify.py --sample N                   # N random library tracks
"""
import argparse, json, subprocess, sys, tempfile, os, urllib.request, urllib.parse, collections, random

BASE = os.environ.get("REMUX_BASE", "http://127.0.0.1:3008")
TOK  = os.environ.get("REMUX_TOKEN", "c3bb11fa62ec42f2a1eafd49e199251f")
UID  = os.environ.get("REMUX_UID", "ab993c51-b008-4629-98aa-5066ba9e46b4")
DB   = os.environ.get("REMUX_DB", "/opt/remux/data/db.sqlite")
RANGE = "bytes=0-262143"          # 256 KiB probe window
HDRS = {"X-Emby-Token": TOK}


def _req(method, path, body=None, rng=None, redirect=True):
    url = path if path.startswith("http") else BASE + path
    h = dict(HDRS)
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        h["Content-Type"] = "application/json"
    if rng:
        h["Range"] = rng
    req = urllib.request.Request(url, data=data, headers=h, method=method)

    class _NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *a, **k):
            return None
    opener = urllib.request.build_opener() if redirect else urllib.request.build_opener(_NoRedirect)
    try:
        r = opener.open(req, timeout=30)
        return r.status, dict(r.headers), r.read()
    except urllib.error.HTTPError as e:
        return e.code, dict(e.headers), e.read()
    except Exception as e:
        return 0, {"_err": str(e)}, b""


def get_json(path):
    s, _, b = _req("GET", path)
    try:
        return json.loads(b)
    except Exception:
        return None


def playback_info(item_id):
    s, _, b = _req("POST", f"/Items/{item_id}/PlaybackInfo?UserId={UID}", body={"UserId": UID})
    try:
        return json.loads(b)
    except Exception:
        return None


def fetch_stream(url, redirect=True):
    """Ranged GET. Returns (status, content_type, nbytes, accept_ranges, final_url_hint)."""
    s, h, b = _req("GET", url, rng=RANGE, redirect=redirect)
    ct = (h.get("Content-Type") or h.get("content-type") or "").lower()
    ar = (h.get("Accept-Ranges") or h.get("accept-ranges") or "")
    loc = h.get("Location") or h.get("location") or ""
    return s, ct, len(b), ar, loc


def decode_test(url):
    """Download the full stream and decode it with ffmpeg to null. Returns (ok, detail)."""
    s, h, b = _req("GET", url)
    if s not in (200, 206) or not b:
        return False, f"download HTTP {s} bytes={len(b)}"
    with tempfile.NamedTemporaryFile(suffix=".media", delete=False) as f:
        f.write(b)
        path = f.name
    try:
        pr = subprocess.run(
            ["ffprobe", "-v", "error", "-select_streams", "a:0",
             "-show_entries", "stream=codec_name,sample_rate,channels:format=duration",
             "-of", "json", path],
            capture_output=True, text=True, timeout=60)
        meta = json.loads(pr.stdout or "{}")
        astreams = meta.get("streams", [])
        if not astreams:
            return False, f"ffprobe: no audio stream ({pr.stderr.strip()[:80]})"
        dec = subprocess.run(["ffmpeg", "-v", "error", "-i", path, "-f", "null", "-"],
                             capture_output=True, text=True, timeout=180)
        if dec.returncode != 0 or dec.stderr.strip():
            return False, f"decode error: {dec.stderr.strip()[:120]}"
        st = astreams[0]
        return True, f"{st.get('codec_name')} {st.get('sample_rate')}Hz {st.get('channels')}ch {float(meta.get('format',{}).get('duration',0)):.0f}s"
    finally:
        os.unlink(path)


def verify_track(item_id, name="", deep=False):
    """Returns (ok, [issues], detail)."""
    issues = []
    pi = playback_info(item_id)
    if not pi or not pi.get("MediaSources"):
        return False, ["PlaybackInfo returned no MediaSources"], ""
    ms = pi["MediaSources"][0]
    proto, remote, cont = ms.get("Protocol"), ms.get("IsRemote"), ms.get("Container")
    astreams = [s for s in (ms.get("MediaStreams") or []) if s.get("Type") == "Audio"]
    if not astreams:
        issues.append("no Audio MediaStream in source")
    if not ms.get("SupportsDirectPlay") and not ms.get("SupportsTranscoding"):
        issues.append("source supports neither direct play nor transcoding")

    # universal MUST resolve to audio, never the video HLS pipeline.
    u = (f"/Audio/{item_id}/universal?UserId={UID}&api_key={TOK}"
         f"&MaxStreamingBitrate=1400000000&Container=flac,mp3,aac,m4a,ogg,opus,wav"
         f"&AudioCodec=flac,aac,mp3&PlaySessionId=verify&DeviceId=verify")
    # inspect the redirect target first (diagnostic), then follow it.
    s0, ct0, n0, ar0, loc0 = fetch_stream(u, redirect=False)
    if 300 <= s0 < 400 and "/videos/" in loc0:
        issues.append(f"universal redirects into VIDEO pipeline: {loc0[:70]}")
    su, ctu, nu, aru, _ = fetch_stream(u, redirect=True)
    if su not in (200, 206):
        issues.append(f"universal HTTP {su}")
    elif not ctu.startswith("audio/"):
        issues.append(f"universal Content-Type not audio/*: {ctu or '(none)'}")

    # /stream?static=true
    st_s, st_ct, st_n, st_ar, _ = fetch_stream(f"/Audio/{item_id}/stream?static=true&api_key={TOK}")
    if st_s not in (200, 206) or not st_ct.startswith("audio/") or st_n == 0:
        issues.append(f"stream HTTP {st_s} ct={st_ct or '-'} bytes={st_n}")

    # /Items/{id}/File
    f_s, f_ct, f_n, _, _ = fetch_stream(f"/Items/{item_id}/File?api_key={TOK}")
    if f_s not in (200, 206) or f_n == 0:
        issues.append(f"File HTTP {f_s} bytes={f_n}")

    detail = f"{proto}/remote={remote}/{cont}"
    if deep:
        ok, d = decode_test(f"/Audio/{item_id}/stream?static=true&api_key={TOK}")
        detail += f" | decode: {d}"
        if not ok:
            issues.append(f"decode: {d}")
    return (len(issues) == 0), issues, detail


def children(parent_id, limit=None):
    q = f"/Items?ParentId={parent_id}&IncludeItemTypes=Audio&Recursive=true&Fields=MediaSources&userId={UID}"
    if limit:
        q += f"&Limit={limit}"
    d = get_json(q) or {}
    return d.get("Items", [])


def playlist_members(pid, limit=None):
    q = f"/Playlists/{pid}/Items?UserId={UID}&Fields=MediaSources"
    if limit:
        q += f"&Limit={limit}"
    d = get_json(q) or {}
    return d.get("Items", [])


def sample_tracks(n):
    out = subprocess.run(
        ["sudo", "sqlite3", DB,
         "SELECT hex(id), title FROM media WHERE kind='track' ORDER BY RANDOM() LIMIT %d;" % n],
        capture_output=True, text=True).stdout.strip().splitlines()
    res = []
    for line in out:
        parts = line.split("|", 1)
        if len(parts) == 2:
            h = parts[0].lower()
            res.append((f"{h[0:8]}-{h[8:12]}-{h[12:16]}-{h[16:20]}-{h[20:32]}", parts[1]))
    return res


def run(items, deep):
    print(f"=== PLAYBACK VERIFY: {len(items)} track(s) against {BASE} ===")
    npass = 0
    buckets = collections.Counter()
    fails = []
    for i, (tid, name) in enumerate(items, 1):
        ok, issues, detail = verify_track(tid, name, deep=deep)
        tag = "PASS" if ok else "FAIL"
        if ok:
            npass += 1
        else:
            for iss in issues:
                buckets[iss.split(":")[0]] += 1
            fails.append((name or tid, issues))
        # compact per-track line
        mark = "✓" if ok else "✗"
        print(f"  {mark} [{tag}] {(name or tid)[:48]:48} {detail}")
        if not ok:
            for iss in issues:
                print(f"        - {iss}")
    print(f"\n=== {npass}/{len(items)} passed ===")
    if buckets:
        print("failure buckets:")
        for b, c in buckets.most_common():
            print(f"  {c:4}  {b}")
    return npass == len(items)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--item"); ap.add_argument("--album"); ap.add_argument("--playlist")
    ap.add_argument("--sample", type=int); ap.add_argument("--limit", type=int)
    ap.add_argument("--idfile", help="file of '<id>|<name>' lines")
    ap.add_argument("--deep", action="store_true")
    a = ap.parse_args()

    if a.idfile:
        items = []
        for line in open(a.idfile):
            line = line.strip()
            if not line:
                continue
            tid, _, nm = line.partition("|")
            items.append((tid, nm))
    elif a.item:
        d = get_json(f"/Items?Ids={a.item}&userId={UID}") or {}
        nm = (d.get("Items") or [{}])[0].get("Name", "")
        items = [(a.item, nm)]
        a.deep = True  # single item always deep
    elif a.album:
        items = [(c["Id"], c.get("Name", "")) for c in children(a.album, a.limit)]
    elif a.playlist:
        items = [(c["Id"], c.get("Name", "")) for c in playlist_members(a.playlist, a.limit)]
    elif a.sample:
        items = sample_tracks(a.sample)
    else:
        ap.error("one of --item/--album/--playlist/--sample required")

    if not items:
        print("no items to test"); sys.exit(2)
    ok = run(items, a.deep)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
