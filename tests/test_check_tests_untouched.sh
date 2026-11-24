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
run "code+tests with correct Tests-From passes" 0 "closes #1
Tests-From: $TESTS"

setup; impl; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; echo changed >rust/tests/screen.rs; commit
run "changed test file fails" 1 "Tests-From: $TESTS" "rust/tests/screen.rs"

setup; impl; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; git rm -q rust/tests/old.rs; commit
run "deleted test fails" 1 "Tests-From: $TESTS" "rust/tests/old.rs"

setup; impl; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; echo extra >rust/tests/extra.rs; commit
run "added test file fails" 1 "Tests-From: $TESTS" "rust/tests/extra.rs"

setup; impl; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; commit
run "missing Tests-From fails" 1 "no line here" "Add \`Tests-From:"

setup; git checkout -q -b impl "$TESTS"; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; commit; git push -q origin impl:wip/notests
run "Tests-From not a tests/* head fails" 1 "Tests-From: $(git rev-parse HEAD)" "not the head of a tests/* branch"

setup; impl; git checkout -q -b tmp; echo 'fn b(){}' >>rust/crates/a/src/lib.rs; commit
git checkout -q -B impl tmp; git branch -q -D tmp
# stale: tests/x moves on, so the old sha is no longer a head
git checkout -q tests/x; echo more >rust/tests/more.rs; commit; git push -q origin tests/x; git checkout -q impl
run "Tests-From stale (not current head) fails" 1 "Tests-From: $TESTS" "not the head of a tests/* branch"

setup; impl; printf '#[test]\nfn sneaky(){}\n' >>rust/crates/a/src/lib.rs; commit
run "#[test] added in src fails" 1 "Tests-From: $TESTS" "rust/crates/a/src/lib.rs"

echo; echo "$pass passed, $failn failed"; [ $failn = 0 ]
