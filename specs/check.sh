#!/usr/bin/env bash
# Model-check the specs in this folder with TLC.
#
# The correct config of each spec must pass. Each buggy config must fail, and with the property it names:
# a buggy variant that passes means the spec no longer catches that bug, and that fails this script too.
#
#   specs/check.sh            needs java 11 or newer on PATH, or JAVA=/path/to/java
#
# The checks themselves are the list at the bottom of this file.
set -euo pipefail
cd "$(dirname "$0")"

TLA_VERSION=1.7.4
TLA_SHA256=936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88
TLA_URL="https://github.com/tlaplus/tlaplus/releases/download/v$TLA_VERSION/tla2tools.jar"
JAR=".tools/tla2tools-$TLA_VERSION.jar"
JAVA="${JAVA:-java}"

die() { echo "check.sh: $*" >&2; exit 2; }

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
    else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

"$JAVA" -version >/dev/null 2>&1 || die "java doesn't run. Install a JDK (11 or newer), or set JAVA=/path/to/java"

if [ ! -f "$JAR" ] || [ "$(sha256 "$JAR")" != "$TLA_SHA256" ]; then
    echo "Downloading tla2tools.jar $TLA_VERSION"
    mkdir -p .tools
    curl -fsSL --retry 3 -o "$JAR.part" "$TLA_URL" || die "couldn't download $TLA_URL"
    got=$(sha256 "$JAR.part")
    if [ "$got" != "$TLA_SHA256" ]; then
        rm -f "$JAR.part"
        die "tla2tools.jar has SHA-256 $got, expected $TLA_SHA256. Not using it"
    fi
    mv "$JAR.part" "$JAR"
fi
echo "tla2tools.jar $TLA_VERSION, SHA-256 verified"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
failures=0
summary=""
runs=0

# tlc SPEC CFG: runs TLC, leaves its output in $log and its exit code in $code.
# TLC exits 0 for no error, 12 for a violated invariant and 13 for a violated temporal property.
# One worker and a fixed fingerprint function, so that every run prints the same counterexample.
tlc() {
    runs=$((runs + 1))
    log="$WORK/$runs.log"
    code=0
    "$JAVA" -XX:+UseParallelGC -cp "$JAR" tlc2.TLC -workers 1 -fp 0 -metadir "$WORK/$runs.states" \
        -config "$2" "$1.tla" >"$log" 2>&1 || code=$?
}

# The counterexample, without the source positions TLC adds to every step.
trace() {
    sed -n '/^Error: /,$p' "$log" |
        sed -E 's/ line [0-9]+, col [0-9]+ to line [0-9]+, col [0-9]+ of module [A-Za-z0-9_]+>/>/' |
        grep -v -e '^Finished ' -e '^The depth ' -e '^The average ' -e 'states generated' | sed 's/^/    /'
}

result() {      # result PASS|FAIL NAME TEXT
    [ "$1" = PASS ] || failures=$((failures + 1))
    line=$(printf '%-4s  %-30s %s' "$1" "$2" "$3")
    summary="$summary$line"$'\n'
    echo "$line"
    if [ "$1" = FAIL ]; then echo "--- full TLC output:"; cat "$log"; fi
}

heading() { echo; echo "=== $1"; }

# A buggy config must be the correct config with exactly one switch set to TRUE, so that it is checked against
# the same properties. Only its comment and its number of sessions may differ.
same_but_for() {    # same_but_for CORRECT_CFG BUGGY_CFG SWITCH
    grep -q "^ *$3 = FALSE\$" "$1" || return 1
    diff <(grep -v -e '^\\\*' -e '^ *Sessions = ' "$1" | sed "s/^\( *$3\) = FALSE\$/\1 = TRUE/") \
         <(grep -v -e '^\\\*' -e '^ *Sessions = ' "$2") >/dev/null
}

# expect_pass SPEC: SPEC.cfg must pass.
expect_pass() {
    heading "$1: the correct design. TLC must find no error"
    tlc "$1" "$1.cfg"
    if [ "$code" = 0 ] && grep -q '^Model checking completed. No error has been found.' "$log"; then
        grep -E 'states generated, .* distinct states found, 0 states left|^Finished in ' "$log" | sed 's/^/    /'
        states=$(sed -n 's/^[0-9]* states generated, \([0-9]*\) distinct states found, 0 states left.*/\1/p' "$log")
        result PASS "$1" "no error, $states distinct states"
    else
        result FAIL "$1" "TLC reported an error (exit code $code)"
    fi
}

# expect_invariant SPEC BUG PROPERTY: BUG.cfg must violate the invariant PROPERTY.
expect_invariant() {
    heading "$2: a past bug. TLC must report that $3 is violated"
    if ! same_but_for "$1.cfg" "$2.cfg" "$2"; then
        log=/dev/null; result FAIL "$2" "$2.cfg must be $1.cfg with only $2 = TRUE"; return
    fi
    tlc "$1" "$2.cfg"
    if [ "$code" = 12 ] && grep -q "^Error: Invariant $3 is violated." "$log"; then
        trace
        result PASS "$2" "violates $3, as it must"
    else
        result FAIL "$2" "TLC did not report $3 (exit code $code)"
    fi
}

# expect_liveness SPEC BUG PROPERTY: BUG.cfg must violate the liveness property PROPERTY.
# TLC doesn't say which temporal property failed, so this also checks that BUG.cfg passes without PROPERTY.
expect_liveness() {
    heading "$2: a past bug. TLC must report that $3 is violated"
    if ! same_but_for "$1.cfg" "$2.cfg" "$2"; then
        log=/dev/null; result FAIL "$2" "$2.cfg must be $1.cfg with only $2 = TRUE"; return
    fi
    grep -v "^ *$3\$" "$2.cfg" >"$WORK/without.cfg"
    if [ "$(wc -l <"$WORK/without.cfg")" -ne $(($(wc -l <"$2.cfg") - 1)) ]; then
        log=/dev/null; result FAIL "$2" "$2.cfg doesn't check $3"; return
    fi
    tlc "$1" "$WORK/without.cfg"
    if [ "$code" != 0 ]; then
        result FAIL "$2" "fails even without $3 (exit code $code): another property is violated"; return
    fi
    tlc "$1" "$2.cfg"
    if [ "$code" = 13 ] && grep -q '^Error: Temporal properties were violated.' "$log"; then
        trace
        echo "    (every other property holds: without $3 this config passes)"
        result PASS "$2" "violates $3, as it must"
    else
        result FAIL "$2" "TLC did not report $3 (exit code $code)"
    fi
}

# ---- The checks. Add one line per config when you add a spec or a buggy variant.
expect_pass      SessionLifecycle
expect_invariant SessionLifecycle HandOffEarly                 ToolCallOnlyOnReadyScreen
expect_liveness  SessionLifecycle CleanupDependsOnDyingProcess ResourcesEventuallyReleased
expect_pass      Stop
expect_invariant Stop             CheckStopOnlyAtStart         NoCallServedWhileStopped

heading "Summary"
printf '%s' "$summary"
if [ "$failures" -gt 0 ]; then
    echo "$failures check(s) failed"
    exit 1
fi
echo "All checks passed"
