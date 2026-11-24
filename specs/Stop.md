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
