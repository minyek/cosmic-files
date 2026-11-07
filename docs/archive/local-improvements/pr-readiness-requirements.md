# PR readiness requirements

The final PR includes every branch commit and the accepted uncommitted repairs. It must have a current, honest description, reproducible verification, explicit limits, and no local IDE/journal artifacts.

1. URI-backed regular files have a file MIME type, and remote acceptance checks compare MIME as well as names and directory flags against the FUSE path.
2. A failed or cancelled archive operation leaves no newly published destination. Successful publication never replaces an existing destination; encoder and buffer finalization errors are observed.
3. Failed default-handler persistence preserves existing preference bytes, permissions, and symlink target semantics. Submission order remains FIFO.
4. Clean application exit and picker completion cannot discard already-submitted recents mutations. Persistence waits run off the GUI thread.
5. Save checks belong to one submission, including repeated submissions of the same path. Filename/location edits and cancellation invalidate older checks.
6. Every direct navigation transition supersedes pending resolution. Sidebar metadata completions belong to one context-menu request even when repeated requests use the same entity.
7. Watch bookkeeping records actual successful registrations. Failed registration remains retryable without creating an automatic retry loop.
8. Search item mutation preserves correct displayed ordering even when the result count stays at its cap. The 200-result boundary regression passes with the existing invalidation; the reported filename tie-order defect was not reproduced.
9. A displayed desktop action cannot invoke a different command after its backing file or selection changes. Activation uses the invocation's parsed action or rejects an obsolete invocation.
10. Current upstream drag-selection behavior survives integration with viewport culling. Rename retains its no-overwrite safety guarantee; unsupported atomic backend operations fail explicitly rather than silently using a racy replacement fallback.
11. Tests, source hashes, release binaries, PR text, and acceptance claims describe the final source. Missing personal DCO/understanding attestations remain for the human author; the agent does not certify them.

Existing passing checks remain evidence for their recorded snapshots. Remote MIME parity, new failure paths, and new request-identity cases require fresh regression and acceptance evidence. Physical-GPU latency and arbitrary WAN/server configurations remain outside the available acceptance environment.
