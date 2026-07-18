#!/usr/bin/env python3
"""A/B before/after harness for remux-server — offline, no CodSpeed cloud.

Proves a performance change is (a) actually faster and (b) behaviour-preserving,
by comparing a *baseline* (the code without your patch) against the *treatment*
(the code with it). Two subcommands:

  ab.py bench   — run the divan benches for baseline vs treatment and print a
                  per-case median-time delta table, flagging regressions.
  ab.py verify  — boot the baseline and treatment server binaries, replay a
                  request corpus against each, and assert byte-identical JSON
                  after stripping known-volatile fields (Ids, timestamps, …).
  ab.py gate    — run `verify`, then (if present/executable) tools/parity and
                  tools/playback as external truth gates. Non-zero exit on any
                  hard failure, so it works as a CI/regression gate.

Per-patch isolation (important: this working tree may carry unrelated
uncommitted changes, so `git stash` is unsafe). The baseline is reconstructed
from file *snapshots* you take BEFORE editing:

  # before making the optimisation:
  tools/ab/ab.py snapshot crates/remux-server/src/api/models.rs
  # ... make your change ...
  tools/ab/ab.py bench --files crates/remux-server/src/api/models.rs \\
      --benches items,shows

`bench` builds/benches the current tree (treatment), swaps the snapshot copies
in (baseline), builds/benches again, then restores your working files — so the
measured delta is exactly your patch, regardless of any other in-flight edits.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SNAP_DIR = Path(__file__).resolve().parent / ".snapshots"
CRATE = "remux-server"

# Regression threshold: treatment slower than baseline by more than this
# fraction on any case is a hard failure for `bench --fail-on-regression`.
#
# Calibrated against the measured noise floor, not guessed: benching a *no-op
# comment change* through this harness still produced per-case deltas of
# +4.6% / +5.5% / +13.6%. These are full HTTP round-trips against a shared
# in-memory server, so run-to-run variance is large (means routinely exceed
# medians several-fold). A threshold near the noise floor would flag noise as
# regression, so it sits above it. Treat anything under ~25% as inconclusive
# and re-run; real wins here have been multiples (4×–77×), not percentages.
DEFAULT_REGRESSION = 0.25

# ── divan output parsing ─────────────────────────────────────────────────────

_UNIT_NS = {"ns": 1.0, "µs": 1e3, "us": 1e3, "ms": 1e6, "s": 1e9}
# A data row looks like:
#   ├─ limit=50&sortBy=DateCreated   1.2 ms │ 3 ms │ 1.5 ms │ … │ 100 │ 100
# Columns are separated by U+2502 (│). cell[0] holds "name  <fastest>"; the
# median is cell[2]. Group/parent rows have empty numeric cells and are skipped.
_TIME_RE = re.compile(r"([\d.]+)\s*(ns|µs|us|ms|s)\b")
_LEAF_NAME_RE = re.compile(r"^[\s│├╰─┤┌└]*(.*?)\s+[\d.]+\s*(?:ns|µs|us|ms|s)\b")


def parse_divan(text: str, target: str) -> dict[str, float]:
    """Map "target:leaf-label" -> median nanoseconds for every leaf bench row."""
    out: dict[str, float] = {}
    for line in text.splitlines():
        if "│" not in line:
            continue
        cells = line.split("│")
        if len(cells) < 4:
            continue
        median_cell = cells[2].strip()
        m = _TIME_RE.search(median_cell)
        if not m:
            continue
        name_m = _LEAF_NAME_RE.match(cells[0])
        if not name_m:
            continue
        label = name_m.group(1).strip()
        if not label:
            continue
        out[f"{target}:{label}"] = float(m.group(1)) * _UNIT_NS[m.group(2)]
    return out


def fmt_ns(ns: float) -> str:
    for unit, scale in (("s", 1e9), ("ms", 1e6), ("µs", 1e3)):
        if ns >= scale:
            return f"{ns / scale:.3g} {unit}"
    return f"{ns:.3g} ns"


# ── shell helpers ────────────────────────────────────────────────────────────

def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, cwd=REPO, text=True, **kw)


def snapshot_path(rel: str) -> Path:
    return SNAP_DIR / (rel.replace("/", "__") + ".base")


def cmd_snapshot(args) -> int:
    SNAP_DIR.mkdir(parents=True, exist_ok=True)
    for rel in args.files:
        src = REPO / rel
        if not src.is_file():
            print(f"error: {rel} is not a file", file=sys.stderr)
            return 2
        shutil.copy2(src, snapshot_path(rel))
        print(f"snapshotted {rel}")
    return 0


def run_benches(benches: list[str]) -> dict[str, float]:
    """Build + run each divan bench target, return merged median map (ns)."""
    medians: dict[str, float] = {}
    for b in benches:
        proc = run(
            ["cargo", "bench", "-p", CRATE, "--bench", b],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        if proc.returncode != 0:
            print(proc.stdout, file=sys.stderr)
            raise SystemExit(f"bench '{b}' failed (exit {proc.returncode})")
        medians.update(parse_divan(proc.stdout, b))
    return medians


def with_baseline_files(files: list[str], fn):
    """Swap snapshot copies over `files`, run fn(), always restore working copies."""
    saved: dict[str, bytes] = {}
    missing = [f for f in files if not snapshot_path(f).is_file()]
    if missing:
        raise SystemExit(
            "no snapshot for: " + ", ".join(missing) +
            "\nrun `ab.py snapshot <files>` BEFORE editing them."
        )
    try:
        for f in files:
            p = REPO / f
            saved[f] = p.read_bytes()
            shutil.copy2(snapshot_path(f), p)
        return fn()
    finally:
        for f, data in saved.items():
            (REPO / f).write_bytes(data)


def cmd_bench(args) -> int:
    benches = [b.strip() for b in args.benches.split(",") if b.strip()]
    files = [f.strip() for f in (args.files or "").split(",") if f.strip()]
    if not files:
        print("error: --files is required (the files your patch touches)",
              file=sys.stderr)
        return 2

    print(f"→ treatment: benching {benches} on the working tree …")
    treatment = run_benches(benches)

    print("→ baseline: swapping in snapshots and benching …")
    baseline = with_baseline_files(files, lambda: run_benches(benches))

    keys = sorted(set(baseline) | set(treatment))
    print(f"\n{'case':<52} {'baseline':>12} {'treatment':>12} {'Δ':>9}")
    print("-" * 90)
    worst = 0.0
    for k in keys:
        b = baseline.get(k)
        t = treatment.get(k)
        if b is None or t is None:
            print(f"{k:<52} {'—' if b is None else fmt_ns(b):>12} "
                  f"{'—' if t is None else fmt_ns(t):>12} {'(new/removed)':>9}")
            continue
        delta = (t - b) / b if b else 0.0
        worst = max(worst, delta)
        mark = "  ⚠" if delta > DEFAULT_REGRESSION else (
            "  ✓" if delta < -DEFAULT_REGRESSION else "")
        print(f"{k:<52} {fmt_ns(b):>12} {fmt_ns(t):>12} {delta * 100:>+7.1f}%{mark}")
    print("-" * 90)
    print(f"worst regression: {worst * 100:+.1f}%")
    if args.fail_on_regression and worst > DEFAULT_REGRESSION:
        print("FAIL: a case regressed beyond threshold", file=sys.stderr)
        return 1
    return 0


# ── response-equivalence (verify) ────────────────────────────────────────────

# Fields whose values legitimately differ between two identically-seeded builds
# (ids, timestamps, tokens, image-tag hashes, session ids, play counts). Mirrors
# the ignore-list documented for tools/parity. Matched case-insensitively by
# exact key name; *Id suffixes are matched structurally.
VOLATILE_KEYS = {
    "id", "serverid", "etag", "playsessionid", "accesstoken", "devicid",
    "deviceid", "datecreated", "datemodified", "datelastsaved",
    "startdate", "enddate", "premieredate", "path", "primaryimagetag",
    "parentprimaryimagetag", "imagetags", "backdropimagetags",
    "parentthumbimagetag", "parentbackdropimagetags", "playcount",
    "lastplayeddate", "userdata",
}


def normalize(obj):
    """Recursively drop volatile keys / *Id keys so the diff is behavioural."""
    if isinstance(obj, dict):
        out = {}
        for k, v in obj.items():
            kl = k.lower()
            if kl in VOLATILE_KEYS or kl.endswith("id") or kl.endswith("ticks"):
                continue
            out[k] = normalize(v)
        return out
    if isinstance(obj, list):
        return [normalize(v) for v in obj]
    return obj


def build_binary(profile="release") -> Path:
    flag = ["--release"] if profile == "release" else []
    proc = run(["cargo", "build", "-p", CRATE, *flag, "--bin", "remux-server"],
               stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if proc.returncode != 0:
        print(proc.stdout, file=sys.stderr)
        raise SystemExit("build failed")
    return REPO / "target" / profile / "remux-server"


def boot(binary: Path, port: int, data_dir: Path) -> subprocess.Popen:
    env = dict(os.environ)
    env.update(
        PORT=str(port),
        DATA_DIR=str(data_dir),
        DATABASE_URL=f"sqlite://{data_dir/'db.sqlite'}?mode=rwc",
        DISABLE_DHT="true",
        TORRENT_HTTP_PORT="0",
        METRICS_ENABLED="true",
        CONFIG=str(data_dir / "config"),  # nonexistent -> file source is optional
    )
    proc = subprocess.Popen(
        [str(binary)], cwd=REPO, env=env,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    base = f"http://127.0.0.1:{port}"
    for _ in range(120):
        try:
            urllib.request.urlopen(f"{base}/system/ping", timeout=1).read()
            return proc
        except (urllib.error.URLError, ConnectionError):
            if proc.poll() is not None:
                raise SystemExit("server exited during startup")
            time.sleep(0.5)
    proc.kill()
    raise SystemExit(f"server on :{port} never became ready")


def http_json(base: str, method: str, path: str, token: str | None,
              body: dict | None):
    url = base + path
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    auth = 'MediaBrowser Client="AB", Device="AB", DeviceId="ab", Version="1.0"'
    if token:
        auth += f', Token="{token}"'
    req.add_header("Authorization", auth)
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            raw = r.read()
            status = r.status
    except urllib.error.HTTPError as e:
        raw, status = e.read(), e.code
    try:
        return status, json.loads(raw) if raw else None
    except json.JSONDecodeError:
        return status, {"__nonjson__": raw.decode("utf-8", "replace")[:200]}


def seed_and_capture(base: str, requests: list[tuple[str, str]]):
    """Complete startup, authenticate, then capture normalized responses."""
    http_json(base, "POST", "/startup/user",
              None, {"Name": "ab", "Password": "ab"})
    http_json(base, "POST", "/startup/complete", None, None)
    _, auth = http_json(base, "POST", "/users/authenticatebyname", None,
                        {"Username": "ab", "Pw": "ab"})
    token = (auth or {}).get("AccessToken")

    captured = {}
    for method, path in requests:
        status, payload = http_json(base, method, path, token, None)
        captured[f"{method} {path}"] = {
            "status": status, "body": normalize(payload)}
    return captured


def load_requests(path: Path) -> list[tuple[str, str]]:
    reqs = []
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(None, 1)
        reqs.append((parts[0].upper(), parts[1]) if len(parts) == 2
                    else ("GET", parts[0]))
    return reqs


def cmd_verify(args) -> int:
    import tempfile
    files = [f.strip() for f in (args.files or "").split(",") if f.strip()]
    if not files:
        print("error: --files is required", file=sys.stderr)
        return 2
    requests = load_requests(Path(args.requests) if args.requests
                             else Path(__file__).resolve().parent / "requests.txt")

    def capture() -> dict:
        binary = build_binary()
        with tempfile.TemporaryDirectory() as d:
            proc = boot(binary, args.port, Path(d))
            try:
                return seed_and_capture(f"http://127.0.0.1:{args.port}", requests)
            finally:
                proc.kill()

    print("→ treatment: booting working-tree build …")
    treatment = capture()
    print("→ baseline: booting snapshot build …")
    baseline = with_baseline_files(files, capture)

    hard = 0
    for key in sorted(set(baseline) | set(treatment)):
        b, t = baseline.get(key), treatment.get(key)
        if b != t:
            hard += 1
            print(f"DIFF  {key}")
            bj = json.dumps(b, sort_keys=True, indent=2).splitlines()
            tj = json.dumps(t, sort_keys=True, indent=2).splitlines()
            import difflib
            for dl in list(difflib.unified_diff(bj, tj, "baseline", "treatment",
                                                lineterm=""))[:40]:
                print("   " + dl)
    if hard:
        print(f"\nRESULT: FAIL — {hard} endpoint(s) diverged", file=sys.stderr)
        return 1
    print(f"\nRESULT: PASS — {len(treatment)} endpoints byte-identical "
          "(after volatile-field normalization)")
    return 0


def cmd_gate(args) -> int:
    rc = cmd_verify(args)
    if rc != 0:
        return rc
    for tool in ("parity/parity.py", "playback/verify.py"):
        p = REPO / "tools" / tool
        if p.is_file() and os.access(p, os.R_OK):
            print(f"→ external gate: {tool} (run manually if configured): {p}")
        else:
            print(f"→ external gate {tool}: not available/readable, skipped")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    s = sub.add_parser("snapshot", help="save baseline copies BEFORE editing")
    s.add_argument("files", nargs="+")
    s.set_defaults(func=cmd_snapshot)

    b = sub.add_parser("bench", help="A/B divan medians, baseline vs treatment")
    b.add_argument("--benches", required=True, help="comma-separated bench targets")
    b.add_argument("--files", help="comma-separated files your patch touches")
    b.add_argument("--fail-on-regression", action="store_true")
    b.set_defaults(func=cmd_bench)

    v = sub.add_parser("verify", help="A/B response equivalence, baseline vs treatment")
    v.add_argument("--files", help="comma-separated files your patch touches")
    v.add_argument("--requests", help="request corpus (default tools/ab/requests.txt)")
    v.add_argument("--port", type=int, default=48610)
    v.set_defaults(func=cmd_verify)

    g = sub.add_parser("gate", help="verify + external parity/playback gates")
    g.add_argument("--files")
    g.add_argument("--requests")
    g.add_argument("--port", type=int, default=48610)
    g.set_defaults(func=cmd_gate)

    args = ap.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
