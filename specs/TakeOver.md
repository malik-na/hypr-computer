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
