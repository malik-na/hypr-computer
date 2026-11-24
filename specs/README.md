# Specs

Formal models of the stateful parts of the Rust rewrite, written in TLA+ and checked by TLC on every pull request.

A spec describes what a part may do, step by step, and the properties that must hold whatever the order of events. TLC tries every order. When a property can break, it prints the shortest run of events it found that breaks it.

## The rule

**Every stateful part gets a spec here before its code is written.** The spec must come with at least one buggy variant taken from a real bug, and TLC must fail on that variant: a spec that has never caught a bug proves nothing.

For a new part:

1. Write `specs/<Part>.tla` and `specs/<Part>.cfg`. Name the actions the way the Rust events will be named. Keep the constants small enough for TLC to finish in seconds.
2. Add a switch (a `BOOLEAN` constant, `FALSE` in the correct design) for each real bug, used in one place in the spec, next to a comment that says what the bug was.
3. Add `specs/<Bug>.cfg` for each switch: the correct config with only that switch set to `TRUE`. It may use fewer sessions, which keeps the counterexample short. `check.sh` refuses any other difference, so a bug is always checked against the same properties as the correct design.
4. Start each `<Bug>.cfg` with a header comment naming what it must violate: `\* check: <Spec> invariant <Property>`, or `\* check: <Spec> liveness <Property>`. `check.sh` finds every check from these headers, so there is no list to edit. A config without a valid header, or naming a spec or property that doesn't exist, fails it.
5. Write the part's notes in `specs/<Part>.md`, next to the spec: what it covers, what it leaves out, and the Rust names. `check.sh` fails if it is missing. Each spec's notes live in its own file; this README doesn't list them.

## Run it

```bash
specs/check.sh
```

It needs Java 11 or newer (`JAVA=/path/to/java` to pick one). It downloads `tla2tools.jar` 1.7.4 once into `specs/.tools/` and checks its SHA-256. It exits non-zero if a correct spec fails, or if a buggy variant passes or fails on a different property than the one it names, or if a `.cfg` or `.md` is missing or malformed.
