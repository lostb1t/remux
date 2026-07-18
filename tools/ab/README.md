# Remux A/B before/after harness

A standalone, offline tool that **proves** a performance change is both faster
*and* behaviour-preserving. No CodSpeed cloud, no external Jellyfin. It is the
regression gate for the performance-optimization work: run it before and after
every change to measure the speedup and confirm responses did not move.

It complements the two other measurement surfaces:

- **`/remux/metrics`** (in-server, `metrics_enabled = true`) — per-endpoint p50/p95/max
  from *real* traffic, all ~300 routes, keyed on the matched route template.
- **divan benches** (`crates/remux-server/benches/`) — synthetic workload on a
  seeded in-memory server; this tool drives them for A/B.

## Why snapshots instead of `git stash`

This working tree routinely carries large unrelated uncommitted changes, so
`git stash` would move far more than your patch. Instead you snapshot the exact
files your optimization will touch **before** editing them; the tool
reconstructs the baseline by swapping those snapshot copies in, so the measured
delta is *only* your patch — regardless of any other in-flight edits.

## Workflow

```sh
# 1. BEFORE editing, snapshot the files your optimization will touch:
tools/ab/ab.py snapshot crates/remux-server/src/api/models.rs

# 2. Make the optimization.

# 3. Measure the speedup (baseline = snapshot, treatment = working tree):
tools/ab/ab.py bench \
    --files crates/remux-server/src/api/models.rs \
    --benches items,shows,users \
    --fail-on-regression

# 4. Prove responses are byte-identical (minus volatile fields):
tools/ab/ab.py verify \
    --files crates/remux-server/src/api/models.rs

# 5. Full gate (verify + optional external parity/playback truth gates):
tools/ab/ab.py gate --files crates/remux-server/src/api/models.rs
```

Your working files are always restored after each run (the swap is in a
`try/finally`).

## Subcommands

- **`snapshot <files…>`** — copy the current content of each file into
  `tools/ab/.snapshots/` as the baseline. Run once, before editing.
- **`bench --benches <a,b,…> --files <…>`** — build+run each divan bench target
  on the working tree (treatment) and on the snapshot baseline, parse the median
  of every case, and print a delta table. `--fail-on-regression` exits non-zero
  if any case is >5% slower.
- **`verify --files <…> [--requests f] [--port N]`** — build+boot the baseline
  and treatment `remux-server` binaries on a throwaway temp DB, complete the
  startup wizard, authenticate, replay the request corpus
  (`tools/ab/requests.txt`), and assert identical JSON after stripping volatile
  fields (Ids, timestamps, tokens, image-tag hashes, play counts, `UserData`).
  Non-zero exit + a unified diff per divergent endpoint on any difference.
- **`gate --files <…>`** — `verify`, then point at `tools/parity/parity.py` and
  `tools/playback/verify.py` as external truth gates if present.

## `stats.py` — scoring two samples honestly

```sh
tools/ab/stats.py "<label>" baseline_samples.txt treatment_samples.txt
```

Each file is one timing per line (seconds). It reports min / p10 / median / p90 /
max for both arms, then a **Mann-Whitney U** p-value and a **bootstrap 95% CI on
the median ratio**, and only issues `TREATMENT FASTER` / `TREATMENT SLOWER` when
that CI excludes 1.0 — otherwise `NO DIFFERENCE RESOLVED`. It was validated
against synthetic distributions (correctly finds a true 2× difference; correctly
reports no difference for identical inputs), so it cannot flatter a null result.

## Measuring on a *live media server* — hard-won rules

This host transcodes for real users, so it is never idle; system load during
measurement ranged from 9 to over 100. Every one of these rules was learned by
getting a wrong answer first:

1. **Interleave the arms every round.** Running all of A then all of B makes load
   drift indistinguishable from the effect. A sequential run gave "no
   difference", then "2.4× slower", then "equal" for the *same* change.
2. **Use large samples and multiple rounds.** A handful of samples on this box is
   noise, not evidence.
3. **Prefer `min` and low quantiles** when load is uncontrollable — they are far
   less polluted by competing CPU than the mean.
4. **Avoid restarting the server between arms** where possible. For a
   database-level change, toggle it in place (`DROP`/`CREATE INDEX`) so the same
   process, page cache and binary serve both arms.
5. **Benchmark the query the server actually emits.** A hand-simplified SQL
   microbenchmark said an `EXISTS`→`IN` rewrite was 1.4× faster; end-to-end it
   was ~1.9× *slower*, because the real query's extra clauses changed SQLite's
   plan choice. That change was shipped, caught, and reverted.

## Interpreting the numbers — measured noise floor

These benches are full HTTP round-trips against a shared in-memory server, so
variance is large (means routinely run several times the median). Benching a
**no-op comment change** through this harness still produced per-case deltas of
**+4.6% / +5.5% / +13.6%** — that is the noise floor, not a signal.

Consequently `--fail-on-regression` trips at **25%**, above the floor. Treat any
delta under ~25% as **inconclusive** and re-run before believing it. Genuine
wins found so far have been multiples (4×–77×), which sit far outside the noise;
if a change only moves a case by single-digit percent, this harness cannot
resolve it and you should either measure it another way (`perf_span` /
`/remux/metrics`) or not bother.

## Validation status of this tool

Be honest about what has actually been exercised:

| Piece | Status |
|---|---|
| divan median parser | ✅ validated against real `cargo bench` output |
| `snapshot` / baseline swap / restore | ✅ validated, including the "no snapshot" hard error |
| `bench` (full flow) | ✅ run end-to-end; a no-op change correctly produced a noise-level delta table and restored the working file |
| server build + boot + corpus replay | ✅ run end-to-end: 36/36 corpus requests returned <400 against a freshly-booted binary |
| `normalize` volatile-field stripping | ✅ unit-tested |
| `verify` (the two-build diff as a whole) | ⚠️ **not yet run end-to-end** — every component above is proven, but the full baseline-vs-treatment double-build diff has not been executed once. Expect to shake out rough edges the first time. |
| `gate` | ⚠️ depends on `verify`; the external parity/playback hooks only report availability |

## What each gate proves

| Gate | Proves |
|---|---|
| `bench` | the change is faster (per-case median delta, statistically sampled by divan) |
| `verify` | two identically-seeded builds return byte-identical responses on the corpus → behaviour preserved |
| `parity.py` | Remux still matches a real Jellyfin field-for-field (external truth) |
| `playback/verify.py` | audio/video streams still decode end-to-end |

`verify` compares an empty-library server's response *shapes/fields*; data-heavy
value equivalence is covered by the divan benches (which run the same seeded 30k
dataset before and after) plus the crate's integration tests. Treat a green
`bench` + `verify` + `cargo test` as the bar for landing a perf change.
