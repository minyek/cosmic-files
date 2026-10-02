# Background work and acceptance

- Background results must match both the request generation and the current
  consumer. Path or item identity alone cannot reject a stale result after an
  A-to-B-to-A transition. Preserve these checks in tab rescans, sidebar metadata,
  previews, desktop-entry parsing and picker save validation.
- Recents and default-handler mutations use FIFO workers. Clean shutdown must
  await their barriers through `persistence::flush`; spawning a thread or taking
  a mutex alone does not preserve submission order or ensure completion at exit.
- Recents acceptance must isolate the actual home directory. The
  `recently-used-xbel` dependency used by this code resolves its storage below
  the home directory; setting `XDG_DATA_HOME` alone does not isolate that data.
- Rendered thumbnail dimensions are not original image dimensions. Only carry
  metadata with known provenance into the details view.
- This repository's canonical functional gate is the three Cargo feature
  configurations in `.github/workflows/ci.yml`. Measure scrolling performance
  separately under controlled load; functional test counts do not establish
  frame-time improvements.

These facts were distilled from the scrolling worktree's implementation and
acceptance journal. The corresponding implementations are in `src/tab.rs`,
`src/app.rs`, `src/dialog.rs`, `src/recents.rs`, `src/mime_app.rs` and
`src/persistence.rs`.
