# D5(c) baselines, measured 2026-11-03 (raw files: baseline-{base,target}-{check-*,clippy-default}.txt)
- check all-features / default: base (e03001c) 1 real warning (dead test helper `assert_eq_tab_path_contents`, src/app.rs, fixed by a fork issue); target 0.
- check no-default-features: base 34 warnings + target 32 (all pre-existing, upstream's). 
- clippy default (--keep-going): both trees have 2 lib warnings (`very complex type` gvfs.rs, `too many arguments` tab.rs) plus `examples/copy.rs` clippy ERROR (exit 101, upstream's own); target also `dialog.rs` collapsible-if at base only. So clippy is judged by findings, not exit code, as specced.
- Baseline per command/config = union of base and target findings, matched by lint+file+message.
