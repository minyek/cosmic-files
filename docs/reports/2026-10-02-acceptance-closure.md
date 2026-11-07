# Acceptance closure for the squash merge

The user approved closing the audit gaps before merging. Work remains isolated
from `master`; no installed application or real profile has been changed.

## Observed results

- Watcher panic recovery: two targeted tests passed. A separately compiled
  mutation that propagated the backend panic failed the new recovery test.
  The production reconciler now accepts the dependency's watcher/cache types
  generically, allowing the test to inject a one-time backend panic without
  changing its behavior.
- Drag selection: both grid and list tests passed through the full tab widget
  event pipeline at a scrolled position where earlier rows are culled. Tests
  assert the exact selected filenames. Removing the handlers and applying an
  incorrect scroll offset each failed both tests; restored source passed both.
- Picker completion: all three new public-wrapper tests failed on the original
  code. Framework close emitted completion while a real recents write was
  blocked. Repeated close actions after Save or Cancel also bypassed the
  pending-result guard. The corrected wrapper now publishes a result only after
  persistence drains and emits completion once. All 29 dialog tests passed.
  Removing the startup fallback failed its new test; restored source passed.
- Archive and rename: all 26 operation tests passed. Omitting archive encoder
  finalization failed both format tests; adding an unsafe rename fallback failed
  the unsupported-syscall test. Exact source restoration passed all 26 again.

The picker failure is now reproduced, superseding the earlier audit's
source-only concern. Its impact is picker result/close ordering and queued
persistence; the older App-only picker test did not exercise this public wrapper
boundary. The existing release remains the user-tested snapshot, but it does
not contain the lifecycle correction now included in the prepared source. The final gate, isolated
picker acceptance and release build must include that correction.

## Evidence

Targeted transcripts are retained under
`docs/reports/artefacts/sqmerge-closure/`. Each final source mutation check must
restore the original source exactly before integration. Cargo verification uses
the owning worktree and a serialized shared build directory: directly invoking
a cached test executable after another worktree has built can execute the wrong
test inventory. One such zero-test invocation was discarded and is not counted
as evidence. Retained text logs normalize trailing whitespace only.

## Complete functional gate and review

The independent [closure review](2026-10-02-closure-review.md) passed. The final
prepared-source gate at `9b5379c` passed: 235 unit plus 7 integration tests with
default features disabled, and 250 unit plus 8 integration tests in each default
and all-feature configuration. All three release executables and all acceptance
examples built successfully. The full transcript and source hashes are retained
in `artefacts/sqmerge-final/`. Existing unused-code warnings remain in the minimal
feature configuration; the default, all-feature and release builds were clean.

## Final-source runtime acceptance

All six isolated scenarios passed against exactly the gate's source/tool hash
manifest: production XBEL ordering and recents scan; real trash deletion,
listing and byte-preserving restoration; recents initialization failure under
a write limit followed by successful recovery; local FTP/GVFS enumeration with
real FUSE name/type/MIME parity through initial, nested, create and remove
stages; X11 and nested Wayland interaction; and the real file-picker example.
The remote backend also preserved an occupied rename destination.

Direct visual inspection of 24 retained screenshots confirmed ordinary menus
for directory/invalid desktop entries, the parsed launcher menu, 320x180 image
dimensions, wheel scrolling and the selected last entry in 20,000-item list/grid
views, watcher insertion/removal, and the picker's returned path. The picker
uses descending order in this fixture, so its selected last item is
`entry-00000.txt`. These observations, exact image names and the run outputs are
retained in `artefacts/sqmerge-final/runtime/`; `visual-review.json` distinguishes
visual observations from the harness's process/window assertions.

These runs used temporary profiles, private displays/D-Bus and loopback FTP.
They do not establish physical-GPU presentation timing, WAN behavior, every
remote backend, or non-Linux atomic-rename support.

## Separate frame measurement

The exclusive managed measurement passed all twelve runs: three repetitions of
1,000 and 20,000 entries in each view, with 60 warmup frames and 600 sampled
frames per run (7,200 samples total). Each run asserts that it reaches the end
of the listing. Environment, executable hash, scan timings and full distributions
are in `artefacts/sqmerge-final/performance/`.

| Entries | View | Median range (ms) | p95 range (ms) | p99 range (ms) | Worst sample (ms) |
|---:|---|---:|---:|---:|---:|
| 1,000 | List | 0.441–0.449 | 0.483–0.489 | 0.554–0.725 | 0.886 |
| 1,000 | Grid | 0.591–0.597 | 0.683–0.698 | 0.696–0.908 | 0.973 |
| 20,000 | List | 0.645–0.654 | 0.894–0.912 | 1.163–1.408 | 1.717 |
| 20,000 | Grid | 0.940–0.965 | 1.143–1.262 | 1.223–1.941 | 2.499 |

This is the development profile with optimization level 1 and default features
disabled. It measures widget construction, tree diff, layout, event dispatch and
draw-command generation. It excludes physical-GPU presentation and application
message handling after the sampled interval. There is no before/after speedup
claim. Scanning is reported separately.

The installed job launcher initially prevented admission after all jobs ended:
its one-level census counted two empty nested systemd slices as running jobs.
An isolated copy corrected that census to recurse through slices and count only
populated job units. Five fixture cases failed/passed as expected before/after;
the corrected live census returned zero. The root transferred only its own
verified queue request under the existing admission lock, preserving queue age.
The same memory, CPU, lock and exclusivity checks then admitted the measurement.
The shared launcher was not edited or restarted. The correction, failing/passing
checks and queue-transfer diff are retained in `artefacts/sqmerge-final/launcher-census/`.
The shared runner's owner should apply the recursive populated-unit census to
`live_job_count` and run its own canonical gate before installing that fix.

## Acceptance verdict

**MET.** The original clause-by-clause audits remain historical snapshots.
Their actual gaps are closed by this report and the independent closure review:
readiness AC2 (archive finalization), AC4 (picker persistence), AC10 (drag and
unsupported rename), the watcher/startup boundaries, AC11 (current evidence),
and branch-acceptance steps 1–5 (isolated runtime, measurement and review).
Unchanged clauses retain their original test mappings and passed the fresh full
matrix. The symlink-badge audit remains passed. Its optional combined-scenario
coverage limits are not additional user acceptance requirements.

Local integration and the requested release build from `master` are complete.
See the [delivery record](2026-10-02-main-delivery.md).
