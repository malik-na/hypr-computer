# Specs

Formal models of the stateful parts of the Rust rewrite, written in TLA+ and checked by TLC on every pull request.

A spec describes what a part may do, step by step, and the properties that must hold whatever the order of events. TLC tries every order. When a property can break, it prints the shortest run of events it found that breaks it.

## The rule

**Every stateful part gets a spec here before its code is written.** The spec must come with at least one buggy variant taken from a real bug, and TLC must fail on that variant: a spec that has never caught a bug proves nothing.

For a new part:

1. Write `specs/<Part>.tla` and `specs/<Part>.cfg`. Name the actions the way the Rust events will be named. Keep the constants small enough for TLC to finish in seconds.
2. Add a switch (a `BOOLEAN` constant, `FALSE` in the correct design) for each real bug, used in one place in the spec, next to a comment that says what the bug was.
3. Add `specs/<Bug>.cfg` for each switch: the correct config with only that switch set to `TRUE`. It may use fewer sessions, which keeps the counterexample short. `check.sh` refuses any other difference, so a bug is always checked against the same properties as the correct design.
4. Add one line per config to the list at the bottom of `check.sh`, naming the property each bug must violate.
5. Add the part to this README: what it covers, what it leaves out, and the Rust names.

## Run it

```bash
specs/check.sh
```

It needs Java 11 or newer (`JAVA=/path/to/java` to pick one). It downloads `tla2tools.jar` 1.7.4 once into `specs/.tools/` and checks its SHA-256. It exits non-zero if a correct spec fails, or if a buggy variant passes or fails on a different property than the one it names.

## SessionLifecycle

`SessionLifecycle.tla` models one daemon and its sessions. An agent connects, the daemon starts a screen process for it, verifies that the screen is ready, serves tool calls on it, and tears the session down when the agent's connection closes or the screen fails or crashes. The agent can go away at any moment, including while the screen is still starting.

The model keeps apart what the daemon believes (`session`) and what is true in the world (`conn`, `screen`, `apps`). Both past bugs lived in the gap between the two.

### Properties

| Property | In plain words |
| --- | --- |
| `ToolCallOnlyOnReadyScreen` | A tool call is served only on a screen that is verified ready |
| `NoScreenWithoutSession` | No screen process and no app exists for a session that is gone |
| `NoSessionWithoutAgent` | A session whose agent is gone is being torn down, or is gone too |
| `TeardownAtMostOnce` | Teardown is started at most once per session |
| `DaemonNeverStuck` | Every session always has a next step, and a teardown never waits for the agent |
| `SessionsAreIndependent` | A step of one session, teardown included, never changes another session |
| `ResourcesEventuallyReleased` | Every session that was disconnected or has failed gets its screen and apps removed |
| `StartingSessionBecomesReady` | A session whose agent stays connected, and whose screen neither fails nor crashes, becomes ready |

The last two say that something happens in the end. They hold only under the assumptions in `Fairness`: a screen that is spawning comes up, a screen that is up settles, the daemon's readiness check notices it, and the daemon carries out every teardown step. Nothing is assumed about agents: they may connect, call and disconnect at any time, or never.

### The two bugs it catches

| Config | The real bug | Property it violates |
| --- | --- | --- |
| `HandOffEarly.cfg` | The prototype handed the screen to the agent as soon as its process answered. Focus could still sit on an output with no workspace, and an app launched then never got a window. | `ToolCallOnlyOnReadyScreen` |
| `CleanupDependsOnDyingProcess.cfg` | The Python server removed its display in its own signal handler. When the handler didn't run, the display was left behind. | `ResourcesEventuallyReleased` |

The second switch changes only `Fairness`. That is how TLA+ says who does the cleanup: the daemon kills the process groups itself, so the cleanup is guaranteed. A process that is dying guarantees nothing.

### Reading a counterexample

`check.sh` prints the run that TLC found. For `HandOffEarly`, shortened here to the variables that matter:

```
Error: Invariant ToolCallOnlyOnReadyScreen is violated.
State 2: <Connect>
/\ session = (s1 :> "Starting")      /\ screen = (s1 :> "Spawning")
State 3: <ScreenStarted>
/\ session = (s1 :> "Ready")         /\ screen = (s1 :> "Unsettled")
State 4: <ToolCall>
/\ session = (s1 :> "Ready")         /\ screen = (s1 :> "Unsettled")     /\ apps = (s1 :> "NoWindow")
```

- Each `State` is the world after one step. The name in `< >` is the action that led to it.
- `s1 :> "Ready"` means: for session `s1`, the value is `"Ready"`.
- Read down one variable at a time and look for the step where belief and world part. Here it is state 3: the daemon says `Ready` while the screen is still `Unsettled`.
- A liveness counterexample ends with `Stuttering` (from here on, nothing more has to happen) or `Back to state N` (this loop can repeat for ever). The last state shows what is never cleaned up. For `CleanupDependsOnDyingProcess` it is `session = "TearingDown"` with `screen = "Settled"`: the agent is gone, and its screen stays.
- TLC doesn't name a violated liveness property. `check.sh` proves which one it is by checking that the same config passes without it.

### Rust names

The Rust state machine is one pure function, `domain::lifecycle::step(state, event) -> (state, Option<Effect>)`: each action emits at most one effect. It holds only `session`. The other variables are the world, as the adapters see it.

| `session` in the spec | Rust |
| --- | --- |
| `"Absent"` | `SessionState::Absent`: the state a new session id starts in. A session that reaches it again is dropped; ids are never reused |
| `"Starting"` | `SessionState::Starting` |
| `"Verifying"` | `SessionState::Verifying` |
| `"Ready"` | `SessionState::Ready` |
| `"TearingDown"` | `SessionState::TearingDown` |

| Action in the spec | Rust event | State change | Effects |
| --- | --- | --- | --- |
| `Connect` | `Event::Connect` | none → `Starting` | `Effect::SpawnScreen` |
| `ScreenStarted` | `Event::ScreenStarted` | `Starting` → `Verifying`. `TearingDown` stays | `Effect::VerifyScreen`. None in `TearingDown` |
| `ScreenSettles` | none: the world | | |
| `ScreenReady` | `Event::ScreenReady`, sent only after the readiness check passed | `Verifying` → `Ready` | none |
| `ScreenFailed` | `Event::ScreenFailed`: no start, or no pass, in time | `Starting`, `Verifying` → `TearingDown` | `Effect::Teardown` |
| `ScreenCrashed` | `Event::ScreenCrashed`: the process exited on its own | `Starting`, `Verifying`, `Ready` → `TearingDown` | `Effect::Teardown` |
| `ToolCall` | `Event::ToolCall` | `Ready` stays | `Effect::ServeToolCall`. In any other state, the state stays and the effect is `Effect::RejectToolCall` |
| `Disconnect` | `Event::Disconnect` | `Starting`, `Verifying`, `Ready` → `TearingDown`. `TearingDown` stays | `Effect::Teardown`. None in `TearingDown` |
| `ScreenStopped`, `AppsStopped` | none: the world. This is what `Effect::Teardown` does | | |
| `TeardownDone` | `Event::TeardownDone`, sent only after the screen process and the apps are seen to be gone | `TearingDown` → `Absent` | `Effect::CloseConnection`, harmless if the agent already disconnected |

- `teardowns` counts `Effect::Teardown` per session.
- An event that the spec doesn't allow in a state changes nothing and emits no effect, except a reply to the caller. Examples: `ToolCall` before `Ready` (`Effect::RejectToolCall`), `ScreenCrashed` during `TearingDown`, any event for a session that is gone.
- `Effect::Teardown` must not depend on the screen process or the agent doing anything. That is the `CleanupDependsOnDyingProcess` bug.

`rust/crates/domain/tests/lifecycle_spec.rs` property-tests `step` against the invariants above, over random orders of events; it fails on both buggy variants when they are put back into `step`.

### Left out

- **Take-over, stop and the notification.** Each gets its own spec. Take-over and stop attach at `ToolCall`: both add a condition under which a `Ready` session doesn't serve. The notification attaches at `ScreenReady` (it appears) and `TeardownDone` (it goes away).
- **The daemon's own start and exit.** The model has one daemon that is always there.
- **The delay between an event and the daemon seeing it.** A closed connection and an exited process are reported by the kernel, so each is one step here. A tool call that arrives in between fails with an error and isn't served.
- **What a tool call does.** Every call is the worst case for a screen that isn't ready: it launches an app.
- **Restarting a screen.** A screen that fails or crashes ends its session. The agent connects again and gets a new one.
- **Shared names.** Display names, runtime folders and browser profiles are assumed to be unique per session.
- **A limit on sessions.** `Sessions` is the number of slots in the model, not a limit of the daemon.

## Stop

`Stop.tla` models the user stopping agents and letting them go on. `hypr-computer stop` stops every agent, and every agent that connects until `hypr-computer resume`. `hypr-computer stop <agent>` stops one, and `hypr-computer resume <agent>` resumes one. A stopped agent's tool calls are refused, screenshots included, with "Stopped by the user", and its screen stays so the user can look at it.

Tool calls arrive at any time, and a stop can arrive in the middle of one. The dangerous call is one that holds a button or key down across several inputs, like a drag: a press, moves, a release. The model gives each session at most one call at a time, which is admitted, may press, may move up to `Moves` times with the button held, and releases.

The session lifecycle is `SessionLifecycle`'s; here a session is only gone (`"Absent"`) or open with a ready screen (`"Open"`), and the spec attaches at its `ToolCall`.

### Properties

| Property | In plain words |
| --- | --- |
| `NoCallServedWhileStopped` | Once a session's stop has reached the daemon, and until it is resumed, no call of it is let through and its screen gets no input from it but the release of what it holds |
| `StoppedMeansReleased` | When `hypr-computer stop` answers, the session has no call in progress and holds no button or key |
| `OnlyInputHolds` | Only a call that is giving input holds a button or key |
| `StoppedScreenStays` | A session is open exactly while its screen is up: stopping never takes the screen away |
| `RunningSessionServes` | A running session with nothing in progress serves its next call: resume makes it serve again |
| `SessionsAreIndependent` | A step of one session, stopping and resuming it included, never changes another; only stop and resume of all change several |
| `StopAllHoldsForNewAgents` | While `hypr-computer stop` (all) is in force, an agent that connects starts stopped: it can't get out of a stop by reconnecting |
| `OnlyDisconnectEndsScreen` | A screen goes only when its agent's connection closes |
| `StopTakesEffect` | `hypr-computer stop` always answers: a stopping session becomes stopped, unless it is resumed or ends first |
| `StoppedInputIsReleased` | An input in progress ends released when its session is stopped |

The last two hold under the assumptions in `Fairness`: a call in progress ends (the screen finishes it without waiting for the agent), a stopped call releases what it holds, and a stopping session whose call is over becomes stopped. Nothing is assumed about agents and the user.

### The bug it catches

| Config | The real bug | Property it violates |
| --- | --- | --- |
| `CheckStopOnlyAtStart.cfg` | Until v0.6, the Python server's drag checked the stop file once, before pressing. A stop during the drag let it move on to its end with the button held. Fixed by re-checking at every step and releasing in `finally`. | `NoCallServedWhileStopped` |

TLC's counterexample: an agent connects, a call is admitted (`ToolCall`), the user runs `hypr-computer stop` (`StopAll`: `run` becomes `"Stopping"` but `call` stays `"Admitted"`), and the call presses anyway (`Press`: `held` becomes `TRUE` and `served` records it).

### Rust names

The Rust state machine is `domain::stop::Stops<K>`, keyed by session id, with one pure `step(&mut self, Event<K>) -> Vec<Effect<K>>`; the top of `rust/crates/domain/tests/stop.rs` documents it in full. It holds `run` (`Run`), `call` (`Call`) and `stopNew` (`stop_new()`). `session` and `screen` are the lifecycle's, `held` and `moved` are the world; `served` is the model's record of a violation.

| Action in the spec | Rust event | Effect |
| --- | --- | --- |
| `Connect`, `Disconnect` | `Event::Connect`, `Event::Disconnect`, at the lifecycle's: a new session is `Stopped` while stop-all is in force | none |
| `ToolCall` | `Event::ToolCall`, from the guard, `Registry::call` | `Effect::Admit`, or `Effect::Refuse` ("Stopped by the user") for a session that isn't running |
| `Press`, `Move` | `Event::Press`, `Event::Move`, asked before each input; a call may give several | `Effect::Give`, or `Effect::Abort` once the call is stopped |
| `Release`, `Finish` | `Event::Release` once nothing is held, `Event::Finish` when the call is done | none, or `Effect::Abort` once the call is stopped |
| `Stop`, `StopAll` | `Event::Stop`, `Event::StopAll`: `hypr-computer stop <agent>`, `hypr-computer stop`; both also stop the call in progress | `Effect::Stopped` for each session with nothing in progress |
| `Abort` | `Event::Abort`, once the stopped call has released what it held | `Effect::Stopped` if its session is stopping |
| `Stopped` | no event of its own: it happens in the step of the `Stop`, `StopAll` or `Abort` that leaves nothing in progress | `Effect::Stopped`: `hypr-computer stop` answers |
| `Resume`, `ResumeAll` | `Event::Resume`, `Event::ResumeAll`: `hypr-computer resume <agent>`, `hypr-computer resume` | none |

Events the spec doesn't allow in a state, and events for a session that is gone, change nothing and emit no effect, except the answers above. `rust/crates/domain/tests/stop_spec.rs` property-tests `step` against every property above, the two liveness ones bounded: a stopped call is told to abort at its next input, and a stopping session is stopped once it reports the release. It fails when the `CheckStopOnlyAtStart` bug is put back into `step`.

### Left out

- **The lifecycle.** Starting screens, readiness and teardown are `SessionLifecycle`'s; a session here is gone or ready.
- **How an agent is named.** `<agent>` picks sessions; the model stops one slot at a time.
- **A stop that outlives the daemon.** The model has one daemon that is always there. A stop is in the daemon's memory; with no daemon, no agent is working.
- **Several calls of one session at once.** The model gives a session one call at a time. With several in flight, each is a call in progress here: a stop stops every one of them.

## TakeOver

`TakeOver.tla` models the user taking over an agent's screen and handing it back. Live views are watch-only. Clicking into one, or `hypr-computer take <agent>`, takes that agent's screen over: the user's pointer and keys go to it, the view shows a banner, and the user's clipboard is shared with it. The agent's tool calls wait meanwhile; they aren't refused. Escape twice in the view, or `hypr-computer give`, hands back: the clipboards part, and a call that waited runs, once.

The danger is in the overlaps. A call can be giving input (a drag, long text) when the user clicks in, and the user's input must then wait for it to end, or the two mix on the screen. The two ways of handing back, and the session ending, must each undo everything the take-over did. The model gives each session at most one call at a time, which either runs (`"Running"`, giving input until it ends) or waits (`"Waiting"`).

The session lifecycle is `SessionLifecycle`'s; here a session is only gone (`"Absent"`) or open with a ready screen (`"Open"`), and the spec attaches at its `ToolCall`, as `Stop` does, independently of it.

### Properties

| Property | In plain words |
| --- | --- |
| `AgentAndUserNeverInterleave` | While the user has the screen, no call of the agent's is giving it input |
| `WaitingOnlyWhileTakenOver` | A call waits only while the screen is taken over: handing back, either way, lets it go on at once |
| `ResumedAtMostOnce` | A call that waited runs once |
| `ClipboardOnlyDuringTakeOver` | The user's clipboard is shared with a screen only while the user has that screen |
| `OneTakeOverAtATime` | One screen at a time is taken over |
| `EndedSessionLeavesNothing` | A session that ended, during a take-over or not, leaves no take-over, no call and no shared clipboard behind |
| `SessionsAreIndependent` | A step of one session never changes another |
| `OnlyTheUserHandsBack` | A take-over ends only by Escape twice, `hypr-computer give`, or the session ending: never by anything the agent does |
| `OnlyTheUserTakesOver` | Only the user's click or `hypr-computer take` takes a screen over |
| `TakeOverTakesEffect` | A click into the view always gets the user the screen, unless they hand back or the session ends first: an agent that keeps calling can't keep the user out |

The last holds under the assumptions in `Fairness`: a running call ends (the screen gives its input without waiting for the agent), and once nothing is in progress the user who clicked in gets the screen. Nothing is assumed about agents and the user; in particular the user may keep a screen for ever, and a waiting call is answered only when they hand back.

### The bugs it catches

| Config | The bug | Property it violates |
| --- | --- | --- |
| `TakeOverMidCall.cfg` | The click takes over at once, while the agent's drag or text is still giving input. The user's pointer and keys land between the agent's inputs, and the agent's drag ends wherever the user moved. The live-view prototype's version of it: the host's pointer leaked into the nested compositor while it hovered the view. | `AgentAndUserNeverInterleave` |
| `AdmitWhileTaking.cfg` | The guard lets calls through until the user actually has the screen. An agent that keeps calling always has a call in progress, so the user's click never takes effect. | `TakeOverTakesEffect` |
| `EscapeForgetsWaitingCall.cfg` | Two code paths hand back; only `hypr-computer give` wakes the waiting call. After Escape twice the agent's call waits for ever. | `WaitingOnlyWhileTakenOver` |
| `ClipboardKeptOnHandBack.cfg` | The clipboards stay joined after the hand-back, so the agent's apps can read what the user copies next: a password, a 2FA code. | `ClipboardOnlyDuringTakeOver` |
| `EndKeepsTakeOver.cfg` | The daemon forgets a session that ends during a take-over, but not that the user took it over. The gone session holds the take-over, and a click into another agent's view does nothing. | `EndedSessionLeavesNothing` |

TLC's counterexample for `TakeOverMidCall`: an agent connects, its call starts running (`ToolCall`), and the user clicks into the view (`TakeOver`): `control` becomes `"User"` while `call` is still `"Running"`.

### Rust names

The Rust state machine is `domain::takeover::TakeOvers<K>`, keyed by session id, with one pure `step(&mut self, Event<K>) -> Vec<Effect<K>>`; the top of `rust/crates/domain/tests/takeover.rs` documents it in full. It holds `control` (`Control`) and `call` (`Call`), and which key the user pressed last, for Escape twice. `session` is the lifecycle's, `clip` is the world: the clipboards are joined from the `TakenOver(k)` effect to the `Given(k)` effect. `resumed` is the model's record.

| Action in the spec | Rust event | Effect |
| --- | --- | --- |
| `Connect`, `Disconnect` | `Event::Connect`, `Event::Disconnect`, at the lifecycle's | `Effect::Given` if the session was taken over |
| `ToolCall` | `Event::ToolCall`, from the guard, `Registry::call` | `Effect::Admit`, or `Effect::Wait` while the screen is taken over |
| `Finish` | `Event::Finish`, when the call has given all its input | `Effect::TakenOver` if the user clicked in meanwhile |
| `TakeOver` | `Event::Click` (the user presses a button in the view while it is watch-only), `Event::Take` (`hypr-computer take <agent>`) | `Effect::TakenOver`, or none until the call in progress ends; `Effect::Given` (and `Effect::Resume`) for another screen taken over first |
| `Granted` | no event of its own: it happens in the step of the `Click`, `Take` or `Finish` that leaves nothing in progress | `Effect::TakenOver`: the banner shows, the clipboards are joined |
| `EscapeTwice` | the second of two `Event::Key(k, Key::Escape)` in a row while the user has the screen | `Effect::Given`, and `Effect::Resume` for a waiting call |
| `Give` | `Event::Give` (`hypr-computer give`) | `Effect::Given`, and `Effect::Resume` for a waiting call |
| none: the user's input | `Event::Click`, `Event::Pointer`, `Event::Key` while the user has the screen | `Effect::Forward`: give it to the screen |

Events the spec doesn't allow in a state, and events for a session that is gone, change nothing and emit no effect. `rust/crates/domain/tests/takeover_spec.rs` property-tests `step` against every property above, `TakeOverTakesEffect` bounded: a session the user clicked into is theirs once its call reports `Finish`. It fails on each of the five bugs when it is put back into `step`.

### Left out

- **The lifecycle.** Starting screens, readiness and teardown are `SessionLifecycle`'s; a session here is gone or ready.
- **Stop.** A stopped session refuses its calls whether or not the user has its screen; `Stop` covers that, and the two attach at `ToolCall` independently.
- **The user's input itself.** The model has no step for it: the user's input reaches the screen exactly while `control` is `"User"`. The Rust state machine says so with `Effect::Forward`. A button or key the user still holds when they hand back is released by the hand-back.
- **Which Escapes count.** Escape twice is one step here. In Rust the first Escape goes to the screen and the second hands back; any other key or click between them starts the count again.
- **Time.** A call may wait as long as the user keeps the screen; an agent's client may give up on it, which is a disconnect or nothing.
