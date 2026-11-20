#!/usr/bin/bash
# Scratch-repo test of scripts/land.sh: what it lands, and what it refuses.
# Run: bash scripts/tests/land_test.sh
set -uo pipefail

LAND=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/land.sh
ROOT=$(mktemp -d -p "${LAND_TEST_TMP:-${TMPDIR:?set TMPDIR or LAND_TEST_TMP}}" land-test.XXXXXX) || exit 1
[[ $ROOT == */land-test.* ]] || exit 1
chmod g+rwx "$ROOT"
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
    printf 'fix: change a\n\nFork-issue: minyek/cosmic-files#1\nAssisted-by: Claude\n' >"$dir/code-msg"
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
#    command names that old master as the base.
new_repo synced
echo two >"$ROOT/synced/wt/src/a.rs"
git -C "$ROOT/synced/wt" commit -q -am "wip"
old_master=$(git -C "$ROOT/synced/main" rev-parse master)
git -C "$ROOT/synced/main" tag backup/main-pre-sync-20260101T000000Z "$old_master"
git -C "$ROOT/synced/main" commit -q --amend -m "base, rebased by a sync"
land synced
[ "$STATUS" -ne 0 ] && grep -qF "git rebase --autosquash --onto master $old_master" <<<"$OUT"
check "the restack command names the old master a backup tag keeps" $? "status=$STATUS: $OUT"

# add_remote <name> <remote>: a bare repository holding the main checkout's master, added as <remote>.
add_remote() {
    local dir=$ROOT/$1
    git init -q --bare "$dir/$2.git"
    git -C "$dir/main" push -q "$dir/$2.git" master
    git -C "$dir/main" remote add "$2" "$dir/$2.git"
}

# commit_change <name>: one commit on the branch, which leaves it linear and ready to land; a push case
# adds its remotes first.
commit_change() {
    echo two >"$ROOT/$1/wt/src/a.rs"
    git -C "$ROOT/$1/wt" commit -q -am "wip"
}

# 7. --push with an origin but no upstream remote lands locally and leaves origin's master as it was.
new_repo noupstream
add_remote noupstream origin
commit_change noupstream
before=$(git -C "$ROOT/noupstream/origin.git" rev-parse master)
land noupstream --push
[ "$STATUS" -eq 0 ] && grep -q 'not pushed' <<<"$OUT" && [ "$(git -C "$ROOT/noupstream/origin.git" rev-parse master)" = "$before" ]
check "--push without an upstream remote pushes nothing" $? "status=$STATUS: $OUT"

# 8. --push with an upstream but no origin remote lands locally and pushes nothing.
new_repo noorigin
add_remote noorigin upstream
commit_change noorigin
before=$(git -C "$ROOT/noorigin/upstream.git" rev-parse master)
land noorigin --push
[ "$STATUS" -eq 0 ] && grep -q 'not pushed' <<<"$OUT" && [ "$(git -C "$ROOT/noorigin/upstream.git" rev-parse master)" = "$before" ]
check "--push without an origin remote pushes nothing" $? "status=$STATUS: $OUT"

# 9. --push where origin is the upstream itself never pushes to it.
new_repo same
add_remote same origin
git -C "$ROOT/same/main" remote add upstream "$ROOT/same/origin.git"
commit_change same
before=$(git -C "$ROOT/same/origin.git" rev-parse master)
land same --push
[ "$STATUS" -eq 0 ] && grep -q 'not pushed' <<<"$OUT" && [ "$(git -C "$ROOT/same/origin.git" rev-parse master)" = "$before" ]
check "--push where origin is the upstream pushes nothing" $? "status=$STATUS: $OUT"

# 10. --push with a separate upstream pushes master to origin, and only there.
new_repo pushed
add_remote pushed origin
add_remote pushed upstream
commit_change pushed
before=$(git -C "$ROOT/pushed/upstream.git" rev-parse master)
land pushed --push
[ "$STATUS" -eq 0 ] && [ "$(git -C "$ROOT/pushed/origin.git" rev-parse master)" = "$(git -C "$ROOT/pushed/main" rev-parse master)" ] && [ "$(git -C "$ROOT/pushed/upstream.git" rev-parse master)" = "$before" ]
check "--push with a separate upstream pushes master to origin only" $? "status=$STATUS: $OUT"

# 11. A branch that merged a side branch of its own is told to restack onto master from its own base,
#     the master commit it was cut from.
new_repo sidemerge
echo two >"$ROOT/sidemerge/main/src/a.rs"
git -C "$ROOT/sidemerge/main" commit -q -am "master moves before the branch is cut"
git -C "$ROOT/sidemerge/wt" reset -q --hard master
base=$(git -C "$ROOT/sidemerge/main" rev-parse master)
git -C "$ROOT/sidemerge/wt" switch -q -c side
echo side >"$ROOT/sidemerge/wt/src/b.rs"
git -C "$ROOT/sidemerge/wt" add src/b.rs
git -C "$ROOT/sidemerge/wt" commit -q -m "side"
git -C "$ROOT/sidemerge/wt" switch -q feat
echo three >"$ROOT/sidemerge/wt/src/a.rs"
git -C "$ROOT/sidemerge/wt" commit -q -am "wip"
git -C "$ROOT/sidemerge/wt" merge -q --no-ff -m "merge side" side
land sidemerge
[ "$STATUS" -ne 0 ] && grep -qF "git rebase --autosquash --onto master $base" <<<"$OUT"
check "a merged side branch is restacked from the branch's own base" $? "status=$STATUS: $OUT"

# 12. A code message with a Signed-off-by line, or without the Assisted-by trailer, is refused.
new_repo signed
commit_change signed
printf 'fix: change a\n\nFork-issue: minyek/cosmic-files#1\nAssisted-by: Claude\nSigned-off-by: t <t@example.invalid>\n' >"$ROOT/signed/code-msg"
before=$(git -C "$ROOT/signed/main" rev-parse master)
land signed
[ "$STATUS" -ne 0 ] && grep -q 'Signed-off-by' <<<"$OUT" && [ "$(git -C "$ROOT/signed/main" rev-parse master)" = "$before" ]
check "a code message with Signed-off-by is refused" $? "status=$STATUS: $OUT"

new_repo unassisted
commit_change unassisted
printf 'fix: change a\n\nFork-issue: minyek/cosmic-files#1\n' >"$ROOT/unassisted/code-msg"
before=$(git -C "$ROOT/unassisted/main" rev-parse master)
land unassisted
[ "$STATUS" -ne 0 ] && grep -q 'Assisted-by' <<<"$OUT" && [ "$(git -C "$ROOT/unassisted/main" rev-parse master)" = "$before" ]
check "a code message without Assisted-by is refused" $? "status=$STATUS: $OUT"

# 13. A rejected push exits non-zero and says so: master has landed locally, and origin's master,
#     which another clone moved, stays where that clone left it.
new_repo rejected
add_remote rejected origin
add_remote rejected upstream
other=$(git -C "$ROOT/rejected/origin.git" commit-tree -p master -m "landed in another clone" 'master^{tree}')
git -C "$ROOT/rejected/origin.git" update-ref refs/heads/master "$other"
commit_change rejected
land rejected --push
[ "$STATUS" -ne 0 ] && grep -q 'push failed' <<<"$OUT" \
    && [ "$(git -C "$ROOT/rejected/origin.git" rev-parse master)" = "$other" ] \
    && [ "$(git -C "$ROOT/rejected/main" rev-parse 'master^{tree}')" = "$(git -C "$ROOT/rejected/wt" rev-parse 'HEAD^{tree}')" ]
check "a rejected push exits non-zero and leaves origin's master to the other clone" $? "status=$STATUS: $OUT"

exit "$FAILED"
