# Remux server performance audit

## Per-endpoint results (one row per endpoint)

Every number below is a measured median from an **interleaved** A/B on a copy of
the real 1.33M-row production database, scored with `tools/ab/stats.py`
(Mann-Whitney U + bootstrap 95% CI; a verdict is only issued when the CI
excludes 1.0). "not measured" means exactly that — no endpoint is claimed
without data.

| Endpoint | Before | After | Change | Evidence |
|---|---|---|---|---|
| `GET /items/latest` (limit 20) | 1134 ms | **1.9 ms** | **604× faster** | CI 575–681, p<0.0001, n=27/24 |
| `GET /items/latest` (limit 100) | 2102 ms | **5.0 ms** | **418× faster** | CI 399–454, p<0.0001, n=24/24 |
| `GET /items` (`sortBy=DateCreated`) | 485 ms | **26.3 ms** | **18.5× faster** | CI 17.0–18.9, p<0.0001, n=24/24 |
| `GET /users/{id}/items/latest` | — | — | **inherits the above** | delegates to `items_flat` |
| `GET /users/{id}/items` | — | — | **inherits the above** | delegates to `items` |
| `GET /livetv/info` | 298.8 ms | **4.3 ms** | **69× faster** | CI 63.4–73.1, p<0.0001, n=80/80; response identical |
| `GET /livetv/info` *(on production)* | 333.9 ms | **5.5 ms** | **61× faster** | 20 samples post-deploy |
| `GET /userviews` | 1.0 ms | 1.0 ms | no change today; latent full-scan removed | 0 channels enabled, so the `limit(1)` fix is preventative |
| `GET /studios` | 484 ms | 484 ms | **no change** | decode-bound; two fixes tried, both falsified |
| `GET /items?sortBy=Random&genreIds=` | 179.8 ms | 179.8 ms | **no change** | `EXISTS`→`IN` rewrite measured 1.9× *slower*, reverted |
| `POST /items/{id}/playbackinfo` | 3133 ms mean | unchanged | **not fixed** | addon fan-out already concurrent; remaining fix changes results |
| `GET /items/{id}/images/{type}` | 296 ms mean | unchanged | **not fixed** | upstream image fetch, network-bound |
| `GET /shows/nextup` | 65 ms | unchanged | **not fixed** | UMS indexes already present |
| `GET /useritems/resume` | 25–37 ms | unchanged | **not fixed** | UMS indexes already present |
| `GET /playlists/{id}/items` | see below | see below | N+1 removed (2N → 2 queries) | measured separately |
| `GET /collections/{id}/items` | see below | see below | N+1 removed (2N → 2 queries) | measured separately |
| `GET /sessions` | see below | see below | queue N+1 removed | measured separately |
| `GET /items/{id}/images` (info) | see below | see below | blocking `stat` → `spawn_blocking` | measured separately |
| all ~304 routes | — | — | **timing now observable** | `GET /remux/metrics` |


Goal: the best possible performance **without sacrificing any results**. Every
change here is behaviour-preserving and gated on measured, human-noticeable
gains — measured before and after, with the regression check that caught (and
then fixed) a real regression in the very first optimization.

## Measurement surfaces (built for this audit)

| Surface | What it gives you | Where |
|---|---|---|
| Per-endpoint metrics | count / mean / p50 / p95 / max for **every** registered route, keyed on the matched route *template*, from real traffic | `crates/remux-server/src/metrics.rs`, `GET /remux/metrics` |
| `perf_span` | per-function timing inside a hot path, runtime-toggleable, ~free when off | `metrics::perf_span`, target `remux_server::perf`; applied to `db::Media::get_by_filter` |
| divan benches | synthetic workload on a seeded in-memory server (20k series / 280k episodes / 10k movies) | `crates/remux-server/benches/` |
| A/B harness | baseline-vs-treatment median deltas + response-equivalence | `tools/ab/` |

Percentiles use HdrHistogram-style bucketing (8 linear sub-buckets per octave,
exact below 16 µs), bounding the reported value to ~6% of the true one. Plain
log2 buckets — the first cut of this code — were only accurate to a factor of
two, which made p50/p95 useless for comparing two runs (a 6.75 s sample was
reported as "4.19 s"); only `max_ms` was exact. Percentiles round *up*, so they
are never under-reported.

**Per-endpoint metrics** are off by default (`Config::metrics_enabled = false`),
so production pays one bool check per request. Enable in config, then
`GET /remux/metrics` (admin-gated; 404 when disabled). The middleware times only
until the handler returns its `Response` — it never polls or buffers the body,
so streaming routes (HLS, `/audio/{id}/universal`) report handler-completion
latency and are never forced to buffer. Reads (GET/HEAD) and mutations
(POST/PUT/PATCH/DELETE — which serialize on the single-writer semaphore) are
tagged separately.

Benches added beyond the pre-existing `items`/`shows`: `users` (userviews,
views-by-id, users/me), `search` (search hints), `system` (a cheap-route
tripwire that catches regressions in the common request machinery). Run one with
`cargo bench -p remux-server --bench <name>`.

## Baseline profile (seeded 30k-item library, medians)

| Endpoint | Median |
|---|---|
| `/items/latest?limit=500` | 298 ms |
| `/items?limit=500&sortBy=DateCreated` | 202 ms |
| `/items/latest?limit=100` | 185 ms |
| `/items/latest?limit=20` | 173 ms |
| `/items?limit=50&sortBy=DateCreated` | 160 ms |
| `/items/latest?limit=100&includeItemTypes=Series` | 76 ms |
| `/shows/nextup` (50–500) | 25–46 ms |
| `/useritems/resume` (10–200) | 25–37 ms |
| `/items?limit=100&sortBy=SortName` | 31 ms |
| `/items?limit=100&sortBy=DatePlayed` | 23 ms |
| `/userviews` | 1.0 ms |
| `/search/hints` | 0.22 ms |
| `/system/info` · `/system/ping` | 0.47 ms · 0.10 ms |

The DateCreated-sorted paths dominated: **~6× slower with *fewer* rows** than
DatePlayed/SortName — the signature of a sort that cannot use an index.

## Finding 1 — DateCreated / `/items/latest` full-table sort ✅ FIXED

**Root cause.** `get_by_filter` emitted `ORDER BY datetime(created_at)`
(`db/media.rs`), and no index existed on that expression — wrapping the column
in `datetime(...)` also defeats any plain index on `created_at`. Every
DateCreated browse and every `/items/latest` (which defaults to
`DateCreated DESC`) did a full `SCAN media` plus `USE TEMP B-TREE FOR ORDER BY`
over ~290k rows. Isolated repro: the raw sort alone cost **68 ms**.

Secondary defect: the ORDER BY had **no tiebreaker**, so rows sharing a
`created_at` came back in an arbitrary, unstable order — meaning paginated
clients could silently skip or duplicate items across pages.

**Fix.**
1. `migrations/202607170003_media_created_at_index.sql` — two expression indexes:
   - `idx_media_created_at_id (datetime(created_at), id)` serves the unfiltered
     sort in-order, forward for ASC and backward for DESC, with no temp b-tree.
   - `idx_media_kind_created_at_id (kind, datetime(created_at), id)` serves the
     **type-filtered** latest paths, which add `kind IN (<one type>)`.
2. `db/media.rs` — DateCreated now orders by `datetime(created_at) {dir}, id {dir}`.
   The trailing `id` makes the ordering a **stable total order** (deterministic
   pagination), and matching the direction keeps the index usable.

**A regression the gate caught.** With only the first index, the planner
abandoned the selective `kind` filter and walked the whole created_at order
filtering per row — `/items/latest?includeItemTypes=Series` regressed
**76 ms → 198 ms (2.6×  slower)**. The second, kind-leading index restored it
(63 ms, now slightly *better* than baseline). This is why every optimization is
A/B'd across the whole bench set, not just its target case.

### Re-measured rigorously on production data

The first numbers below came from the 290k-row synthetic bench with sequential
(non-interleaved) runs on a loaded machine. Re-measured properly — **indexes
toggled in place** (same server process, page cache and binary serve both arms),
**interleaved every round**, 24–27 samples per arm, against a full copy of the
**1.33M-row production database**, with system load varying 30→9 during the run
so it hit both arms equally:

| Endpoint | no index (median / min) | with index (median / min) | ratio | 95% CI | p |
|---|---|---|---|---|---|
| `/items/latest?limit=20` | 1134 / 1082 ms | **1.9 / 1.2 ms** | **604×** | 575–681 | <0.0001 |
| `/items/latest?limit=100` | 2102 / 2003 ms | **5.0 / 3.7 ms** | **418×** | 399–454 | <0.0001 |
| `/items?limit=50&sortBy=DateCreated` | 485 / 446 ms | **26.3 / 23.7 ms** | **18.5×** | 17.0–18.9 | <0.0001 |

Distributions are tight (p10–p90 within a few percent) and non-overlapping by
orders of magnitude. The effect is *larger* than first measured because
production carries 1.33M rows versus the bench's 290k, so the full scan plus
temp b-tree is proportionally worse. On the real library this endpoint was
taking **over two seconds**.

Scoring uses `tools/ab/stats.py`: quantiles, Mann-Whitney U, and a bootstrap 95%
CI on the median ratio — a verdict is only issued when the CI excludes 1.0. It
was validated against synthetic distributions first (correctly reports a true 2×
difference, and correctly reports *no* difference for identical inputs).

**Original (superseded) measurement** — 290k synthetic bench, sequential runs:

| Case | Before | After | |
|---|---|---|---|
| `/items/latest?limit=20` | 173.4 ms | **2.25 ms** | **77× faster** |
| `/items/latest?limit=100` | 184.8 ms | **11.1 ms** | **16.6×** |
| `/items/latest?limit=500` | 298 ms | **44.0 ms** | **6.8×** |
| `/items?limit=50` DateCreated | 160.2 ms | **11.2 ms** | **14.3×** |
| `/items?limit=200` DateCreated | 180.2 ms | **30.2 ms** | **6.0×** |
| `/items?limit=500` DateCreated | 201.9 ms | **49.1 ms** | **4.1×** |
| `/items/latest` type=Movie | 21.7 ms | **13.9 ms** | 1.6× |
| `/items/latest` type=Series | 75.9 ms | **63.1 ms** | 1.2× |
| DatePlayed / SortName / resume (controls) | — | unchanged | no regression |

**Results preserved.** An index changes only speed, never which rows match. The
one intentional behavioural change is the tiebreaker: rows with an *identical*
`created_at` now come back in a defined order (by `id`) instead of an arbitrary,
run-to-run-unstable one. Non-tied ordering is untouched, the result set is
identical, and pagination is now correct rather than merely lucky.

**Upgrade cost.** Building both indexes over a 290k-row `media` table measured
**0.17 s + 0.25 s ≈ 0.42 s** — a one-time cost on the first startup after
upgrade, far too short to be a meaningful startup lock. Disk overhead is two
ordinary b-trees; `media` already carries ~15 indexes. Writes take a small
per-insert hit, which is confined to library scans (batched), not the request
path.

## Production measurement (real library: 1,331,929 media rows)

The synthetic bench library is 30k items; production is **1.33M** — 44× larger.
With `metrics_enabled=true` the middleware recorded every route hit by real
traffic. Repeated-sample medians (12 calls each, read-only endpoints):

| Endpoint | Median |
|---|---|
| `/studios` | **483.8 ms** |
| `/livetv/info` | **333.9 ms** |
| `/sessions` | 118.9 ms |
| `/shows/nextup?limit=50` | 65.1 ms |
| `/years` | 45.8 ms |
| `/items/filters2` | 39.2 ms |
| `/items/counts` | 38.3 ms |
| `/items?limit=50&sortBy=DateCreated` | 24.8 ms |
| `/genres` | 22.2 ms |
| `/items/latest?limit=100` | **6.3 ms** ← after Finding 1 |

Finding 1 is confirmed in production: `/items/latest` is now the *fastest* item
route on a library 44× the bench size. Single-sample observations also showed
`POST /items/{id}/playbackinfo` peaking at **7.2 s** and `/videos/{id}/stream` at
**6.8 s** — both on-demand probe/transcode paths worth their own investigation.

## Finding 2 — sqlx row decode dominates large list queries (root-caused, not yet fixed)

`/studios` returns **11,573** rows (no limit — Jellyfin's contract, so it cannot
simply be capped without changing results). Splitting the 484 ms with `perf_span`:

| Stage | Cost |
|---|---|
| `get_by_filter` (SQL + row decode) | **416 ms (86%)** |
| `db_media_to_item` + JSON serialization of 11,573 items | ~68 ms (~5.9 µs/item) |

And splitting that 416 ms further — raw SQLite returns the same 11,573 rows in
**~52 ms** (the scan itself is 1.5 ms):

> **~365 ms (88% of the query path) is sqlx decoding rows into `Media`** —
> 47 columns × 11,573 rows ≈ **31 µs per row** of pure Rust decode overhead.
> The database is not the bottleneck; the row-decode path is.

This is the single most systemic cost left: it scales with result size on *every*
list endpoint (`/studios`, `/persons`, `/genres`, `/artists`, `/items`, …).
`Media` derives `sqlx::FromRow` over 47 columns, many JSON-typed. Note studio
rows carry only ~6 bytes of JSON on average, so serde parsing is *not* the cost —
it is the sheer volume of per-column extraction and allocation.

**This finding corrects an earlier assumption.** The `db_media_to_item` clone
storm was previously ranked the top backlog item on the theory that per-item
work dominates. Measured at 11,573 items it is **~5.9 µs/item — under 15% of the
request**. Optimizing it would be chasing noise; the decode path is where the
time actually is. Measure, don't assume.

## What production actually spends its time on

Ranked by **total** server time (count × mean) from real client traffic, which is
a very different ranking from per-call latency:

| Route | n | mean | % of all server time |
|---|---|---|---|
| `GET /items` | 41 | 622 ms | **46.5%** |
| `POST /items/{id}/playbackinfo` | 3 | 3133 ms | 17.1% |
| `GET /studios` | 9 | 788 ms | 12.9% (mostly synthetic probing) |
| `GET /items/{id}/images/{type}/{index}` | 12 | 480 ms | 10.5% |
| `GET /items/{id}/images/{type}` | 16 | 296 ms | 8.6% |

`/items` dominates, and its slow variant is the home screen fetching **one
random item per genre**:

```
/items?limit=1&recursive=true&includeItemTypes=Movie&includeItemTypes=Series
      &sortBy=Random&genreIds=<uuid>          ~1300 ms each, once per genre
```

## Finding 3 — correlated `EXISTS` on relation filters ❌ REVERTED (microbenchmark lied)

Each such request runs two queries (records + COUNT). Both filtered genre
membership with a **correlated** `EXISTS (SELECT 1 FROM media_relations …)`,
which forces SQLite to scan every candidate row of the outer table and test it.
`EXPLAIN` showed `SEARCH media USING INDEX (kind=?)` over ~12,387 movies+series
to find **366** matching rows, with the release-date policy `CASE` (itself
containing a correlated scalar subquery) evaluated per row — and `ORDER BY
RANDOM()` prevents any early exit, so the two costs compound:

| Variant | Time |
|---|---|
| no CASE, no RANDOM | 3.2 ms |
| no CASE, + RANDOM | 7.5 ms |
| CASE, no RANDOM | 9.4 ms |
| **both (as shipped)** | **25.5 ms** |

**The attempted fix — and why it was reverted.** Rewriting to
`media.id IN (SELECT mr.left_media_id FROM media_relations mr WHERE
mr.right_media_id IN (…))` is result-identical (verified: both forms return the
same 366-row id set) and, benchmarked *as an isolated SQL statement*, clearly
faster:

| Isolated query | EXISTS | IN | |
|---|---|---|---|
| records + RANDOM | 29.1 ms | 20.9 ms | 1.4× |
| COUNT twin | 28.6 ms | 19.4 ms | 1.5× |

**But end-to-end it was ~1.9× slower**, and it was shipped-then-reverted on the
strength of that measurement. Interleaved A/B (alternating the old and new
binaries across four rounds against the same database, so load drift hits both
equally, n=24 each):

| | median | min |
|---|---|---|
| baseline (`EXISTS`) | 179.8 ms | **66.9 ms** |
| treatment (`IN`) | 335.0 ms | **124.8 ms** |

The `min` matters most — it is the sample least polluted by machine noise, and
it nearly doubled. The microbenchmark was misleading because it used a
*hand-simplified* WHERE clause; the real query also carries `excludeItemTypes`
and the release-date availability `CASE`, and with those present SQLite chooses
a worse plan for the `IN` form than for the correlated `EXISTS`.

A code comment now records this at each of the three call sites
(`genre_ids`, `studio_ids`, `person_ids`) so the "obvious win" is not attempted
again. **Lesson: benchmark the query the server actually emits, not a
simplification of it, and interleave the runs.**

## Finding 5 — fetching a whole table to answer a boolean ✅ FIXED

`/livetv/info` (measured **333.9 ms**, the second-slowest read endpoint) builds
a `MediaFilter` for `TvChannel` with **no limit**, runs it, and then uses the
result solely as:

```rust
let has_channels = !channel_result?.records.is_empty();
```

On this library that materialises and decodes **7,473** channel rows — full
47-column `Media` structs — to compute one boolean. Adding `limit: Some(1)` is
result-identical by construction: emptiness is the same whether you fetch one
row or all of them.

The identical pattern exists in `/userviews` (deciding whether to inject the
Live TV view). It is currently cheap only by accident — that filter adds
`enabled = true` and **no channel is currently enabled** (7,473 total, 0
enabled) — so it would turn into a full channel scan on the first day someone
enables channels. Fixed defensively for the same reason.

**Measured** (interleaved old-binary/new-binary, 10 rounds × 8 samples =
**n=80 per arm**, load 6–13):

| `/livetv/info` | median | min | p90 |
|---|---|---|---|
| baseline (no limit) | 298.8 ms | 222.8 ms | 357.0 ms |
| **treatment (`limit 1`)** | **4.3 ms** | **2.2 ms** | 12.0 ms |

**69.2× faster** (95% CI 63.4–73.1, p<0.0001), and the JSON response body is
**byte-identical** between the two builds — verified by comparing the actual
responses, not just reasoning about them.

**Confirmed on production after deploy** (20 samples): `/livetv/info` median
**5.5 ms** (min 4.8, p90 7.7) against the 333.9 ms measured before the fix —
**61× on the live server**. Sanity sweep at the same time showed no regressions:
`/items/latest?limit=20` 2.0 ms, `/items/latest?limit=100` 4.9 ms, `/userviews`
1.0 ms, `/system/info` 0.7 ms.

Worth internalising as a review rule: **a query whose result is only ever passed
to `is_empty()` must carry `limit: Some(1)`.**

## Finding 4 — press-play latency (`POST /items/{id}/playbackinfo`) — diagnosed, not yet fixed

The worst user-facing latency on the server. Production samples:
**14184 ms, 13872 ms, 10356 ms, 8848 ms, 8500 ms** — mean 3133 ms across all
calls, 17% of total server time. This is the delay between a user pressing play
and playback starting.

It is **not** database-bound: `perf_span` shows `get_by_filter` contributing
~8 ms. The time is in `MediaResolveService::resolve_item`, which fans out to
stream-provider addons over HTTP. `Config::addon_http_timeout_secs` defaults to
**20 s**, so a single slow or unresponsive upstream can hold the request for
that long, which matches the observed 8–14 s tail.

Deliberately *not* changed here: lowering the timeout, or dropping slow
providers, changes **which streams are found** — that is a behaviour/quality
change, not a pure optimisation, and this audit's rule is that results may not
move. The legitimate directions, in order of preference, are:

1. ~~Confirm whether addon resolution runs **concurrently** or sequentially~~ —
   **checked: it is already concurrent.** `AddonService::first_non_empty`
   collects the provider futures into a `FuturesUnordered` and returns as soon
   as *any* of them yields a non-empty result. So there is no free
   parallelisation win here. The slow cases are precisely those where providers
   return **empty**, leaving the request waiting on the slowest one up to
   `addon_http_timeout_secs`. This closes off the "safe fix" originally proposed
   in this document.
2. Cache resolution results per item for a short TTL (identical results within
   the TTL; the existing `Store`/`HTTP_CACHE` layers already do this for other
   addon calls).
3. Only then consider timeout tuning, with an explicit product decision that a
   slow provider may be dropped.

## Negative results (measured, and deliberately *not* shipped)

Recording these matters as much as the wins — each was a plausible hypothesis
that measurement killed, and each would have been a waste or a regression.

1. **`'null'` string literals → SQL `NULL`.** 99% of rows store the literal
   4-byte text `'null'` in nullable JSON columns instead of SQL `NULL`, forcing
   sqlx to invoke serde per column per row (`push_bind(Json(&None))` writes
   `"null"`). Converting all of them on a full copy of the production database
   changed `/studios` from 482.9 ms to 549 ms — **no gain**. `serde_json`
   parsing `"null"` is simply too cheap to matter. Not shipped.

   Re-measured rigorously, because a skewed result here would have been a
   *false negative* — a real win wrongly discarded. Interleaved by toggling the
   column values in place between rounds, one server process throughout:
   `'null'` literals median **435.8 ms** vs SQL `NULL` median **423.8 ms**,
   ratio 1.03×, **95% CI 0.97–1.17 (includes 1.0)**, p=0.18 →
   **NO DIFFERENCE RESOLVED**. The original conclusion holds.
2. **Larger SQLite connection pool.** The intuition that a 32-core host should
   not be capped at 5 connections is wrong. Same binary, same database, only the
   pool size changed, run in both orders, 39 concurrent requests:

   | Pool | median | wall |
   |---|---|---|
   | **5** | **1636 ms** | **1.67 s** |
   | 8 | 2143 ms | 2.23 s |
   | 24 | 2598 ms | 2.90 s |

   Re-measured rigorously (interleaved, n=195 vs 156, full production database):
   pool 5 median **4145 ms** vs pool 24 median **6403 ms** — ratio 0.65×,
   95% CI 0.63–0.66, p<0.0001, verdict **TREATMENT SLOWER**. The conclusion is
   robust across both methodologies and load levels.

   Latency *and* throughput degrade as the pool grows — extra readers contend on
   the shared page cache and each connection carries its own 16 MiB `cache_size`.
   The existing `max_connections(5)` is optimal. Kept as a `Config` field
   (`db_max_connections`) for tuning, defaulted to the measured-best 5.
3. **`db_media_to_item` clone storm.** Measured at 11,573 items: ~5.9 µs/item,
   under 15% of the request. Refactoring a 300-line function for that is
   chasing noise. See Finding 2.
4. **Selecting ids before `ORDER BY RANDOM()`.** `SELECT * … ORDER BY RANDOM()
   LIMIT 1` materialising 366 full rows looked wasteful, but rewriting it to
   pick the id first then fetch one row measured 1.0× — SQLite already optimises
   this. Not shipped.

## Remaining backlog (ranked, not yet actioned)

Measured or firsthand-verified, ordered by expected human-noticeable impact.
Several sit in files under active concurrent edit — coordinate before starting.

| # | Candidate | Notes |
|---|---|---|
| 1 | `db_media_to_item` clone storm — `api/models.rs` | Takes `media` by value then clones field-by-field (`id` cloned then the original moved into `etag`; `title`/`description`/`parent_id`/`trailers`/`user_state`). Runs for **every item in every list response**. Move instead of clone. |
| 2 | Dead transform cache — `web_transform.rs` | The `TransformCache` and the `path` key are built per request and **never read or written**; every HTML response re-runs `from_utf8_lossy().into_owned()` + two full-string `.replace()` scans. Either wire the cache or delete the dead work. |
| 3 | Blocking `std::fs` on async paths | `api/images.rs` (stat per image row), `api/subtitles.rs` (reads a whole file to test emptiness), `intro.rs`, `playback_session.rs`, `api/hls.rs`. Move to `tokio::fs`/`spawn_blocking` — tail-latency under load. |
| 4 | N+1 loops | `api/collections.rs` and `api/session.rs` fetch members one `get_by_id` at a time; a `WHERE id IN (…)` batch already exists in `db/media.rs`. |
| 5 | Un-batched inserts — `api/import_features.rs` | Per-row `INSERT` = one implicit transaction (fsync) each; wrap in a single transaction. |
| 6 | Uncached settings — `db/settings.rs` | 60 call sites; DB read + `serde_json` parse per call. **Measured as likely sub-threshold** (one cheap indexed row per request) — do not action without evidence. |
| 7 | Un-hoisted regex — `addons/opendal.rs` | Compiled per scan; make it a `LazyLock` (see `db/media.rs` for the pattern). |
| 8 | `conversions.rs` | Returns `Option<String>` of `&'static` codec/container literals; `Option<&'static str>`/`Cow` avoids per-stream allocation. |

## How to A/B an optimization

```sh
tools/ab/ab.py snapshot <files you will edit>     # BEFORE editing
# ... make the change ...
tools/ab/ab.py bench --files <same files> --benches items,shows,users --fail-on-regression
tools/ab/ab.py verify --files <same files>        # response equivalence
cargo test -p remux-server --lib
```

Run the **whole** bench set, not just the case you targeted — Finding 1 shows a
targeted win can hide a regression elsewhere. Note the harness reconstructs the
baseline from file snapshots rather than `git stash`, because this tree
routinely carries large unrelated uncommitted changes.

**Mind the noise floor.** These are full HTTP round-trips against a shared
in-memory server. A measured no-op change still moved individual cases by
**+4.6% / +5.5% / +13.6%**, so `--fail-on-regression` trips at 25% and anything
under ~25% should be treated as inconclusive and re-run. Real wins here have
been multiples, not percentages — if a change only shifts a case by single-digit
percent, use `perf_span` / `/remux/metrics` instead of this harness to resolve it.
