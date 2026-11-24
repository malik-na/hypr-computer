-------------------------------- MODULE Stop --------------------------------
(***************************************************************************)
(* The user stops one agent, or all of them, and resumes them.            *)
(*                                                                         *)
(* `hypr-computer stop` stops every agent, and every agent that connects   *)
(* until `hypr-computer resume`.  `hypr-computer stop <agent>` stops one.  *)
(* A stopped session's tool calls are refused, screenshots included, and   *)
(* its screen stays, so the user can look at it.  Resume lets it go on.    *)
(*                                                                         *)
(* Tool calls arrive at any time, from every agent at once, and a stop     *)
(* can arrive in the middle of one.  The dangerous call is one that holds  *)
(* a button or key down across several inputs, like a drag: pressed, then *)
(* moved, then released.  If a stop arrives between its press and its      *)
(* release, its next input must be the release, and nothing else.          *)
(*                                                                         *)
(* The session lifecycle is SessionLifecycle's: here a session is either   *)
(* gone, or open with a ready screen.  This spec attaches at its ToolCall: *)
(* a stopped session that is Ready doesn't serve.                          *)
(*                                                                         *)
(* Each action is one event of the Rust state machine, or one step of the  *)
(* world.  specs/README.md maps every name here to its Rust name.          *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    Sessions,               \* session slots: how many agents the model lets connect at once
    Moves,                  \* how many moves an input in progress makes between its press and its release
    CheckStopOnlyAtStart    \* bug switch, see StopsCall.  FALSE in the correct design

ASSUME Moves \in Nat /\ CheckStopOnlyAtStart \in BOOLEAN

VARIABLES
    session,    \* daemon: "Absent" (no session in this slot), or "Open": connected, its screen ready
    screen,     \* world:  the session's screen process.  "Up" or "None"
    run,        \* daemon: whether the session serves.  "Running", "Stopping" or "Stopped"
    stopNew,    \* daemon: `hypr-computer stop` (all) is in force: a session that connects starts stopped
    call,       \* daemon: the session's tool call in progress
    held,       \* world:  the screen holds the session's button or key down
    moved,      \* world:  how many moves the input in progress has made
    served      \* model only: the session was served (a call admitted, or input given) while it was stopped

vars == <<session, screen, run, stopNew, call, held, moved, served>>

\* "Stopping": the stop has reached the daemon, which no longer serves the session, but the input in progress
\*             (if any) hasn't been released yet.  The user's `hypr-computer stop` is still waiting.
\* "Stopped":  every input in progress is released.  `hypr-computer stop` answers.
RunStates == {"Running", "Stopping", "Stopped"}

\* "Idle":     no call in progress.
\* "Admitted": the guard let a call through; it hasn't given its first input yet.  A screenshot ends here.
\* "Input":    the call pressed its button or key and holds it down while it moves.
\* "Aborting": the call was stopped while admitted or holding; it gives no more input, only the release.
CallStates == {"Idle", "Admitted", "Input", "Aborting"}

TypeOK ==
    /\ session \in [Sessions -> {"Absent", "Open"}]
    /\ screen  \in [Sessions -> {"None", "Up"}]
    /\ run     \in [Sessions -> RunStates]
    /\ stopNew \in BOOLEAN
    /\ call    \in [Sessions -> CallStates]
    /\ held    \in [Sessions -> BOOLEAN]
    /\ moved   \in [Sessions -> 0..Moves]
    /\ served  \in [Sessions -> BOOLEAN]

Init ==
    /\ session = [s \in Sessions |-> "Absent"]
    /\ screen  = [s \in Sessions |-> "None"]
    /\ run     = [s \in Sessions |-> "Running"]
    /\ stopNew = FALSE
    /\ call    = [s \in Sessions |-> "Idle"]
    /\ held    = [s \in Sessions |-> FALSE]
    /\ moved   = [s \in Sessions |-> 0]
    /\ served  = [s \in Sessions |-> FALSE]

-----------------------------------------------------------------------------
\* Records whether session s is served now while it isn't running.  Every action that serves calls it.
Serve(s) == served' = [served EXCEPT ![s] = @ \/ run[s] # "Running"]

\* An agent connects (SessionLifecycle's Connect up to ScreenReady, in one step).  While `hypr-computer stop`
\* (all) is in force it starts stopped: an agent can't get out of a stop by reconnecting.
Connect(s) ==
    /\ session[s] = "Absent"
    /\ session' = [session EXCEPT ![s] = "Open"]
    /\ screen'  = [screen EXCEPT ![s] = "Up"]
    /\ run'     = [run EXCEPT ![s] = IF stopNew THEN "Stopped" ELSE "Running"]
    /\ served'  = [served EXCEPT ![s] = FALSE]
    /\ UNCHANGED <<stopNew, call, held, moved>>

\* The agent's connection closes, at any moment.  Its session is torn down, with its screen, so nothing is held
\* any more.  (SessionLifecycle's teardown, in one step.)
Disconnect(s) ==
    /\ session[s] = "Open"
    /\ session' = [session EXCEPT ![s] = "Absent"]
    /\ screen'  = [screen EXCEPT ![s] = "None"]
    /\ run'     = [run EXCEPT ![s] = "Running"]
    /\ call'    = [call EXCEPT ![s] = "Idle"]
    /\ held'    = [held EXCEPT ![s] = FALSE]
    /\ moved'   = [moved EXCEPT ![s] = 0]
    /\ UNCHANGED <<stopNew, served>>

\* A tool call arrives, and the guard lets it through: only for a running session, one call at a time.  A call
\* for a session that isn't running is refused, "Stopped by the user", and changes nothing, so it isn't an
\* action here.
ToolCall(s) ==
    /\ session[s] = "Open"
    /\ call[s] = "Idle"
    /\ run[s] = "Running"
    /\ call' = [call EXCEPT ![s] = "Admitted"]
    /\ Serve(s)
    /\ UNCHANGED <<session, screen, run, stopNew, held, moved>>

\* A call that gives no input (a screenshot) is done.
Finish(s) ==
    /\ call[s] = "Admitted"
    /\ call' = [call EXCEPT ![s] = "Idle"]
    /\ UNCHANGED <<session, screen, run, stopNew, held, moved, served>>

\* The call's first input: it presses its button or key, and holds it.  (A click or key press is a Press and a
\* Release with no Move.)
Press(s) ==
    /\ call[s] = "Admitted"
    /\ call' = [call EXCEPT ![s] = "Input"]
    /\ held' = [held EXCEPT ![s] = TRUE]
    /\ Serve(s)
    /\ UNCHANGED <<session, screen, run, stopNew, moved>>

\* The call moves, its button still held: one part of a drag.
Move(s) ==
    /\ call[s] = "Input"
    /\ moved[s] < Moves
    /\ moved' = [moved EXCEPT ![s] = @ + 1]
    /\ Serve(s)
    /\ UNCHANGED <<session, screen, run, stopNew, call, held>>

\* The call releases what it holds and is done.
Release(s) ==
    /\ call[s] = "Input"
    /\ call'  = [call EXCEPT ![s] = "Idle"]
    /\ held'  = [held EXCEPT ![s] = FALSE]
    /\ moved' = [moved EXCEPT ![s] = 0]
    /\ UNCHANGED <<session, screen, run, stopNew, served>>

\* BUG SWITCH CheckStopOnlyAtStart.
\* Correct: a stop also stops the session's call in progress.  A call that is admitted, or holds its button,
\*          gives no more input: it releases what it holds, and the agent is told "Stopped by the user".
\* Bug:     the stop is checked only when a call starts.  A call let through before the stop goes on to its end.
\*          The Python server did this until v0.6: a drag checked the stop file once, before pressing, so a stop
\*          during the drag let it move on to its end with the button held.
StopsCall(c) == IF c \in {"Admitted", "Input"} /\ ~CheckStopOnlyAtStart THEN "Aborting" ELSE c

\* The daemon starts stopping session s.  From here on it admits no call for s.
BeginStop(s) ==
    /\ run'  = [run EXCEPT ![s] = "Stopping"]
    /\ call' = [call EXCEPT ![s] = StopsCall(@)]

\* `hypr-computer stop <agent>`: one session.  Stopping one that is already stopped changes nothing.
Stop(s) ==
    /\ session[s] = "Open"
    /\ run[s] = "Running"
    /\ BeginStop(s)
    /\ UNCHANGED <<session, screen, stopNew, held, moved, served>>

\* `hypr-computer stop`: every session, and every one that connects until `hypr-computer resume`.
StopAll ==
    /\ stopNew' = TRUE
    /\ run'  = [s \in Sessions |-> IF session[s] = "Open" /\ run[s] = "Running" THEN "Stopping" ELSE run[s]]
    /\ call' = [s \in Sessions |-> IF session[s] = "Open" /\ run[s] = "Running" THEN StopsCall(call[s])
                                                                                 ELSE call[s]]
    /\ UNCHANGED <<session, screen, held, moved, served>>

\* The stopped call gives its last input: it releases what it holds.  Nothing else.
Abort(s) ==
    /\ call[s] = "Aborting"
    /\ call'  = [call EXCEPT ![s] = "Idle"]
    /\ held'  = [held EXCEPT ![s] = FALSE]
    /\ moved' = [moved EXCEPT ![s] = 0]
    /\ UNCHANGED <<session, screen, run, stopNew, served>>

\* Nothing of the session's is in progress any more: it is stopped, and `hypr-computer stop` answers.
Stopped(s) ==
    /\ run[s] = "Stopping"
    /\ call[s] = "Idle"
    /\ run' = [run EXCEPT ![s] = "Stopped"]
    /\ UNCHANGED <<session, screen, stopNew, call, held, moved, served>>

\* `hypr-computer resume <agent>`: the session serves again.  A call stopped before still ends with its release;
\* the agent makes its calls again.  Resuming one while `hypr-computer stop` (all) is in force resumes that one.
Resume(s) ==
    /\ session[s] = "Open"
    /\ run[s] # "Running"
    /\ run' = [run EXCEPT ![s] = "Running"]
    /\ UNCHANGED <<session, screen, stopNew, call, held, moved, served>>

\* `hypr-computer resume`: every session, and sessions that connect from now on start running.
ResumeAll ==
    /\ stopNew' = FALSE
    /\ run' = [s \in Sessions |-> IF session[s] = "Open" THEN "Running" ELSE run[s]]
    /\ UNCHANGED <<session, screen, call, held, moved, served>>

SessionNext(s) ==
    \/ Connect(s)
    \/ Disconnect(s)
    \/ ToolCall(s)
    \/ Finish(s)
    \/ Press(s)
    \/ Move(s)
    \/ Release(s)
    \/ Stop(s)
    \/ Abort(s)
    \/ Stopped(s)
    \/ Resume(s)

Next == StopAll \/ ResumeAll \/ \E s \in Sessions : SessionNext(s)

-----------------------------------------------------------------------------
(***************************************************************************)
(* Fairness: what is guaranteed to happen, sooner or later.                *)
(*                                                                         *)
(*  - A call in progress ends: one admitted gives its input or finishes,   *)
(*    and one holding its button releases it.  The screen does this; it    *)
(*    doesn't wait for the agent.                                          *)
(*  - A stopped call releases what it holds.                               *)
(*  - A stopping session whose call is over becomes stopped.               *)
(*                                                                         *)
(* Nothing is assumed about agents and the user: Connect, Disconnect,      *)
(* ToolCall, Stop, StopAll, Resume and ResumeAll happen at any allowed     *)
(* moment, or never.                                                       *)
(***************************************************************************)
Fairness ==
    \A s \in Sessions :
        /\ WF_vars(Press(s) \/ Finish(s))
        /\ WF_vars(Release(s))
        /\ WF_vars(Abort(s))
        /\ WF_vars(Stopped(s))

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
\* Safety: what must never happen.

\* No tool call is served for a session once its stop has reached the daemon, until it is resumed: no call is
\* let through, and the screen gets no input from it but the release of what it holds.
NoCallServedWhileStopped ==
    \A s \in Sessions : ~served[s]

\* When `hypr-computer stop` answers, the session has nothing in progress and holds no button or key.
StoppedMeansReleased ==
    \A s \in Sessions : run[s] = "Stopped" => call[s] = "Idle" /\ ~held[s]

\* Only a call that is giving input holds a button or key.
OnlyInputHolds ==
    \A s \in Sessions : held[s] => call[s] \in {"Input", "Aborting"}

\* A session is open exactly while its screen is up: stopping never takes the screen away.
StoppedScreenStays ==
    \A s \in Sessions : session[s] = "Open" <=> screen[s] = "Up"

\* A running session with nothing in progress serves its next call: resume makes the session serve again.
RunningSessionServes ==
    \A s \in Sessions : (session[s] = "Open" /\ run[s] = "Running" /\ call[s] = "Idle") => ENABLED ToolCall(s)

\* A step of one session, stopping and resuming it included, never changes another session.  Only `hypr-computer
\* stop` and `hypr-computer resume` (all) change several.
Slot(s) == <<session[s], screen[s], run[s], call[s], held[s], moved[s], served[s]>>

SessionsAreIndependent ==
    [][\/ StopAll
       \/ ResumeAll
       \/ \A s, t \in Sessions : (s # t /\ Slot(s)' # Slot(s)) => Slot(t)' = Slot(t)]_vars

\* While `hypr-computer stop` (all) is in force, an agent that connects starts stopped, and only resume undoes it.
StopAllHoldsForNewAgents ==
    [][\A s \in Sessions : session[s] = "Absent" /\ session'[s] = "Open" => (run'[s] = "Running" <=> ~stopNew)]_vars

\* A screen goes only when its agent's connection closes: stop and resume never take it away.
OnlyDisconnectEndsScreen ==
    [][\A s \in Sessions : screen[s] = "Up" /\ screen'[s] # "Up" => Disconnect(s)]_vars

-----------------------------------------------------------------------------
\* Liveness: what must happen in the end, given Fairness.

\* `hypr-computer stop` always answers: a stopping session becomes stopped, unless it is resumed or ends first.
StopTakesEffect ==
    \A s \in Sessions : run[s] = "Stopping" ~> run[s] # "Stopping"

\* An input in progress ends released when its session is stopped.
StoppedInputIsReleased ==
    \A s \in Sessions : (held[s] /\ run[s] # "Running") ~> ~held[s]

=============================================================================
