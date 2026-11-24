#!/usr/bin/env bash
# Fails an implementation PR that adds, changes or deletes tests.
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
fail=0

head=$(git rev-parse "$HEAD_REF") || exit 2
changed=$(git diff --name-only --no-renames "$BASE...$head")
tests=() other=()
while IFS= read -r f; do
  [ -z "$f" ] && continue
  if is_test "$f"; then tests+=("$f"); else other+=("$f"); fi
done <<<"$changed"

[ ${#tests[@]} -eq 0 ] && { echo "ok: no test files changed"; exit 0; }
[ ${#other[@]} -eq 0 ] && { echo "ok: only test files changed (test agent PR)"; exit 0; }

msgs=()
say() { msgs+=("$*"); fail=1; }
listed="${tests[*]}"

sha=$(printf '%s\n' "$BODY" | tr -d '\r' | sed -n 's/^Tests-From: \([0-9a-f]\{40\}\)[[:space:]]*$/\1/p' | head -n1)
if [ -z "$sha" ]; then
  say "Add \`Tests-From: <sha of the tests/<ticket> head>\` to the PR body."
else
  ok=1
  if ! git cat-file -e "$sha^{commit}" 2>/dev/null || ! git merge-base --is-ancestor "$sha" "$head"; then
    say "Tests-From $sha is not in this PR's history. Rebase onto the tests/<ticket> branch."; ok=0
  fi
  if ! git ls-remote --heads "$REMOTE" 'refs/heads/tests/*' | cut -f1 | grep -qx "$sha"; then
    say "Tests-From $sha is not the head of a tests/* branch on $REMOTE. Use the current head of tests/<ticket>, then rebase."; ok=0
  fi
  if [ $ok = 1 ]; then
    snap() { git ls-tree -r "$1" | while IFS=$'\t' read -r meta path; do is_test "$path" && echo "$meta	$path"; done | sort -k2 -t$'\t'; }
    if [ "$(snap "$sha")" != "$(snap "$head")" ]; then
      diffs=$(diff <(snap "$sha") <(snap "$head") | sed -n 's/^[<>].*\t//p' | sort -u | tr '\n' ' ')
      listed=$diffs
      say "This PR changes tests (${diffs% }). Tests belong to the test agent: ask for the change on tests/<ticket>, then rebase."
    fi
    # no unit tests sneaked into non-test files
    sneak=$(git diff -U0 --no-renames "$sha" "$head" | awk '
      /^\+\+\+ /{f=substr($2,3); next}
      /^\+/ && f!="" && f !~ /^(tests\/|rust\/tests\/|rust\/crates\/[^\/]+\/tests\/)/ && f !~ /\.proptest-regressions$/ \
        && /#\[test\]|#\[tokio::test\]|proptest!|#\[cfg\(test\)\]/ {print f}' | sort -u | tr '\n' ' ')
    [ -n "$sneak" ] && say "This PR adds tests inside source files (${sneak% }). Tests belong to the test agent: ask for them on tests/<ticket>, then rebase."
  fi
fi
if [ $fail = 0 ]; then echo "ok: tests match $sha"; else
  [ -z "$sha" ] && echo "This PR changes tests (${tests[*]}). Tests belong to the test agent: ask for the change on tests/<ticket>, then rebase."
  printf '%s\n' "${msgs[@]}"
fi
exit $fail
