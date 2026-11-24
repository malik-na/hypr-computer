------------------------------- MODULE Notice -------------------------------
(***************************************************************************)
(* The notice that tells the user an agent is at work.                    *)
(*                                                                         *)
(* Each session gets one desktop notification, from the notification      *)
(* service on the session bus (org.freedesktop.Notifications).  It names  *)
(* the agent and says where to watch it, and stays until the user presses *)
(* OK or Stop.  Stop stops that agent, and no other, as                   *)
(* `hypr-computer stop <agent>` does.  When the session ends, its notice  *)
(* is replaced by a short-lived "done" notice.  When the service fails,   *)
(* or doesn't answer in time, the host desktop shows its own notice       *)
(* instead, which goes away by itself.                                     *)
(*                                                                         *)
(* The service is another program.  It answers when it likes, or never,  *)
(* and it may keep a notification on screen after its sender closes it.   *)
(* The daemon must still never leave a notice behind its session.         *)
(*                                                                         *)
(* The session lifecycle is SessionLifecycle's: here a session is either  *)
(* gone or open, and it opens when its agent has said its name.  The stop *)
(* itself is Stop's: here it is only whether the daemon stopped the       *)
(* session.  Session ids aren't reused: each one opens once, at most,    *)
(* and ends once.  A session's notice is named by its session's id.       *)
(*                                                                         *)
(* Each action is one event of the Rust state machine, or one step of the *)
(* world.  specs/README.md maps every name here to its Rust name.         *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    Sessions,           \* the sessions: each opens at most once, and ends at most once
    CloseOnEnd,         \* bug switch, see Disconnect.  FALSE in the correct design
    ForgetLateAnswer,   \* bug switch, see Disconnect.  FALSE in the correct design
    WaitForever,        \* bug switch, see Timeout.     FALSE in the correct design
    StopStopsAll        \* bug switch, see Answer.      FALSE in the correct design

ASSUME {CloseOnEnd, ForgetLateAnswer, WaitForever, StopStopsAll} \subseteq BOOLEAN

VARIABLES
    session,    \* daemon: "New" (not connected yet), "Open", or "Gone"
    notice,     \* daemon: what it believes about each notice, see Notices
    service,    \* world:  the notification service.  "Up", "Down" (none, or no bus: a call fails) or "Hung"
    keeps,      \* world:  the service keeps a notification on screen after its sender closes it (Omarchy's)
    asks,       \* bus:    Notify calls for the notice that the service hasn't taken yet
    answer,     \* bus:    the service's answer to Notify, on its way to the daemon.  "None", "Id" or "Error"
    cards,      \* world:  how many of the notice's lasting notifications (with OK and Stop) are on screen
    done,       \* world:  its "done" notice: "None", "Asked" (on its way), "Shown" (going by itself), "Gone"
    closes,     \* bus:    a CloseNotification for the notice is on its way.  Only the bug CloseOnEnd sends one
    host,       \* world:  the host desktop showed its own notice for this one.  It goes by itself
    signal,     \* bus:    the user's answer on the notification, on its way.  "None", "Ok", "Stop", "Closed"
    pressed,    \* model:  the user pressed Stop on the notice
    stopped     \* daemon: it stopped the notice's session (Stop's `Stop`)

vars == <<session, notice, service, keeps, asks, answer, cards, done, closes, host, signal, pressed,
          stopped>>

\* What the daemon believes about a notice.
\* "None":     no session yet, or nothing more to do.
\* "Asking":   it asked the service to show the notice and waits for the answer, for a limited time.
\* "Shown":    the service showed it and said its id.
\* "Late":     the service didn't answer in time: the host desktop showed its notice.  The daemon still waits
\*             for the answer, and takes the service's notice if it comes.
\* "Fallback": the service failed: the host desktop showed its notice.
\* "Answered": the user pressed OK or Stop, or closed it.  It is gone.
\* "Ending":   the session ended while the daemon waited for the answer; when it comes, it shows "done".
Notices == {"None", "Asking", "Shown", "Late", "Fallback", "Answered", "Ending"}

Live(n) == session[n] = "Open"

TypeOK ==
    /\ session \in [Sessions -> {"New", "Open", "Gone"}]
    /\ notice  \in [Sessions -> Notices]
    /\ service \in {"Up", "Down", "Hung"}
    /\ keeps   \in BOOLEAN
    /\ asks    \in [Sessions -> 0..2]
    /\ answer  \in [Sessions -> {"None", "Id", "Error"}]
    /\ cards   \in [Sessions -> 0..2]
    /\ done    \in [Sessions -> {"None", "Asked", "Shown", "Gone"}]
    /\ closes  \in [Sessions -> BOOLEAN]
    /\ host    \in [Sessions -> BOOLEAN]
    /\ signal  \in [Sessions -> {"None", "Ok", "Stop", "Closed"}]
    /\ pressed \in [Sessions -> BOOLEAN]
    /\ stopped \in [Sessions -> BOOLEAN]

Init ==
    /\ session = [s \in Sessions |-> "New"]
    /\ notice  = [n \in Sessions |-> "None"]
    /\ service \in {"Up", "Down", "Hung"}
    /\ keeps   \in BOOLEAN
    /\ asks    = [n \in Sessions |-> 0]
    /\ answer  = [n \in Sessions |-> "None"]
    /\ cards   = [n \in Sessions |-> 0]
    /\ done    = [n \in Sessions |-> "None"]
    /\ closes  = [n \in Sessions |-> FALSE]
    /\ host    = [n \in Sessions |-> FALSE]
    /\ signal  = [n \in Sessions |-> "None"]
    /\ pressed = [n \in Sessions |-> FALSE]
    /\ stopped = [n \in Sessions |-> FALSE]

-----------------------------------------------------------------------------
\* The daemon.

\* A session opens and its agent says its name: the daemon asks the service for the notice (Notify, with OK and
\* Stop, never expiring) and starts waiting for the answer.
Connect(n) ==
    /\ session[n] = "New"
    /\ session' = [session EXCEPT ![n] = "Open"]
    /\ notice'  = [notice EXCEPT ![n] = "Asking"]
    /\ asks'    = [asks EXCEPT ![n] = @ + 1]
    /\ UNCHANGED <<service, keeps, answer, cards, done, closes, host, signal, pressed, stopped>>

\* BUG SWITCHES CloseOnEnd and ForgetLateAnswer.
\* Correct: when the session ends, a notice on screen is replaced by "done", which expires: the daemon asks the
\*          service for it (Notify, replacing the notice's id).  A notice whose answer hasn't come yet is
\*          remembered ("Ending"), and replaced by "done" as soon as its id comes.
\* CloseOnEnd: the notice is closed instead (CloseNotification).  The Python server's first notice did this
\*          (78fb1c2): Omarchy's shell keeps a notification on screen after its sender closes it, until its own
\*          time runs out, and the notice's never does.  Fixed before v0.8.0 (e92614e) by replacing it with
\*          "AI agent done", which expires in 4 s.
\* ForgetLateAnswer: the daemon forgets a notice whose id hasn't come yet.  The Python server does this
\*          (v0.8.0, close_notice): a server that exits before notify-send has printed the notice's id only
\*          stops notify-send, and the notice, which never expires, stays on screen.
Disconnect(n) ==
    /\ session[n] = "Open"
    /\ session' = [session EXCEPT ![n] = "Gone"]
    /\ notice'  = [notice EXCEPT ![n] = IF @ \in {"Asking", "Late"} /\ ~ForgetLateAnswer THEN "Ending" ELSE "None"]
    /\ done'    = [done EXCEPT ![n] = IF notice[n] = "Shown" /\ ~CloseOnEnd THEN "Asked" ELSE @]
    /\ closes'  = [closes EXCEPT ![n] = @ \/ (notice[n] = "Shown" /\ CloseOnEnd)]
    /\ UNCHANGED <<service, keeps, asks, answer, cards, host, signal, pressed, stopped>>

\* The service's answer comes: the notice's id.  A notice that fell back to the host's is taken: the host's goes
\* by itself.  One whose session has ended is replaced by "done" at once.
Shown(n) ==
    /\ answer[n] = "Id"
    /\ answer' = [answer EXCEPT ![n] = "None"]
    /\ notice' = [notice EXCEPT ![n] = CASE @ \in {"Asking", "Late"} -> "Shown"
                                         [] @ = "Ending"             -> "None"
                                         [] OTHER                    -> @]
    /\ done'   = [done EXCEPT ![n] = IF notice[n] = "Ending" THEN "Asked" ELSE @]
    /\ UNCHANGED <<session, service, keeps, asks, cards, closes, host, signal, pressed, stopped>>

\* The service's answer comes: an error.  The host desktop shows its notice, unless it already has.
Failed(n) ==
    /\ answer[n] = "Error"
    /\ answer' = [answer EXCEPT ![n] = "None"]
    /\ notice' = [notice EXCEPT ![n] = CASE @ \in {"Asking", "Late"} -> "Fallback"
                                         [] @ = "Ending"             -> "None"
                                         [] OTHER                    -> @]
    /\ host'   = [host EXCEPT ![n] = @ \/ notice[n] = "Asking"]
    /\ UNCHANGED <<session, service, keeps, asks, cards, done, closes, signal, pressed, stopped>>

\* BUG SWITCH WaitForever.
\* Correct: the daemon waits a few seconds for the service's answer, then has the host desktop show its notice,
\*          and goes on waiting for the answer.
\* Bug:     it waits for the answer as long as it takes.  The Python server's first notice did this
\*          (78fb1c2): with no notification service to reach, notify-send didn't fail but hung, and no notice
\*          ever appeared.  Fixed before v0.8.0 (d2d3d50): 3 s, then Hyprland's notice.
Timeout(n) ==
    /\ ~WaitForever
    /\ notice[n] = "Asking"
    /\ notice' = [notice EXCEPT ![n] = "Late"]
    /\ host'   = [host EXCEPT ![n] = TRUE]
    /\ UNCHANGED <<session, service, keeps, asks, answer, cards, done, closes, signal, pressed, stopped>>

\* BUG SWITCH StopStopsAll.
\* Correct: the user's answer on the notice comes.  Stop stops the notice's own session, and no other.  OK, Stop
\*          and closing it all leave nothing for the daemon to do with it.  An answer on a notice that isn't
\*          shown any more (its session ended) changes nothing.
\* Bug:     Stop stops every session.  The Python server's "Stop agent" does this (v0.8.0): it stops every
\*          agent, as `hypr-computer stop` does.  #29 asks for Stop to affect only its own session.
Answer(n) ==
    /\ signal[n] # "None"
    /\ answer[n] = "None"       \* the service's messages come in order: its answer before the user's
    /\ signal' = [signal EXCEPT ![n] = "None"]
    /\ IF notice[n] = "Shown"
       THEN /\ notice' = [notice EXCEPT ![n] = "Answered"]
            /\ stopped' = IF signal[n] # "Stop" THEN stopped
                          ELSE IF StopStopsAll THEN [m \in Sessions |-> stopped[m] \/ Live(m)]
                          ELSE [stopped EXCEPT ![n] = TRUE]
       ELSE UNCHANGED <<notice, stopped>>
    /\ UNCHANGED <<session, service, keeps, asks, answer, cards, done, closes, host, pressed>>

-----------------------------------------------------------------------------
\* The world: the notification service and the user.

\* The service takes a Notify call: it shows the notice, which stays until the user answers, and says its id.
ServiceShows(n) ==
    /\ service = "Up"
    /\ asks[n] > 0
    /\ answer[n] = "None"
    /\ asks'   = [asks EXCEPT ![n] = @ - 1]
    /\ cards'  = [cards EXCEPT ![n] = @ + 1]
    /\ answer' = [answer EXCEPT ![n] = "Id"]
    /\ UNCHANGED <<session, notice, service, keeps, done, closes, host, signal, pressed, stopped>>

\* There is no service (or no session bus): the call fails.
ServiceFails(n) ==
    /\ service = "Down"
    /\ asks[n] > 0
    /\ answer[n] = "None"
    /\ asks'   = [asks EXCEPT ![n] = @ - 1]
    /\ answer' = [answer EXCEPT ![n] = "Error"]
    /\ UNCHANGED <<session, notice, service, keeps, cards, done, closes, host, signal, pressed, stopped>>

\* (A "Hung" service takes calls and never answers: it has no action.)

\* The service replaces the notice by "done", which goes by itself; if the notice is already gone, "done" shows
\* on its own.
ServiceReplaces(n) ==
    /\ done[n] = "Asked"
    /\ done'  = [done EXCEPT ![n] = "Shown"]
    /\ cards' = [cards EXCEPT ![n] = IF @ > 0 THEN @ - 1 ELSE 0]
    /\ UNCHANGED <<session, notice, service, keeps, asks, answer, closes, host, signal, pressed, stopped>>

\* The service closes the notice, unless it is one that keeps closed notices on screen.
ServiceCloses(n) ==
    /\ closes[n]
    /\ closes' = [closes EXCEPT ![n] = FALSE]
    /\ cards'  = [cards EXCEPT ![n] = IF keeps \/ @ = 0 THEN @ ELSE @ - 1]
    /\ UNCHANGED <<session, notice, service, keeps, asks, answer, done, host, signal, pressed, stopped>>

\* "done" expires.
DoneExpires(n) ==
    /\ done[n] = "Shown"
    /\ done' = [done EXCEPT ![n] = "Gone"]
    /\ UNCHANGED <<session, notice, service, keeps, asks, answer, cards, closes, host, signal, pressed,
                   stopped>>

\* The user presses OK or Stop on the notice, or closes it.  The service takes it off the screen and tells the
\* daemon.
UserAnswers(n, a) ==
    /\ cards[n] > 0
    /\ signal[n] = "None"
    /\ cards'   = [cards EXCEPT ![n] = @ - 1]
    /\ signal'  = [signal EXCEPT ![n] = a]
    /\ pressed' = [pressed EXCEPT ![n] = @ \/ a = "Stop"]
    /\ UNCHANGED <<session, notice, service, keeps, asks, answer, done, closes, host, stopped>>

Next ==
    \/ \E n \in Sessions :
        \/ Connect(n) \/ Disconnect(n)
        \/ Shown(n) \/ Failed(n) \/ Timeout(n) \/ Answer(n)
        \/ ServiceShows(n) \/ ServiceFails(n) \/ ServiceReplaces(n) \/ ServiceCloses(n) \/ DoneExpires(n)
        \/ \E a \in {"Ok", "Stop", "Closed"} : UserAnswers(n, a)

-----------------------------------------------------------------------------
(***************************************************************************)
(* Fairness: what is guaranteed to happen, sooner or later.                *)
(*                                                                         *)
(*  - A service that is up shows the notice; one that is down fails.  A    *)
(*    hung one does nothing, ever.                                         *)
(*  - The service's answers and the user's reach the daemon, which handles *)
(*    them, and its timer fires.                                           *)
(*  - The service replaces a notice by "done", and handles a close, when   *)
(*    asked.                                                               *)
(*                                                                         *)
(* Nothing is assumed about agents and the user: Connect, Disconnect and   *)
(* UserAnswers happen at any allowed moment, or never.                     *)
(***************************************************************************)
Fairness ==
    \A n \in Sessions :
        /\ WF_vars(ServiceShows(n))
        /\ WF_vars(ServiceFails(n))
        /\ WF_vars(Shown(n))
        /\ WF_vars(Failed(n))
        /\ WF_vars(Timeout(n))
        /\ WF_vars(Answer(n))
        /\ WF_vars(ServiceReplaces(n))
        /\ WF_vars(ServiceCloses(n))

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
\* Safety: what must never happen.

\* A session has at most one notice on screen that stays until the user answers it.
AtMostOneNotice ==
    \A n \in Sessions : cards[n] <= 1

\* No notice outlives its session: once the session has ended, its notice is on screen only while the daemon's
\* request to take it off is on its way, or while the daemon still waits for its id, to replace it at once.
NoNoticeOutlivesItsSession ==
    \A n \in Sessions : (cards[n] > 0 /\ ~Live(n)) => (notice[n] = "Ending" \/ done[n] = "Asked" \/ closes[n])

\* Stop on a notice stops its own session and no other: a session is stopped from its notice only after the user
\* pressed Stop on that one.
StopOnlyItsOwn ==
    \A n \in Sessions : stopped[n] => pressed[n]

\* The daemon stops only a session that is open.
StopOnlyLive ==
    [][\A n \in Sessions : ~stopped[n] /\ stopped'[n] => Live(n)]_vars

-----------------------------------------------------------------------------
\* Liveness: what must happen in the end, given Fairness.

\* The user is told: an open session's notice is shown, by the service or by the host desktop, unless the
\* session ends first.
Told(n) == cards[n] > 0 \/ host[n] \/ notice[n] = "Answered"

UserIsTold ==
    \A n \in Sessions : Live(n) ~> (Told(n) \/ ~Live(n))

\* A notice whose session has ended goes, even if its id came late.
NoticeGoes ==
    \A n \in Sessions : (cards[n] > 0 /\ ~Live(n)) ~> cards[n] = 0

\* Stop on a notice takes effect: its session is stopped, unless it ends first.
StopTakesEffect ==
    \A n \in Sessions : (pressed[n] /\ Live(n)) ~> (stopped[n] \/ ~Live(n))

=============================================================================
