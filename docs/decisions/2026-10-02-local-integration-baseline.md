# Baseline for local integration

Use `b326e48` as the parent of the combined-improvement squash. It preserves the
twelve upstream commits already included in the tested combined branch. The old
main tip, `06278ee`, is its ancestor, so main can advance by fast-forward after
the accepted squash is created in an isolated worktree.

The local `upstream/master` reference at `5d77409` contains six newer commits
outside that tested source. Adding them would expand this integration beyond
the user-tested improvements. Leave them for a separately verified update.

The resulting squash, `66dac3f`, exactly matches the accepted preparation tree.
The [delivery record](../reports/2026-10-02-main-delivery.md) records its release
build and installation check.
