# Independent audit-closure review

Reviewed commit `9b5379c381e1824ec83d69e1a7d95a9107e5d34b` in
`integration/main-preparation`, limited to closure commits `e616dd5`,
`bcea9b9`, `d4820d8`, and `fd80b09` and their retained evidence.

## Verdict

**PASS for the reviewed source and targeted regression boundaries.** I found no
critical, important, or minor code-review finding in this bounded scope. The
new tests provide distinct observable value, their retained mutation runs show
that the assertions are sensitive to the intended regressions, and the picker
lifecycle correction preserves the selected Save/Cancel result until the
persistence barrier completes.

This verdict does not close the original audit's final-source runtime clauses.
The canonical full gate, isolated storage/GUI/picker/remote acceptance, and the
separate exclusive frame measurement remain merge prerequisites owned by the
root review. `docs/reports/2026-10-02-acceptance-closure.md:44-48` records those
remaining steps accurately. The already-passed symlink-badge audit remains
unchanged and was not reopened.

## Reviewed boundaries

- **Watcher recovery (`e616dd5`): passed.** The test injects a one-time panic
  through a real generic `Debouncer`, requires the failed reconciliation to
  return an empty registered set, then reuses the returned watcher and requires
  both requested paths to register. The retained propagation mutant fails this
  test. Generalizing `reconcile` over the dependency's `Watcher` and
  `FileIdCache` traits changes no production ownership or recovery behavior.
- **Archive finalization and unsupported rename (`bcea9b9`): passed.** ZIP and
  TGZ tests execute the production `Operation::Compress` path in child
  processes, force a write failure at finalization, require the error to
  surface, and require the output directory to contain neither a destination
  nor a staging file. The archive-finalization mutant fails both tests. The
  Linux rename test verifies the seccomp fault with a raw `renameat2` syscall,
  executes the production rename path for three unsupported errno values with
  vacant and occupied targets, and checks source/target bytes after failure.
  Its unsafe-fallback mutant fails. Child execution is also asserted rather
  than silently skipped.
- **Drag selection after culling (`d4820d8`): passed.** Both tests build the
  actual responsive tab widget, apply real scrollable state, assert an earlier
  item is absent from the culled widget tree, dispatch press/move/release
  events through that tree, feed emitted messages through `Tab::update`, and
  require the exact two selected filenames. Removing both handlers and
  applying the wrong scroll offset each fails both tests, so the checks cover
  distinct handler presence and coordinate-transform regressions.
- **Picker persistence lifecycle (`fd80b09`): passed.** The public `Dialog`
  wrapper tests reproduce the original behavior for Save, Cancel, and
  framework `Action::Close`. A real recents write is held on an isolated FIFO;
  while blocked, the returned task may emit neither a result nor a close.
  Repeated framework-close and Cancel inputs must be inert. After release, the
  tests require the queued XBEL mutation, the correct preserved result, exactly
  one main-window close, exactly one result publication, and no response to
  repeated completion/close. The new `completed` state belongs with the
  dialog's result lifecycle: it gates publication until `PersistenceFlushed`
  and prevents later messages after the result has been taken. Returning
  `Message::Cancel` from `on_app_exit` routes framework close through the same
  barrier without overwriting an existing Save result.
- **Startup fallback (`fd80b09`): passed.** The test initializes an actually
  tabless application, delivers an empty launch-resolution result, requires an
  active current-directory fallback tab, drives the emitted rescan completion,
  and requires a populated listing. Removing the fallback fails this test.

## Placement and comments

The only new production field, `App::completed`, is lifecycle state consumed by
the same dialog implementation and is appropriately placed. Fault injection is
isolated under a test-only module. The new pre-exec comment explains the
otherwise non-obvious inheritance requirement; I found no change-history,
metadata, redundant, or commented-out source text in the reviewed additions.
No new production diagnostic wording was introduced.
