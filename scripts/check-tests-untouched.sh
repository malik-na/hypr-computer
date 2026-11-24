#!/usr/bin/env bash
# Fails an implementation PR that adds, changes or deletes tests.
# A PR that changes test files and other files passes iff every test file at its head is exactly as at the head of
# some tests/* branch on REMOTE (same paths, modes and blobs, so cherry-picks and rebases are fine), and it adds no
# unit tests inside source files. No SHA in the PR body is needed; a `Tests-From:` line is only reported on.
# Env: BASE (base commit/ref), HEAD (PR head, default HEAD), PR_BODY (untrusted text, never eval'd),
#      REMOTE (default origin). Run from inside the repo; also usable locally.
set -u
BASE=${BASE:?set BASE to the PR base commit}
HEAD_REF=${HEAD:-HEAD}
REMOTE=${REMOTE:-origin}
BODY=${PR_BODY:-}

# Protected files: the tests (tests/, rust/tests/, rust/crates/*/tests/, *.proptest-regressions) and this
# check itself (its script and workflow), so an implementation PR can't switch it off. The check's own self-test
# (tests/test_check_tests_untouched.sh) is protected like any test.
is_test() {
  [[ $1 =~ ^(tests/|rust/tests/|rust/crates/[^/]+/tests/) || $1 =~ \.proptest-regressions$ ]] && return 0
  [ "$1" = scripts/check-tests-untouched.sh ] || [ "$1" = .github/workflows/tests-untouched.yml ]
}
# Every protected path in a commit, with its mode and blob.
snap() { git ls-tree -r "$1" | while IFS=$'\t' read -r meta path; do is_test "$path" && echo "$meta	$path"; done | sort -t$'\t' -k2; }
# Paths whose entry differs between two snapshots (changed, added or deleted).
differ() { diff <(printf '%s\n' "$1") <(printf '%s\n' "$2") | sed -n 's/^[<>].*\t//p' | sort -u; }

head=$(git rev-parse "$HEAD_REF") || exit 2
base=$(git rev-parse "$BASE") || exit 2
changed=$(git diff --name-only --no-renames "$base...$head")
tests=() other=()
while IFS= read -r f; do
  [ -z "$f" ] && continue
  if is_test "$f"; then tests+=("$f"); else other+=("$f"); fi
done <<<"$changed"

[ ${#tests[@]} -eq 0 ] && { echo "ok: no test files changed"; exit 0; }
[ ${#other[@]} -eq 0 ] && { echo "ok: only test files changed (test agent PR)"; exit 0; }

fail=0

# No unit tests sneaked into non-test files (lines this PR adds relative to its base).
sneak=$(git diff -U0 --no-renames "$base...$head" | awk '
  /^\+\+\+ /{f=substr($2,3); next}
  /^\+/ && f!="" && f !~ /^(tests\/|rust\/tests\/|rust\/crates\/[^\/]+\/tests\/)/ && f !~ /\.proptest-regressions$/ \
    && /#\[test\]|#\[tokio::test\]|proptest!|#\[cfg\(test\)\]/ {print f}' | sort -u | tr '\n' ' ')
if [ -n "$sneak" ]; then
  echo "This PR adds tests inside source files (${sneak% }). Tests belong to the test agent: ask for them on tests/<ticket>."
  fail=1
fi

# The test files must be exactly those at the head of some tests/* branch.
heads=$(git ls-remote --heads "$REMOTE" 'refs/heads/tests/*') || { echo "cannot list tests/* branches on $REMOTE"; exit 2; }
if [ -z "$heads" ]; then
  echo "This PR changes tests (${tests[*]}) and $REMOTE has no tests/* branch. Tests belong to the test agent: ask for the change on tests/<ticket>."
  exit 1
fi
while read -r sha _; do
  git cat-file -e "$sha^{commit}" 2>/dev/null && continue
  git fetch -q --no-tags "$REMOTE" "+refs/heads/tests/*:refs/remotes/$REMOTE/tests/*" || { echo "cannot fetch tests/* from $REMOTE"; exit 2; }
  break
done <<<"$heads"

mine=$(snap "$head")
match="" best="" bestn=-1 bestdiff=""
while read -r sha ref; do
  branch=${ref#refs/heads/}
  theirs=$(snap "$sha") || continue
  if [ "$theirs" = "$mine" ]; then match="$branch ($sha)"; break; fi
  d=$(differ "$theirs" "$mine"); n=$(grep -c . <<<"$d")
  if [ $bestn -lt 0 ] || [ "$n" -lt $bestn ]; then bestn=$n best="$branch ($sha)" bestdiff=$d; fi
done <<<"$heads"

if [ -z "$match" ]; then
  ticket=${best%% *}
  echo "This PR changes tests. Its test files match no tests/* head; the closest is $best, which differs in:"
  sed 's/^/  /' <<<"$bestdiff"
  echo "Tests belong to the test agent: ask for the change on $ticket."
  fail=1
fi

claimed=$(printf '%s\n' "$BODY" | tr -d '\r' | sed -n 's/^Tests-From: *\([0-9a-f]\{7,40\}\).*$/\1/p' | head -n1)
if [ -n "$claimed" ] && [ -n "$match" ] && [[ $match != *"($claimed"* ]]; then
  echo "note: the PR body's Tests-From $claimed is ignored; the tests match $match."
fi
[ $fail = 0 ] && echo "ok: tests match $match"
exit $fail
