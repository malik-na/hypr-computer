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
