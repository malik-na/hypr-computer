------------------------------ MODULE TakeOver ------------------------------
(***************************************************************************)
(* The user takes over an agent's screen, and hands it back.              *)
(*                                                                         *)
(* Live views are watch-only.  Clicking into one (or `hypr-computer take   *)
(* <agent>`) takes that agent's screen over: the user's pointer and keys   *)
(* go to it, the view shows a banner, and the user's clipboard is shared   *)
(* with it.  The agent's tool calls wait meanwhile; they aren't refused.   *)
(* Escape twice in the view, or `hypr-computer give`, hands it back: the   *)
(* clipboards part, and a call that waited runs, once.                     *)
(*                                                                         *)
(* The danger is in the overlaps.  A call can be giving input when the     *)
(* user clicks in; the user's input must then wait for it to end, or the   *)
(* two mix on the screen.  The two ways of handing back, and the session   *)
(* ending, must each undo everything the take-over did.                    *)
(*                                                                         *)
(* The session lifecycle is SessionLifecycle's: here a session is either   *)
(* gone, or open with a ready screen.  This spec attaches at its ToolCall: *)
(* a session the user has taken over doesn't serve, its calls wait.  Stop  *)
(* attaches there too, independently.                                      *)
(*                                                                         *)
(* Each action is one event of the Rust state machine, or one step of the  *)
(* world.  specs/README.md maps every name here to its Rust name.          *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    Sessions,                   \* session slots: how many agents the model lets connect at once
    TakeOverMidCall,            \* bug switch, see TakeOver.          FALSE in the correct design
    AdmitWhileTaking,           \* bug switch, see ToolCall.          FALSE in the correct design
    EscapeForgetsWaitingCall,   \* bug switch, see EscapeTwice.       FALSE in the correct design
    ClipboardKeptOnHandBack,    \* bug switch, see HandBack.          FALSE in the correct design
    EndKeepsTakeOver            \* bug switch, see Disconnect.        FALSE in the correct design

ASSUME \A b \in {TakeOverMidCall, AdmitWhileTaking, EscapeForgetsWaitingCall, ClipboardKeptOnHandBack,
                 EndKeepsTakeOver} : b \in BOOLEAN

VARIABLES
    session,    \* daemon: "Absent" (no session in this slot), or "Open": connected, its screen ready
    control,    \* daemon: who drives the screen.  "Agent", "Taking" or "User"
    call,       \* daemon: the agent's tool call.  "Idle", "Waiting" or "Running"
    clip,       \* world:  the screen's clipboard is joined to the user's
    resumed     \* model only: how many times the agent's current call was let go on after waiting

vars == <<session, control, call, clip, resumed>>

\* "Agent":  the view is watch-only, and the agent's calls run.
\* "Taking": the user clicked in while one of the agent's calls was giving input.  That call runs to its end; the
\*           user's input isn't given to the screen yet, and the agent's next call waits.
\* "User":   the user has the screen: their input goes to it, the banner shows, the clipboards are joined.
ControlStates == {"Agent", "Taking", "User"}

\* "Idle":    no call.
\* "Waiting": a call arrived while the user had taken over; it has had no answer and gave no input.
\* "Running": a call that was let through: it may give the screen input until it ends.
CallStates == {"Idle", "Waiting", "Running"}

TypeOK ==
    /\ session \in [Sessions -> {"Absent", "Open"}]
    /\ control \in [Sessions -> ControlStates]
    /\ call    \in [Sessions -> CallStates]
    /\ clip    \in [Sessions -> BOOLEAN]
    /\ resumed \in [Sessions -> Nat]

Init ==
    /\ session = [s \in Sessions |-> "Absent"]
    /\ control = [s \in Sessions |-> "Agent"]
    /\ call    = [s \in Sessions |-> "Idle"]
    /\ clip    = [s \in Sessions |-> FALSE]
    /\ resumed = [s \in Sessions |-> 0]

\* Whether some screen is taken over now.  The user has one pointer and one keyboard: one screen at a time.
TakenOver == {s \in Sessions : control[s] # "Agent"}

-----------------------------------------------------------------------------
\* An agent connects (SessionLifecycle's Connect up to ScreenReady, in one step).  Its view is watch-only.
Connect(s) ==
    /\ session[s] = "Absent"
    /\ session' = [session EXCEPT ![s] = "Open"]
    /\ control' = [control EXCEPT ![s] = "Agent"]
    /\ call'    = [call EXCEPT ![s] = "Idle"]
    /\ clip'    = [clip EXCEPT ![s] = FALSE]
    /\ resumed' = [resumed EXCEPT ![s] = 0]

\* BUG SWITCH EndKeepsTakeOver.
\* Correct: a session that ends during a take-over ends it too, like any other: the view goes, and the user can
\*          take over another screen.
\* Bug:     the daemon forgets the session but not that the user took it over.  The gone session still holds the
\*          take-over, so a click into another agent's view does nothing until the daemon restarts.
\*
\* The agent's connection closes, at any moment, a take-over or a waiting call included.  The session is torn
\* down with its screen (SessionLifecycle's teardown, in one step): the waiting call has no one left to answer.
Disconnect(s) ==
    /\ session[s] = "Open"
    /\ session' = [session EXCEPT ![s] = "Absent"]
    /\ control' = [control EXCEPT ![s] = IF EndKeepsTakeOver THEN @ ELSE "Agent"]
    /\ call'    = [call EXCEPT ![s] = "Idle"]
    /\ clip'    = [clip EXCEPT ![s] = FALSE]
    /\ resumed' = [resumed EXCEPT ![s] = 0]

\* BUG SWITCH AdmitWhileTaking.
\* Correct: once the user has clicked in, the agent's new calls wait, even before the call in progress has ended.
\* Bug:     the guard lets a call through until the user actually has the screen.  An agent that keeps calling
\*          always has a call in progress, so the user never gets the screen.
MustWait(s) ==
    IF AdmitWhileTaking THEN control[s] = "User" ELSE control[s] # "Agent"

\* A tool call arrives at the guard, one at a time per session.  While the screen is taken over it waits: no
\* answer, no input.  Otherwise it runs.
ToolCall(s) ==
    /\ session[s] = "Open"
    /\ call[s] = "Idle"
    /\ call'    = [call EXCEPT ![s] = IF MustWait(s) THEN "Waiting" ELSE "Running"]
    /\ resumed' = [resumed EXCEPT ![s] = 0]
    /\ UNCHANGED <<session, control, clip>>

\* The running call has given all its input and answered the agent.  If the user clicked in meanwhile, the user
\* has the screen now (Granted).
Finish(s) ==
    /\ call[s] = "Running"
    /\ call' = [call EXCEPT ![s] = "Idle"]
    /\ UNCHANGED <<session, control, clip, resumed>>

\* BUG SWITCH TakeOverMidCall.
\* Correct: a click into the view while one of the agent's calls is giving input (a drag, long text) takes over
\*          once that call has ended; until then the user's input isn't given to the screen.
\* Bug:     the click takes over at once.  The user's pointer and keys reach the screen between the inputs of the
\*          agent's drag or text: the two mix, and the agent's drag lands wherever the user moved.
\*
\* The user clicks into the view, or runs `hypr-computer take <agent>`.  Another screen taken over is handed back
\* first: that is its HandBack, a step of its own before this one.  The click itself is given to no one.
TakeOver(s) ==
    /\ session[s] = "Open"
    /\ TakenOver = {}
    /\ IF call[s] = "Running" /\ ~TakeOverMidCall
       THEN /\ control' = [control EXCEPT ![s] = "Taking"]
            /\ UNCHANGED clip
       ELSE /\ control' = [control EXCEPT ![s] = "User"]
            /\ clip'    = [clip EXCEPT ![s] = TRUE]
    /\ UNCHANGED <<session, call, resumed>>

\* The call in progress is over: the user has the screen.  The banner shows and the clipboards are joined.
Granted(s) ==
    /\ control[s] = "Taking"
    /\ call[s] # "Running"
    /\ control' = [control EXCEPT ![s] = "User"]
    /\ clip'    = [clip EXCEPT ![s] = TRUE]
    /\ UNCHANGED <<session, call, resumed>>

\* BUG SWITCH ClipboardKeptOnHandBack.
\* Correct: handing back parts the clipboards, however it is done.
\* Bug:     the clipboards stay joined after the hand-back, so the agent's apps read whatever the user copies
\*          next: a password, a 2FA code.
\*
\* The screen goes back to the agent.  A call that waited runs now, in the same step: once, and only if `wake`.
HandBack(s, wake) ==
    /\ control' = [control EXCEPT ![s] = "Agent"]
    /\ clip'    = [clip EXCEPT ![s] = IF ClipboardKeptOnHandBack THEN @ ELSE FALSE]
    /\ IF call[s] = "Waiting" /\ wake
       THEN /\ call'    = [call EXCEPT ![s] = "Running"]
            /\ resumed' = [resumed EXCEPT ![s] = @ + 1]
       ELSE UNCHANGED <<call, resumed>>
    /\ UNCHANGED session

\* BUG SWITCH EscapeForgetsWaitingCall.
\* Correct: both ways of handing back let the waiting call go on.
\* Bug:     only `hypr-computer give` wakes the waiting call; Escape twice hands the screen back but the agent's
\*          call waits for ever.
\*
\* The user presses Escape twice in the view, with the screen theirs.
EscapeTwice(s) ==
    /\ session[s] = "Open"
    /\ control[s] = "User"
    /\ HandBack(s, ~EscapeForgetsWaitingCall)

\* `hypr-computer give`: hands back the screen taken over, whether the user has it yet or not.
Give(s) ==
    /\ session[s] = "Open"
    /\ control[s] # "Agent"
    /\ HandBack(s, TRUE)

SessionNext(s) ==
    \/ Connect(s)
    \/ Disconnect(s)
    \/ ToolCall(s)
    \/ Finish(s)
    \/ TakeOver(s)
    \/ Granted(s)
    \/ EscapeTwice(s)
    \/ Give(s)

Next == \E s \in Sessions : SessionNext(s)

-----------------------------------------------------------------------------
(***************************************************************************)
(* Fairness: what is guaranteed to happen, sooner or later.                *)
(*                                                                         *)
(*  - A running call ends.  The screen gives its input without waiting for *)
(*    the agent.                                                           *)
(*  - Once nothing is in progress, the user who clicked in gets the screen.*)
(*                                                                         *)
(* Nothing is assumed about agents and the user: Connect, Disconnect,      *)
(* ToolCall, TakeOver, EscapeTwice and Give happen at any allowed moment,  *)
(* or never.  In particular the user may keep a screen for ever: a call    *)
(* that waits is answered only when the user hands back.                   *)
(***************************************************************************)
Fairness ==
    \A s \in Sessions :
        /\ WF_vars(Finish(s))
        /\ WF_vars(Granted(s))

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
\* Safety: what must never happen.

\* The agent's input and the user's never mix: while the user has the screen, no call of the agent's is running.
AgentAndUserNeverInterleave ==
    \A s \in Sessions : ~(control[s] = "User" /\ call[s] = "Running")

\* A call waits only while the screen is taken over.  Handing back, either way, lets it go on at once: a waiting
\* call is resumed at least once.
WaitingOnlyWhileTakenOver ==
    \A s \in Sessions : call[s] = "Waiting" => control[s] # "Agent"

\* ... and at most once: a call that waited runs once.
ResumedAtMostOnce ==
    \A s \in Sessions : resumed[s] <= 1

\* The user's clipboard is shared with a screen only while the user has that screen.
ClipboardOnlyDuringTakeOver ==
    \A s \in Sessions : clip[s] => control[s] = "User"

\* One screen at a time is taken over.
OneTakeOverAtATime ==
    Cardinality(TakenOver) <= 1

\* A session that ended, during a take-over or not, leaves nothing behind: no take-over, no call, no clipboard.
EndedSessionLeavesNothing ==
    \A s \in Sessions : session[s] = "Absent" => control[s] = "Agent" /\ call[s] = "Idle" /\ ~clip[s]

\* A step of one session never changes another.
Slot(s) == <<session[s], control[s], call[s], clip[s], resumed[s]>>

SessionsAreIndependent ==
    [][\A s, t \in Sessions : (s # t /\ Slot(s)' # Slot(s)) => Slot(t)' = Slot(t)]_vars

\* Only the user hands back: the agent's calls never end a take-over.  The session ending does.
OnlyTheUserHandsBack ==
    [][\A s \in Sessions : control[s] # "Agent" /\ control'[s] = "Agent" =>
           EscapeTwice(s) \/ Give(s) \/ Disconnect(s)]_vars

\* Only the user takes over: the agent can't give the user its screen.
OnlyTheUserTakesOver ==
    [][\A s \in Sessions : control[s] = "Agent" /\ control'[s] # "Agent" => TakeOver(s)]_vars

-----------------------------------------------------------------------------
\* Liveness: what must happen in the end, given Fairness.

\* A click into the view always gets the user the screen, unless they hand back or the session ends first: an
\* agent that keeps calling can't keep the user out.
TakeOverTakesEffect ==
    \A s \in Sessions : control[s] = "Taking" ~> control[s] # "Taking"

=============================================================================
