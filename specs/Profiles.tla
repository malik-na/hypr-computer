------------------------------- MODULE Profiles -------------------------------
(***************************************************************************)
(* Saved, named browser profiles, and the lock that keeps each one to a    *)
(* single session at a time.                                               *)
(*                                                                         *)
(* The user makes a profile with `hypr-computer profile new <name>` and    *)
(* removes it with `hypr-computer profile delete <name>`.  An agent uses   *)
(* one by naming it in `open_url {url, profile}`.  Without `profile`, a    *)
(* session's browser starts fresh, as before.  A session's first           *)
(* `open_url` fixes its profile, named or fresh, until the session ends.   *)
(*                                                                         *)
(* Why the lock matters: Chromium allows one browser per profile folder.   *)
(* A second Chromium started on a folder already in use doesn't open a     *)
(* window of its own: it hands its URL to the first one and exits.  So     *)
(* without the lock a second agent's page would open on the first agent's  *)
(* screen, in its logins.                                                  *)
(*                                                                         *)
(* Two locks keep a profile to one browser:                                *)
(*  - the daemon's table (`holder`), which says which session has which    *)
(*    profile, and gives a clear refusal;                                  *)
(*  - a lock of the operating system's on the profile, held by the session *)
(*    and by the browser it starts, so that the kernel frees it when the   *)
(*    last of them is gone.  It covers what the table can't see: a browser *)
(*    left running by a daemon that died, and `hypr-computer profile       *)
(*    delete` run while no daemon is.                                      *)
(*                                                                         *)
(* The session lifecycle is SessionLifecycle's, in three states: gone,     *)
(* open (its screen ready), or ending (torn down: its browser is being     *)
(* closed).  specs/README.md maps every name here to its Rust name.        *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    Sessions,               \* session slots: how many agents the model lets connect at once
    Profiles,               \* the names the user may give profiles
    TwoSessionsBothAcquire, \* bug switch, see Open.            FALSE in the correct design
    ReleaseAtDisconnect,    \* bug switch, see Disconnect.      FALSE in the correct design
    LockNotHeldByBrowser,   \* bug switch, see OsFree.          FALSE in the correct design
    DeleteInUse             \* bug switch, see Delete.          FALSE in the correct design

ASSUME
    /\ Sessions \cap Profiles = {}
    /\ {"Unset", "Fresh", "Nobody"} \cap (Sessions \cup Profiles) = {}
    /\ \A b \in {TwoSessionsBothAcquire, ReleaseAtDisconnect, LockNotHeldByBrowser, DeleteInUse} : b \in BOOLEAN

VARIABLES
    session,    \* daemon: "Absent", "Open" or "Ending" (disconnected; its browser is being closed)
    uses,       \* daemon: the session's profile: "Unset" until its first open_url, then "Fresh" or a profile
    acquiring,  \* daemon: the session asked for a named profile, and its OS lock is being taken
    holder,     \* daemon: the session each profile is held by in the daemon's table, or "Nobody"
    browser,    \* world:  the profile folder the session's running browser uses: "None", "Fresh" or a profile
    orphan,     \* world:  a browser left running on the profile by a daemon that died
    exists      \* world:  the profile's folder is on disk

vars == <<session, uses, acquiring, holder, browser, orphan, exists>>

Choices == {"Unset", "Fresh"} \cup Profiles

TypeOK ==
    /\ session   \in [Sessions -> {"Absent", "Open", "Ending"}]
    /\ uses      \in [Sessions -> Choices]
    /\ acquiring \in [Sessions -> BOOLEAN]
    /\ holder    \in [Profiles -> Sessions \cup {"Nobody"}]
    /\ browser   \in [Sessions -> {"None", "Fresh"} \cup Profiles]
    /\ orphan    \in [Profiles -> BOOLEAN]
    /\ exists    \in [Profiles -> BOOLEAN]

Init ==
    /\ session   = [s \in Sessions |-> "Absent"]
    /\ uses      = [s \in Sessions |-> "Unset"]
    /\ acquiring = [s \in Sessions |-> FALSE]
    /\ holder    = [p \in Profiles |-> "Nobody"]
    /\ browser   = [s \in Sessions |-> "None"]
    /\ orphan    = [p \in Profiles |-> FALSE]
    /\ exists    = [p \in Profiles |-> FALSE]

-----------------------------------------------------------------------------
\* BUG SWITCH LockNotHeldByBrowser.
\* Correct: the OS lock is held by the session and by its browser (an inherited lock, freed by the kernel when the
\*          last holder is gone).  A browser left running when its daemon died still holds it.
\* Bug:     the lock lives only in the daemon (its table, or a lock only the daemon's process holds).  When the
\*          daemon dies, the lock goes with it while the browser runs on, and the next daemon starts a second
\*          browser on the same profile, or lets the user delete it from under the first.
OsFree(p) == ~orphan[p] \/ LockNotHeldByBrowser

\* The profile is in use: the daemon's table, a session's choice or the OS lock says so.
InUse(p) ==
    \/ holder[p] # "Nobody"
    \/ \E s \in Sessions : session[s] # "Absent" /\ uses[s] = p
    \/ ~OsFree(p)

\* An agent connects (SessionLifecycle's Connect up to ScreenReady, in one step).
Connect(s) ==
    /\ session[s] = "Absent"
    /\ browser[s] = "None"
    /\ session' = [session EXCEPT ![s] = "Open"]
    /\ UNCHANGED <<uses, acquiring, holder, browser, orphan, exists>>

\* `open_url` without `profile`: the session's browser, started on the session's profile if it isn't running.  The
\* first one fixes the session's profile as fresh.
OpenNoProfile(s) ==
    /\ session[s] = "Open"
    /\ ~acquiring[s]
    /\ uses'    = [uses EXCEPT ![s] = IF @ = "Unset" THEN "Fresh" ELSE @]
    /\ browser' = [browser EXCEPT ![s] = uses'[s]]
    /\ UNCHANGED <<session, acquiring, holder, orphan, exists>>

\* BUG SWITCH TwoSessionsBothAcquire.
\* Correct: checking that the profile is free and taking it are one step of the daemon's table.
\* Bug:     the table is checked when the call arrives, but written only once the OS lock is taken (an await
\*          later).  Two sessions that ask at once both find the profile free.  (If the OS lock is taken by one
\*          process for every session, it doesn't keep two of the daemon's own sessions apart either.)
TakeAtOpen(s, p) == IF TwoSessionsBothAcquire THEN holder ELSE [holder EXCEPT ![p] = s]

\* `open_url {profile: p}`, the session's first: p exists and the daemon's table has it free, so the session takes
\* it, and the daemon takes its OS lock.  A missing profile, one in use, or a session whose profile is already
\* fixed (to another, or fresh) is refused with a clear error and changes nothing, so it isn't an action here.
\* Asking again for the profile the session already has is OpenNoProfile.
Open(s, p) ==
    /\ session[s] = "Open"
    /\ ~acquiring[s]
    /\ uses[s] = "Unset"
    /\ exists[p]
    /\ holder[p] = "Nobody"
    /\ uses'      = [uses EXCEPT ![s] = p]
    /\ acquiring' = [acquiring EXCEPT ![s] = TRUE]
    /\ holder'    = TakeAtOpen(s, p)
    /\ UNCHANGED <<session, browser, orphan, exists>>

\* The OS lock is taken: the browser starts on the profile.
Acquired(s) ==
    /\ acquiring[s]
    /\ session[s] = "Open"
    /\ OsFree(uses[s])
    /\ acquiring' = [acquiring EXCEPT ![s] = FALSE]
    /\ holder'    = [holder EXCEPT ![uses[s]] = s]
    /\ browser'   = [browser EXCEPT ![s] = uses[s]]
    /\ UNCHANGED <<session, uses, orphan, exists>>

\* The OS lock is held by a browser outside the daemon's table: the call is refused ("in use"), and the session's
\* profile is unset again, so it may ask again later.
Busy(s) ==
    /\ acquiring[s]
    /\ ~OsFree(uses[s])
    /\ acquiring' = [acquiring EXCEPT ![s] = FALSE]
    /\ uses'      = [uses EXCEPT ![s] = "Unset"]
    /\ holder'    = [p \in Profiles |-> IF holder[p] = s THEN "Nobody" ELSE holder[p]]
    /\ UNCHANGED <<session, browser, orphan, exists>>

\* The agent closes its browser's last window, or the browser crashes.  The session keeps its profile: the next
\* open_url starts the browser again on it.
BrowserCloses(s) ==
    /\ session[s] = "Open"
    /\ browser[s] # "None"
    /\ browser' = [browser EXCEPT ![s] = "None"]
    /\ UNCHANGED <<session, uses, acquiring, holder, orphan, exists>>

\* BUG SWITCH ReleaseAtDisconnect.
\* Correct: the session keeps its profile until its teardown is done, after its browser is seen to be gone.
\* Bug:     the profile is released as soon as the agent's connection closes.  Its browser is still shutting down
\*          (writing its cookies, which takes a moment), and the next session's browser starts on the same folder.
Released(s) == [p \in Profiles |-> IF holder[p] = s THEN "Nobody" ELSE holder[p]]

\* The agent's connection closes, at any moment, whatever it was doing.  The daemon starts tearing the session
\* down, and closes its browser.
Disconnect(s) ==
    /\ session[s] = "Open"
    /\ session' = [session EXCEPT ![s] = "Ending"]
    /\ holder'  = IF ReleaseAtDisconnect THEN Released(s) ELSE holder
    /\ UNCHANGED <<uses, acquiring, browser, orphan, exists>>

\* The world: the session's browser has shut down (or been killed, when it wouldn't).
BrowserStopped(s) ==
    /\ session[s] = "Ending"
    /\ browser[s] # "None"
    /\ browser' = [browser EXCEPT ![s] = "None"]
    /\ UNCHANGED <<session, uses, acquiring, holder, orphan, exists>>

\* The session's browser is seen to be gone: the session releases its profile, in the table and its OS lock.
TeardownDone(s) ==
    /\ session[s] = "Ending"
    /\ browser[s] = "None"
    /\ session'   = [session EXCEPT ![s] = "Absent"]
    /\ uses'      = [uses EXCEPT ![s] = "Unset"]
    /\ acquiring' = [acquiring EXCEPT ![s] = FALSE]
    /\ holder'    = Released(s)
    /\ UNCHANGED <<browser, orphan, exists>>

\* `hypr-computer profile new p`.
Create(p) ==
    /\ ~exists[p]
    /\ exists' = [exists EXCEPT ![p] = TRUE]
    /\ UNCHANGED <<session, uses, acquiring, holder, browser, orphan>>

\* BUG SWITCH DeleteInUse.
\* Correct: `hypr-computer profile delete p` is refused while p is in use, and deletes nothing.
\* Bug:     it deletes the folder anyway.  The browser on it carries on without its files, and writes some of them
\*          back, so the user's "deleted" logins come back half-made.
\* `hypr-computer profile delete p`.  Refused while p is in use, so that's not an action here.
Delete(p) ==
    /\ exists[p]
    /\ (~InUse(p) \/ DeleteInUse)
    /\ exists' = [exists EXCEPT ![p] = FALSE]
    /\ UNCHANGED <<session, uses, acquiring, holder, browser, orphan>>

\* The daemon dies (kill -9, a crash) and systemd starts it again.  Its table and sessions are gone; its agents
\* connect again as new sessions.  Their browsers lose their screens and exit, but not at once: until then each
\* is an orphan on its profile.  Profiles on disk stay.
DaemonRestarts ==
    /\ orphan'    = [p \in Profiles |-> orphan[p] \/ \E s \in Sessions : browser[s] = p]
    /\ session'   = [s \in Sessions |-> "Absent"]
    /\ uses'      = [s \in Sessions |-> "Unset"]
    /\ acquiring' = [s \in Sessions |-> FALSE]
    /\ holder'    = [p \in Profiles |-> "Nobody"]
    /\ browser'   = [s \in Sessions |-> "None"]
    /\ UNCHANGED exists

\* The world: an orphaned browser has exited.
OrphanExits(p) ==
    /\ orphan[p]
    /\ orphan' = [orphan EXCEPT ![p] = FALSE]
    /\ UNCHANGED <<session, uses, acquiring, holder, browser, exists>>

SessionNext(s) ==
    \/ Connect(s)
    \/ OpenNoProfile(s)
    \/ \E p \in Profiles : Open(s, p)
    \/ Acquired(s)
    \/ Busy(s)
    \/ BrowserCloses(s)
    \/ Disconnect(s)
    \/ BrowserStopped(s)
    \/ TeardownDone(s)

ProfileNext(p) == Create(p) \/ Delete(p) \/ OrphanExits(p)

Next ==
    \/ DaemonRestarts
    \/ \E s \in Sessions : SessionNext(s)
    \/ \E p \in Profiles : ProfileNext(p)

-----------------------------------------------------------------------------
(***************************************************************************)
(* Fairness: what is guaranteed to happen, sooner or later.                *)
(*                                                                         *)
(*  - Taking a profile's OS lock comes to an end: it is taken, or found    *)
(*    held.                                                                *)
(*  - A session being torn down gets its browser closed (killed if it      *)
(*    won't close), and its teardown is done once the browser is gone.     *)
(*    The daemon does both; neither waits for the agent.                   *)
(*  - An orphaned browser exits: its screen is gone.                       *)
(*                                                                         *)
(* Nothing is assumed about agents and the user: they connect, open pages, *)
(* close the browser, disconnect, create and delete profiles at any        *)
(* allowed moment, or never.  The daemon may die at any moment.            *)
(***************************************************************************)
Fairness ==
    /\ \A s \in Sessions :
        /\ WF_vars(Acquired(s) \/ Busy(s))
        /\ WF_vars(BrowserStopped(s))
        /\ WF_vars(TeardownDone(s))
    /\ \A p \in Profiles : WF_vars(OrphanExits(p))

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
\* Safety: what must never happen.

\* At most one browser runs on a profile's folder: no second agent's page lands in the first one's browser.
OneBrowserPerProfile ==
    \A p \in Profiles :
        Cardinality({s \in Sessions : browser[s] = p}) + (IF orphan[p] THEN 1 ELSE 0) <= 1

\* No browser runs on a profile the user deleted.
NoBrowserOnDeletedProfile ==
    /\ \A s \in Sessions : browser[s] \in Profiles => exists[browser[s]]
    /\ \A p \in Profiles : orphan[p] => exists[p]

\* A session's browser runs on the session's own profile.
BrowserOnItsOwnProfile ==
    \A s \in Sessions : browser[s] # "None" => browser[s] = uses[s]

\* The table gives a profile only to a session that chose it.
HolderChoseIt ==
    \A p \in Profiles : holder[p] \in Sessions => uses[holder[p]] = p /\ session[holder[p]] # "Absent"

\* A session's profile, once fixed, stays until the session ends.  Only a refused lock unsets it.
OneProfilePerSession ==
    [][\A s \in Sessions :
        (uses[s] # "Unset" /\ uses'[s] # uses[s]) => (uses'[s] = "Unset" /\ (TeardownDone(s) \/ Busy(s) \/ DaemonRestarts))]_vars

\* A profile goes only when the user deletes it: not when a session ends, nor when the daemon restarts.
ProfilesStayUntilDeleted ==
    [][\A p \in Profiles : exists[p] /\ ~exists'[p] => Delete(p)]_vars

\* A step of one session never changes another session.  Only a daemon restart changes several.
Slot(s) == <<session[s], uses[s], acquiring[s], browser[s]>>

SessionsAreIndependent ==
    [][\/ DaemonRestarts
       \/ \A s, t \in Sessions : (s # t /\ Slot(s)' # Slot(s)) => Slot(t)' = Slot(t)]_vars

-----------------------------------------------------------------------------
\* Liveness: what must happen in the end, given Fairness.

\* A session that ended releases its profile: the next session may have it.
EndedSessionReleases ==
    \A s \in Sessions, p \in Profiles : (session[s] = "Ending" /\ holder[p] = s) ~> holder[p] # s

\* Taking a profile's lock always comes to an answer.
OpenAnswers ==
    \A s \in Sessions : acquiring[s] ~> ~acquiring[s]

\* A profile a dead daemon's browser was on becomes free again.
OrphanedProfileFreed ==
    \A p \in Profiles : orphan[p] ~> ~orphan[p]

=============================================================================
