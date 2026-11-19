#!/usr/bin/bash
# Scratch-repo test of scripts/land.sh: what it lands, and what it refuses.
# Run: bash scripts/tests/land_test.sh
set -uo pipefail

LAND=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/land.sh
ROOT=$(mktemp -d -p "${LAND_TEST_TMP:-${TMPDIR:?set TMPDIR or LAND_TEST_TMP}}" land-test.XXXXXX) || exit 1
[[ $ROOT == */land-test.* ]] || exit 1
trap 'rm -rf "$ROOT"' EXIT
# The scratch repositories use no user or system git configuration (a signing key, hooks, a template).
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
FAILED=0

# new_repo <name>: a main checkout on master with one commit, and a worktree <name>/wt on branch feat.
new_repo() {
    local dir=$ROOT/$1
    git init -q -b master "$dir/main"
    mkdir -p "$dir/main/src"
    echo one >"$dir/main/src/a.rs"
    git -C "$dir/main" add src/a.rs
    git -C "$dir/main" commit -q -m "base"
    git -C "$dir/main" worktree add -q -b feat "$dir/wt"
    printf 'fix: change a\n\nFork-issue: minyek/cosmic-files#1\n' >"$dir/code-msg"
    printf 'docs: record\n' >"$dir/records-msg"
}

# land <name> [--push]: run land.sh for that repo; the output is in $OUT, the status in $STATUS.
land() {
    local dir=$ROOT/$1
    OUT=$(bash "$LAND" "$dir/main" "$dir/wt" "$dir/code-msg" "$dir/records-msg" ${2:+"$2"} 2>&1)
    STATUS=$?
}

check() { # check <name> <condition-status> <detail>
    if [ "$2" -eq 0 ]; then echo "ok   $1"; else echo "FAIL $1: $3"; FAILED=1; fi
}

# 1. A linear branch lands: master ends at the branch's tree.
new_repo linear
echo two >"$ROOT/linear/wt/src/a.rs"
git -C "$ROOT/linear/wt" commit -q -am "wip"
land linear
[ "$STATUS" -eq 0 ] && [ "$(git -C "$ROOT/linear/main" rev-parse 'master^{tree}')" = "$(git -C "$ROOT/linear/wt" rev-parse 'HEAD^{tree}')" ]
check "a linear branch lands at the branch's tree" $? "status=$STATUS: $OUT"

# 2. A branch that holds a merge commit is refused, and master does not move.
new_repo merged
git -C "$ROOT/merged/wt" switch -q -c side
echo side >"$ROOT/merged/wt/src/b.rs"
git -C "$ROOT/merged/wt" add src/b.rs
git -C "$ROOT/merged/wt" commit -q -m "side"
git -C "$ROOT/merged/wt" switch -q feat
echo two >"$ROOT/merged/wt/src/a.rs"
git -C "$ROOT/merged/wt" commit -q -am "wip"
git -C "$ROOT/merged/wt" merge -q --no-ff -m "merge side" side
echo four >"$ROOT/merged/wt/src/a.rs"
git -C "$ROOT/merged/wt" commit -q -am "after the merge"
before=$(git -C "$ROOT/merged/main" rev-parse master)
land merged
[ "$STATUS" -ne 0 ] && grep -qi 'merge commit' <<<"$OUT" && [ "$(git -C "$ROOT/merged/main" rev-parse master)" = "$before" ]
check "a branch with a merge commit is refused and master stays" $? "status=$STATUS: $OUT"

# 3. When master moved, land.sh tells the user to restack, never to merge master into the branch.
new_repo moved
echo two >"$ROOT/moved/wt/src/a.rs"
git -C "$ROOT/moved/wt" commit -q -am "wip"
echo three >"$ROOT/moved/main/src/c.rs"
git -C "$ROOT/moved/main" add src/c.rs
git -C "$ROOT/moved/main" commit -q -m "master moves"
land moved
[ "$STATUS" -ne 0 ] && grep -q 'rebase' <<<"$OUT" && ! grep -qi 'merge master into the branch' <<<"$OUT"
check "a moved master is answered with the restack" $? "status=$STATUS: $OUT"

# 4. A path on neither list stops the landing.
new_repo unclassified
echo x >"$ROOT/unclassified/wt/build.rs"
git -C "$ROOT/unclassified/wt" add build.rs
git -C "$ROOT/unclassified/wt" commit -q -m "wip"
land unclassified
[ "$STATUS" -ne 0 ] && grep -q 'unclassified path build.rs' <<<"$OUT"
check "a path on neither list stops the landing" $? "status=$STATUS: $OUT"

# 5. The test script itself is a fork-only path: a branch that edits it lands as a records commit.
new_repo records
mkdir -p "$ROOT/records/wt/scripts/tests"
echo "# test" >"$ROOT/records/wt/scripts/tests/land_test.sh"
git -C "$ROOT/records/wt" add scripts/tests/land_test.sh
git -C "$ROOT/records/wt" commit -q -m "wip"
land records
[ "$STATUS" -eq 0 ] && ! grep -q unclassified <<<"$OUT" && [ "$(git -C "$ROOT/records/main" rev-parse 'master^{tree}')" = "$(git -C "$ROOT/records/wt" rev-parse 'HEAD^{tree}')" ]
check "the test script lands as a fork-only path" $? "status=$STATUS: $OUT"

# 6. After a sync the branch descends from the old master, which a backup tag keeps: the restack
#    command names that old master as the base, not the branch's first parent by guesswork.
new_repo synced
echo two >"$ROOT/synced/wt/src/a.rs"
git -C "$ROOT/synced/wt" commit -q -am "wip"
old_master=$(git -C "$ROOT/synced/main" rev-parse master)
git -C "$ROOT/synced/main" tag backup/main-pre-sync-20260101T000000Z "$old_master"
git -C "$ROOT/synced/main" commit -q --amend -m "base, rebased by a sync"
land synced
[ "$STATUS" -ne 0 ] && grep -qF "git rebase --autosquash --onto master $old_master" <<<"$OUT"
check "the restack command names the old master a backup tag keeps" $? "status=$STATUS: $OUT"

# 7. --push with no upstream remote lands locally and says it pushed nothing.
new_repo nopush
echo two >"$ROOT/nopush/wt/src/a.rs"
git -C "$ROOT/nopush/wt" commit -q -am "wip"
land nopush --push
[ "$STATUS" -eq 0 ] && grep -q 'not pushed' <<<"$OUT"
check "--push without an upstream remote pushes nothing" $? "status=$STATUS: $OUT"

exit "$FAILED"
