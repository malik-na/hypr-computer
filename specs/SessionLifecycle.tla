-------------------------- MODULE SessionLifecycle --------------------------
(***************************************************************************)
(* The daemon's session and screen lifecycle.                              *)
(*                                                                         *)
(* One daemon runs per login session.  Agents connect at any time, and     *)
(* each connection is a session.  For each session the daemon starts a     *)
(* screen process (the agent's own compositor).  The screen takes time to  *)
(* become ready, can fail to start and can crash at any time.  The agent   *)
(* can disconnect or be killed at any moment, including while its screen   *)
(* is still starting.  When a session ends, the daemon stops the screen    *)
(* process and the apps on it.  The daemon itself keeps running.           *)
(*                                                                         *)
(* The model keeps two things apart, because both past bugs lived in the   *)
(* gap between them:                                                       *)
(*   - what the daemon believes: `session`, the state of the Rust state    *)
(*     machine;                                                            *)
(*   - what is true in the world: `conn`, `screen` and `apps`.             *)
(*                                                                         *)
(* Each action is one event of the Rust state machine, or one step of the  *)
(* world.  specs/README.md maps every name here to its Rust name.          *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    Sessions,                       \* session slots: how many agents the model lets connect at once
    HandOffEarly,                   \* bug switch, see ScreenStarted.  FALSE in the correct design
    CleanupDependsOnDyingProcess    \* bug switch, see Fairness.  FALSE in the correct design

ASSUME HandOffEarly \in BOOLEAN /\ CleanupDependsOnDyingProcess \in BOOLEAN

VARIABLES
    conn,       \* world:  the agent's connection.  "Open" or "Closed"
    session,    \* daemon: its record of the session.  "Absent" means it holds nothing for this slot
    screen,     \* world:  the session's screen process
    apps,       \* world:  the apps the agent started on that screen
    teardowns   \* model only: how many times teardown was started for the session in this slot

vars == <<conn, session, screen, apps, teardowns>>

SessionStates == {"Absent", "Starting", "Verifying", "Ready", "TearingDown"}
Live          == {"Starting", "Verifying", "Ready"}

\* "Spawning":  the process exists but doesn't answer yet.
\* "Unsettled": it answers, but isn't usable yet.  In the prototype, focus could still sit on an output with no
\*              workspace, and an app launched now never got a window.
\* "Settled":   an app launched now gets a window.
ScreenStates == {"NoProcess", "Spawning", "Unsettled", "Settled"}

\* "NoWindow": an app was launched on a screen that hadn't settled.  This is what the HandOffEarly bug looks like.
AppStates == {"None", "Windowed", "NoWindow"}

TypeOK ==
    /\ conn      \in [Sessions -> {"Open", "Closed"}]
    /\ session   \in [Sessions -> SessionStates]
    /\ screen    \in [Sessions -> ScreenStates]
    /\ apps      \in [Sessions -> AppStates]
    /\ teardowns \in [Sessions -> Nat]

Init ==
    /\ conn      = [s \in Sessions |-> "Closed"]
    /\ session   = [s \in Sessions |-> "Absent"]
    /\ screen    = [s \in Sessions |-> "NoProcess"]
    /\ apps      = [s \in Sessions |-> "None"]
    /\ teardowns = [s \in Sessions |-> 0]

-----------------------------------------------------------------------------
\* The daemon starts tearing a session down.  From here on the session serves nothing.
BeginTeardown(s) ==
    /\ session'   = [session EXCEPT ![s] = "TearingDown"]
    /\ teardowns' = [teardowns EXCEPT ![s] = @ + 1]

\* An agent connects.  The daemon creates the session and spawns its screen process.
Connect(s) ==
    /\ session[s] = "Absent"
    /\ conn'      = [conn EXCEPT ![s] = "Open"]
    /\ session'   = [session EXCEPT ![s] = "Starting"]
    /\ screen'    = [screen EXCEPT ![s] = "Spawning"]
    /\ teardowns' = [teardowns EXCEPT ![s] = 0]
    /\ UNCHANGED apps

\* BUG SWITCH HandOffEarly.
\* Correct: a screen that has started still has to be verified, so the session goes to "Verifying".
\* Bug:     the daemon hands the screen to the agent as soon as the process answers.
AfterScreenStarted == IF HandOffEarly THEN "Ready" ELSE "Verifying"

\* The screen process answers.  If the session is already being torn down (the agent left while the screen was
\* starting), the process is up all the same and the daemon must not bring the session back to life.
ScreenStarted(s) ==
    /\ screen[s] = "Spawning"
    /\ session[s] \in {"Starting", "TearingDown"}
    /\ screen'  = [screen EXCEPT ![s] = "Unsettled"]
    /\ session' = [session EXCEPT ![s] = IF @ = "Starting" THEN AfterScreenStarted ELSE @]
    /\ UNCHANGED <<conn, apps, teardowns>>

\* World only: the screen becomes usable.  The daemon is not told; it has to look (ScreenReady).
ScreenSettles(s) ==
    /\ screen[s] = "Unsettled"
    /\ screen' = [screen EXCEPT ![s] = "Settled"]
    /\ UNCHANGED <<conn, session, apps, teardowns>>

\* The daemon's readiness check passes.  It passes only on a screen that really has settled.
ScreenReady(s) ==
    /\ session[s] = "Verifying"
    /\ screen[s] = "Settled"
    /\ session' = [session EXCEPT ![s] = "Ready"]
    /\ UNCHANGED <<conn, screen, apps, teardowns>>

\* The screen didn't start, or didn't pass the readiness check in time.  Its process may have exited or may
\* still be running; teardown deals with both.
ScreenFailed(s) ==
    /\ session[s] \in {"Starting", "Verifying"}
    /\ \E left \in {screen[s], "NoProcess"} : screen' = [screen EXCEPT ![s] = left]
    /\ BeginTeardown(s)
    /\ UNCHANGED <<conn, apps>>

\* The screen process of a live session exits on its own.  Its apps may live on, so the session is torn down.
ScreenCrashed(s) ==
    /\ session[s] \in Live
    /\ screen[s] # "NoProcess"
    /\ screen' = [screen EXCEPT ![s] = "NoProcess"]
    /\ BeginTeardown(s)
    /\ UNCHANGED <<conn, apps>>

\* The daemon serves a tool call.  The call launches an app, which gets a window only on a settled screen.
ToolCall(s) ==
    /\ conn[s] = "Open"
    /\ session[s] = "Ready"
    /\ apps' = [apps EXCEPT ![s] = IF screen[s] = "Settled" THEN "Windowed" ELSE "NoWindow"]
    /\ UNCHANGED <<conn, session, screen, teardowns>>

\* The agent's connection closes: it disconnected, exited or was killed.  The kernel reports this to the daemon
\* however the agent died.  A session that is already being torn down is not torn down again.
Disconnect(s) ==
    /\ conn[s] = "Open"
    /\ conn' = [conn EXCEPT ![s] = "Closed"]
    /\ IF session[s] \in Live THEN BeginTeardown(s) ELSE UNCHANGED <<session, teardowns>>
    /\ UNCHANGED <<screen, apps>>

\* Teardown, step by step.  The two steps below are steps of the world: the screen process is gone, the apps
\* are gone.  Whether anything guarantees that they happen is the second bug switch: see Fairness.
ScreenStopped(s) ==
    /\ session[s] = "TearingDown"
    /\ screen[s] # "NoProcess"
    /\ screen' = [screen EXCEPT ![s] = "NoProcess"]
    /\ UNCHANGED <<conn, session, apps, teardowns>>

AppsStopped(s) ==
    /\ session[s] = "TearingDown"
    /\ apps[s] # "None"
    /\ apps' = [apps EXCEPT ![s] = "None"]
    /\ UNCHANGED <<conn, session, screen, teardowns>>

\* The daemon has seen that the screen process and the apps are gone.  Only now does it drop the session, and
\* close the connection if the agent is still there (the session ended because its screen failed or crashed).
TeardownDone(s) ==
    /\ session[s] = "TearingDown"
    /\ screen[s] = "NoProcess"
    /\ apps[s] = "None"
    /\ session' = [session EXCEPT ![s] = "Absent"]
    /\ conn'    = [conn EXCEPT ![s] = "Closed"]
    /\ UNCHANGED <<screen, apps, teardowns>>

TeardownStep(s) == ScreenStopped(s) \/ AppsStopped(s) \/ TeardownDone(s)

SessionNext(s) ==
    \/ Connect(s)
    \/ ScreenStarted(s)
    \/ ScreenSettles(s)
    \/ ScreenReady(s)
    \/ ScreenFailed(s)
    \/ ScreenCrashed(s)
    \/ ToolCall(s)
    \/ Disconnect(s)
    \/ TeardownStep(s)

Next == \E s \in Sessions : SessionNext(s)

-----------------------------------------------------------------------------
(***************************************************************************)
(* Fairness: what is guaranteed to happen, sooner or later.                *)
(*                                                                         *)
(*  - A screen process that is spawning comes up, unless it fails or is    *)
(*    stopped first.                                                       *)
(*  - A screen that is up settles, unless it crashes or is stopped first.  *)
(*  - The daemon's readiness check notices a settled screen.               *)
(*  - The daemon finishes a teardown once nothing is left to stop.         *)
(*  - BUG SWITCH CleanupDependsOnDyingProcess.                             *)
(*    Correct: the daemon stops the screen and its apps itself, by         *)
(*    killing their process groups.  A process can't refuse SIGKILL, so    *)
(*    ScreenStopped and AppsStopped are guaranteed.                        *)
(*    Bug: the daemon leaves the cleanup to the session's own process on   *)
(*    its way out.  That process can die or hang before it has cleaned     *)
(*    up, so nothing guarantees ScreenStopped and AppsStopped.             *)
(*                                                                         *)
(* Nothing is assumed about Connect, Disconnect, ToolCall, ScreenFailed    *)
(* and ScreenCrashed: each can happen at any allowed moment, or never.     *)
(***************************************************************************)
CleanupIsGuaranteed == ~CleanupDependsOnDyingProcess

Fairness ==
    \A s \in Sessions :
        /\ WF_vars(ScreenStarted(s))
        /\ WF_vars(ScreenSettles(s))
        /\ WF_vars(ScreenReady(s))
        /\ WF_vars(TeardownDone(s))
        /\ CleanupIsGuaranteed => WF_vars(ScreenStopped(s)) /\ WF_vars(AppsStopped(s))

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
\* Safety: what must never happen.

\* A tool call is served only on a screen that is verified ready.
ToolCallOnlyOnReadyScreen ==
    \A s \in Sessions : apps[s] # "NoWindow"

\* No screen process and no app exists for a session that is gone.
NoScreenWithoutSession ==
    \A s \in Sessions : session[s] = "Absent" => screen[s] = "NoProcess" /\ apps[s] = "None"

\* A session whose agent is gone is being torn down, or is gone too.
NoSessionWithoutAgent ==
    \A s \in Sessions : conn[s] = "Closed" => session[s] \in {"Absent", "TearingDown"}

\* Teardown is started at most once per session.
TeardownAtMostOnce ==
    \A s \in Sessions : teardowns[s] <= 1

\* The daemon is never in a state it can't leave: every slot always has a next step, and a session that is
\* being torn down always has a teardown step, one that needs nothing from the agent.
DaemonNeverStuck ==
    \A s \in Sessions :
        /\ ENABLED SessionNext(s)
        /\ session[s] = "TearingDown" => ENABLED TeardownStep(s)

\* One session's teardown, or any other step of it, never changes another session's state.
Slot(s) == <<conn[s], session[s], screen[s], apps[s], teardowns[s]>>

SessionsAreIndependent ==
    [][\A s, t \in Sessions : (s # t /\ Slot(s)' # Slot(s)) => Slot(t)' = Slot(t)]_vars

-----------------------------------------------------------------------------
\* Liveness: what must happen in the end, given Fairness.

Released(s) == session[s] = "Absent" /\ screen[s] = "NoProcess" /\ apps[s] = "None"

\* Every session that was disconnected or has failed gets its screen and apps removed.
ResourcesEventuallyReleased ==
    \A s \in Sessions : (conn[s] = "Closed" \/ session[s] = "TearingDown") ~> Released(s)

\* A session whose agent stays connected, and whose screen neither fails nor crashes, becomes ready.
StartingSessionBecomesReady ==
    \A s \in Sessions : session[s] = "Starting" ~> session[s] \in {"Ready", "TearingDown", "Absent"}

=============================================================================
