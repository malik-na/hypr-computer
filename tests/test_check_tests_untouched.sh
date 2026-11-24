#!/usr/bin/env bash
# Runs scripts/check-tests-untouched.sh on throwaway repos. Usage: tests/test_check_tests_untouched.sh
set -u
SCRIPT=$(cd "$(dirname "$0")/.." && pwd)/scripts/check-tests-untouched.sh
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
pass=0 failn=0

# setup: bare origin + work clone with main containing src and a test; branch tests/x pushed (test agent head).
setup() {
  rm -rf "$T/o" "$T/w"; git init -q --bare "$T/o"; git init -q -b main "$T/w"; cd "$T/w"
  git remote add origin "$T/o"
  mkdir -p rust/tests rust/crates/a/src; echo 'fn a(){}' >rust/crates/a/src/lib.rs; echo 'old' >rust/tests/old.rs
  git add -A; git commit -qm base; git push -q origin main; BASE=$(git rev-parse HEAD)
  git checkout -q -b tests/x; echo 'new' >rust/tests/screen.rs; git add -A; git commit -qm tests
  git push -q origin tests/x; TESTS=$(git rev-parse HEAD)
}
impl() { git checkout -q -b impl "$TESTS"; }
commit() { git add -A; git commit -qm c; }
run() { # name expect(0|1) body [grep]
  local out rc; out=$(cd "$T/w" && BASE=$BASE HEAD=HEAD REMOTE=origin PR_BODY="$3" bash "$SCRIPT" 2>&1); rc=$?
  local want=$2; [ "$want" != 0 ] && want=1; [ $rc != 0 ] && rc=1
  if [ $rc = $want ] && { [ -z "${4:-}" ] || grep -qF -- "$4" <<<"$out"; }; then pass=$((pass+1)); echo "PASS $1"; else failn=$((failn+1)); echo "FAIL $1 (rc=$rc)"; fi
  sed 's/^/     | /' <<<"$out"
}

setup; git checkout -q -b tests/y "$BASE"; echo t >rust/tests/z.rs; commit
run "tests-only change passes" 0 ""

setup; git checkout -q -b impl "$BASE"; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; commit
run "code-only change passes" 0 ""

setup; impl; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; commit
run "code+tests on the tests/* head passes, naming the branch" 0 "closes #1" "ok: tests match tests/x"

setup; impl; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; commit
run "a Tests-From line is not required, and a correct one is fine" 0 "Tests-From: $TESTS" "tests/x"

setup; git checkout -q -b impl "$BASE"; git cherry-pick "$TESTS" >/dev/null; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; commit
run "cherry-picked tests commit (new sha, same content) passes" 0 "" "ok: tests match tests/x"

setup; impl; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; commit
run "stale Tests-From with matching content passes" 0 "Tests-From: 932e445000000000000000000000000000000000" "is ignored"

# tests/x is rebased (same test content, new sha): the PR's old base of it still matches by content.
setup; impl; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; commit
git checkout -q tests/x; git commit -q --amend -m rebased; git push -qf origin tests/x; git checkout -q impl
run "tests/* head rewritten with the same content passes" 0 "Tests-From: $TESTS" "ok: tests match tests/x"

setup; impl; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; echo changed >rust/tests/screen.rs; commit
run "changed test file with no matching tests/* head fails" 1 "Tests-From: $TESTS" "ask for the change on tests/x"

setup; impl; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; echo changed >rust/tests/screen.rs; commit
run "changed test file is listed" 1 "" "rust/tests/screen.rs"

setup; impl; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; git rm -q rust/tests/old.rs; commit
run "deleted test fails" 1 "" "rust/tests/old.rs"

setup; impl; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; echo extra >rust/tests/extra.rs; commit
run "added test file fails" 1 "" "rust/tests/extra.rs"

# closest head: tests/far differs in two files, tests/x in one, so tests/x is named.
setup; git checkout -q -b tests/far "$BASE"; echo a >rust/tests/a.rs; echo b >rust/tests/b.rs; commit; git push -q origin tests/far
impl; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; echo changed >rust/tests/screen.rs; commit
run "failure names the closest tests/* head" 1 "" "closest is tests/x"

setup; impl; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; echo changed >rust/tests/screen.rs; commit
git push -q origin --delete tests/x
run "changed tests with no tests/* branch at all fails" 1 "" "no tests/* branch"

setup; impl; printf '#[test]\nfn sneaky(){}\n' >>rust/crates/a/src/lib.rs; commit
run "#[test] added in src fails" 1 "Tests-From: $TESTS" "rust/crates/a/src/lib.rs"

setup; impl; printf '#[cfg(test)]\nmod t {}\n' >>rust/crates/a/src/lib.rs; commit
run "#[cfg(test)] sneaked into src fails" 1 "" "adds tests inside source files"

setup; impl; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; mkdir -p scripts; echo 'exit 0' >scripts/check-tests-untouched.sh; commit
run "editing the check itself fails" 1 "Tests-From: $TESTS" "scripts/check-tests-untouched.sh"

echo; echo "$pass passed, $failn failed"; [ $failn = 0 ]
