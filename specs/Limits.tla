------------------------------- MODULE Limits -------------------------------
(***************************************************************************)
(* The limits the user puts on agents with `hypr-computer allow`, and the *)
(* block on password managers, which is always on.                        *)
(*                                                                         *)
(* Out of the box an agent may do anything on its own screen, except use   *)
(* a password manager.  `hypr-computer allow` limits every agent to the    *)
(* apps it names, to the websites it names, or to looking only (read-only  *)
(* mode), for a set time or until `hypr-computer allow --off`.  Each       *)
(* `allow` replaces the limits before it.  A call a limit forbids is       *)
(* refused, and the agent is told to ask the user.                         *)
(*                                                                         *)
(* The user sets limits at any time, the clock runs, and agents call       *)
(* tools at any time.  A limit set while a session runs applies to its     *)
(* next call; a limit whose time is up stops applying.  The browser's     *)
(* every request goes through the daemon's proxy, which holds it to the   *)
(* listed sites, redirects and a page's own requests included.  Apps on    *)
(* the screen may start a password prompt at any time; its window is      *)
(* never shown.                                                            *)
(*                                                                         *)
(* A tool call is one step: the guard decides, and an admitted call does   *)
(* what it does.  Stop.tla covers what happens during a call.  Each action *)
(* is one step of the daemon or of the world; specs/README.md maps every   *)
(* name here to its Rust name.                                             *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    Apps,       \* programs an agent may launch with launch_app, by file name
    Vaults,     \* the password managers and password prompts among them
    Browser,    \* the name an app limit gives the agent's browser
    Sites,      \* hosts a page may be on
    MaxTime,    \* the clock runs from 0 to MaxTime
    \* Bug switches, each used in one place and FALSE in the correct design.
    LimitsReadAtConnect,        \* see Read
    ExpiredLimitStillApplies,   \* see AfterExpiry
    CheckOnlyRequestedUrl,      \* see ProxyLets
    HideOnlyBeforeCapture       \* see BlockedAtMap

ASSUME
    /\ Vaults \subseteq Apps
    /\ Browser \notin Apps
    /\ MaxTime \in Nat \ {0}
    /\ {LimitsReadAtConnect, ExpiredLimitStillApplies, CheckOnlyRequestedUrl, HideOnlyBeforeCapture}
        \subseteq BOOLEAN

All     == "All"        \* a kind with no limit: every app, or every site
Forever == MaxTime + 1  \* a limit set without --for, until `hypr-computer allow --off`: the clock never gets there
None    == "None"
Blocked == "Blocked"    \* the proxy's page: "this site isn't one the user allowed"

Names == Apps \cup {Browser}

\* The last call before the first.
NoCall == [kind |-> "None", app |-> None, site |-> None]

\* The model's records describe the step just taken only.  Before a call, or after a step that is not one:
NoLast  == [act |-> NoCall, admitted |-> TRUE, allowed |-> TRUE]
NoFetch == [site |-> None, allowed |-> TRUE]

\* What a limit is: the apps agents may use (All: every app), the sites their browser may load (All: every
\* site), whether they may only look, and when its time is up.
Limit == [apps: {All} \cup Names, sites: {All} \cup Sites, readOnly: BOOLEAN, until: 1..Forever]

\* No limit: the default, and what `hypr-computer allow --off` sets.
NoLimit == [apps |-> All, sites |-> All, readOnly |-> FALSE, until |-> Forever]

\* What one `hypr-computer allow` can set.  The model lets the user limit one kind at a time, to one app or one
\* site: a list, or several kinds at once, change nothing a check has to get right.
LimitValues ==
    {NoLimit}
    \cup {[NoLimit EXCEPT !.apps = n, !.until = u] : n \in Names, u \in 1..Forever}
    \cup {[NoLimit EXCEPT !.sites = x, !.until = u] : x \in Sites, u \in 1..Forever}
    \cup {[NoLimit EXCEPT !.readOnly = TRUE, !.until = u] : u \in 1..Forever}

\* What an agent asks for.  "Look": a screenshot, or any tool that only reads.  "Input": a click, typing, a key,
\* a scroll, a move or a drag, which go to the app in front (`app`, None with nothing there).  "Launch":
\* launch_app of `app`.  "Open": open_url of a page on `site`.
Acts ==
    [kind: {"Look"}, app: {None}, site: {None}]
    \cup [kind: {"Input"}, app: Names \cup {None}, site: {None}]
    \cup [kind: {"Launch"}, app: Names, site: {None}]
    \cup [kind: {"Open"}, app: {None}, site: Sites]

VARIABLES
    limits,     \* daemon: what the user's last `hypr-computer allow` set
    now,        \* world:  the clock
    session,    \* daemon: "Absent", or "Open": an agent is connected and its screen is ready
    seen,       \* daemon: the limits as they were when the session connected (see Read)
    running,    \* world:  the apps running on the session's screen, by name (the browser is `page`)
    shown,      \* world:  the apps with a window on the screen
    front,      \* world:  the app whose window is in front, which input goes to, or None
    page,       \* world:  the site of the page the browser shows, Blocked, or None with no browser
    capturing,  \* daemon: a screenshot is being taken
    hidden,     \* daemon: the windows the screenshot in progress paints over
    picture,    \* model:  the password managers' windows in the screenshot just taken
    last,       \* model:  the tool call just made: what it asked, whether it was admitted, whether the limits allow it
    fetched     \* model:  the site the browser's request just reached, and whether the limits allow it

vars == <<limits, now, session, seen, running, shown, front, page, capturing, hidden, picture, last, fetched>>

TypeOK ==
    /\ limits \in Limit
    /\ now \in 0..MaxTime
    /\ session \in {"Absent", "Open"}
    /\ seen \in Limit
    /\ running \subseteq Names
    /\ shown \subseteq Names
    /\ front \in shown \cup {None}
    /\ page \in Sites \cup {Blocked, None}
    /\ capturing \in BOOLEAN
    /\ hidden \subseteq Names
    /\ picture \subseteq Vaults
    /\ last \in [act: Acts \cup {NoCall}, admitted: BOOLEAN, allowed: BOOLEAN]
    /\ fetched \in [site: Sites \cup {None}, allowed: BOOLEAN]

-----------------------------------------------------------------------------
(* What the limits allow.  These are the user's intent, which the daemon   *)
(* must carry out exactly.                                                 *)

InForce(L) == now < L.until

\* The limits that apply now: an expired limit stops applying.
InEffect(L) == IF InForce(L) THEN L ELSE NoLimit

\* Whether app n may be used under limits E.  A password manager never may.
AppOk(E, n) == n \notin Vaults /\ (E.apps = All \/ n = E.apps)

SiteOk(E, s) == E.sites = All \/ s = E.sites

\* Whether limits E (in force) allow act.  Reading the screen is always allowed.  Read-only allows nothing else.
\* Input needs the app in front allowed; with an app limit and nothing in front, there is nothing it may go to.
\* The browser is started only by open_url, never by launch_app, so that it always uses the proxy.  open_url
\* needs the browser allowed and the site listed.
Permits(E, act) ==
    CASE act.kind = "Look"   -> TRUE
      [] act.kind = "Input"  -> ~E.readOnly /\ IF act.app = None THEN E.apps = All ELSE AppOk(E, act.app)
      [] act.kind = "Launch" -> ~E.readOnly /\ act.app # Browser /\ AppOk(E, act.app)
      [] act.kind = "Open"   -> ~E.readOnly /\ AppOk(E, Browser) /\ SiteOk(E, act.site)

Allowed(act) == Permits(InEffect(limits), act)

-----------------------------------------------------------------------------
(* What the daemon goes by.  In the correct design it is the above.        *)

\* The limits the guard checks a call against.  Correct: the user's, as they are at that call, so a limit set
\* mid-session applies to the session's next call.
\* LimitsReadAtConnect: read once, when the session starts, and kept for its life: the shape of a guard that
\* takes the limits from the session's config instead of asking for them at every call.
Read == IF LimitsReadAtConnect THEN seen ELSE limits

\* What applies once the limits' time is up.  Correct: nothing; an expired limit stops applying.
\* ExpiredLimitStillApplies: v0.8's `hypr-computer allow --for` (d7f3efe): once the time was up, agents could
\* only read the screen until the user renewed the grant or turned it off.  The owner decided against it.
AfterExpiry == IF ExpiredLimitStillApplies THEN [NoLimit EXCEPT !.readOnly = TRUE] ELSE NoLimit

Applied == IF InForce(Read) THEN Read ELSE AfterExpiry

Admits(act) == Permits(Applied, act)

\* Whether a request of the browser for site s reaches it.  Correct: the browser is always started pointed at
\* the daemon's proxy, which checks every request, so a redirect, a link or a page's own request to a site
\* that isn't listed gets the proxy's page instead.
\* CheckOnlyRequestedUrl: only the URL given to open_url is checked, and the browser fetches the rest itself.
ProxyLets(s) == CheckOnlyRequestedUrl \/ SiteOk(Applied, s)

\* Where the password-manager block lives.  Correct: the screen never maps a password manager's window (it
\* ends its app instead), so no picture can hold it, whenever it comes.
\* HideOnlyBeforeCapture: v0.8 (fde9289) painted deny-listed windows black in each screenshot, finding them
\* when the capture began; one that appeared or moved during the capture was in the picture.  e6d3101 fixed
\* it by looking again after the capture.
BlockedAtMap == ~HideOnlyBeforeCapture

-----------------------------------------------------------------------------
Init ==
    /\ limits = NoLimit
    /\ now = 0
    /\ session = "Absent"
    /\ seen = NoLimit
    /\ running = {}
    /\ shown = {}
    /\ front = None
    /\ page = None
    /\ capturing = FALSE
    /\ hidden = {}
    /\ picture = {}
    /\ last = NoLast
    /\ fetched = NoFetch

\* A step that records nothing.
Quiet == last' = NoLast /\ fetched' = NoFetch /\ picture' = {}

World == <<running, shown, front, page, capturing, hidden>>

\* `hypr-computer allow ...`: replaces the limits.  With --for, the time is up at `until`.
SetLimits(L) ==
    /\ L.until > now
    /\ limits' = L
    /\ Quiet /\ UNCHANGED <<now, session, seen>> /\ UNCHANGED World

\* `hypr-computer allow --off`.
LimitsOff ==
    /\ limits' = NoLimit
    /\ Quiet /\ UNCHANGED <<now, session, seen>> /\ UNCHANGED World

Tick ==
    /\ now < MaxTime
    /\ now' = now + 1
    /\ Quiet /\ UNCHANGED <<limits, session, seen>> /\ UNCHANGED World

Connect ==
    /\ session = "Absent"
    /\ session' = "Open"
    /\ seen' = limits
    /\ Quiet /\ UNCHANGED <<limits, now>> /\ UNCHANGED World

\* The session ends, and its screen with everything on it.
Disconnect ==
    /\ session = "Open"
    /\ session' = "Absent"
    /\ seen' = NoLimit
    /\ running' = {} /\ shown' = {} /\ front' = None /\ page' = None
    /\ capturing' = FALSE /\ hidden' = {}
    /\ Quiet /\ UNCHANGED <<limits, now>>

\* The browser asks the proxy for a page on s, and gets it, or the proxy's page.
Fetch(s) ==
    IF ProxyLets(s)
    THEN page' = s /\ fetched' = [site |-> s, allowed |-> SiteOk(InEffect(limits), s)]
    ELSE page' = Blocked /\ fetched' = NoFetch

\* What an admitted call does.  A screenshot is two steps, so that a window can come in between.
Do(act) ==
    CASE act.kind = "Look"   -> /\ capturing' = TRUE
                                /\ hidden' = IF BlockedAtMap THEN {} ELSE shown \cap Vaults
                                /\ fetched' = NoFetch
                                /\ UNCHANGED <<running, shown, front, page>>
      [] act.kind = "Input"  -> fetched' = NoFetch /\ UNCHANGED World
      [] act.kind = "Launch" -> /\ running' = running \cup {act.app}
                                /\ fetched' = NoFetch
                                /\ UNCHANGED <<shown, front, page, capturing, hidden>>
      [] act.kind = "Open"   -> /\ Fetch(act.site)
                                /\ UNCHANGED <<running, shown, front, capturing, hidden>>

\* A tool call: the guard admits it or refuses it.  Input goes to the app in front.
ToolCall(act) ==
    /\ session = "Open"
    /\ act.kind = "Input" => act.app = front
    /\ act.kind = "Look" => ~capturing
    /\ last' = [act |-> act, admitted |-> Admits(act), allowed |-> Allowed(act)]
    /\ picture' = {}
    /\ IF Admits(act) THEN Do(act) ELSE fetched' = NoFetch /\ UNCHANGED World
    /\ UNCHANGED <<limits, now, session, seen>>

\* The screenshot in progress is taken: what is shown then, but for what it paints over.
CaptureDone ==
    /\ capturing
    /\ capturing' = FALSE
    /\ hidden' = {}
    /\ picture' = (shown \ hidden) \cap Vaults
    /\ last' = NoLast /\ fetched' = NoFetch
    /\ UNCHANGED <<limits, now, session, seen, running, shown, front, page>>

\* An app on the screen (or the browser) opens its window, which comes in front.  A password manager's never
\* shows: its app is ended.
MapWindow(n) ==
    /\ session = "Open"
    /\ n \notin shown
    /\ n \in running \/ (n = Browser /\ page # None)
    /\ IF n \in Vaults /\ BlockedAtMap
       THEN running' = running \ {n} /\ UNCHANGED <<shown, front>>
       ELSE shown' = shown \cup {n} /\ front' = n /\ UNCHANGED running
    /\ Quiet /\ UNCHANGED <<limits, now, session, seen, page, capturing, hidden>>

\* An app the agent may use starts a password prompt or a password manager itself (gpg starts pinentry).
Spawn(v) ==
    /\ session = "Open"
    /\ v \in Vaults \ running
    /\ \E r \in shown : r \notin Vaults
    /\ running' = running \cup {v}
    /\ Quiet /\ UNCHANGED <<limits, now, session, seen, shown, front, page, capturing, hidden>>

\* The agent or an app brings another window in front.
Raise(n) ==
    /\ n \in shown \ {front}
    /\ front' = n
    /\ Quiet /\ UNCHANGED <<limits, now, session, seen, running, shown, page, capturing, hidden>>

\* The page the browser shows sends it to s: a redirect, a link the agent clicked, or a request of its own.
PageGoes(s) ==
    /\ session = "Open"
    /\ page \in Sites
    /\ Fetch(s)
    /\ last' = NoLast /\ picture' = {}
    /\ UNCHANGED <<limits, now, session, seen, running, shown, front, capturing, hidden>>

Next ==
    \/ \E L \in LimitValues : SetLimits(L)
    \/ LimitsOff
    \/ Tick
    \/ Connect
    \/ Disconnect
    \/ \E act \in Acts : ToolCall(act)
    \/ CaptureDone
    \/ \E n \in Names : MapWindow(n) \/ Raise(n)
    \/ \E v \in Vaults : Spawn(v)
    \/ \E s \in Sites : PageGoes(s)

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Properties                                                              *)

\* Every call the guard admits is one the limits in force at that moment allow: a limit set mid-session applies
\* to the next call, and a password manager is never launched or given input.
CallsKeepToLimits == last.admitted => last.allowed

\* Every call the limits in force allow is admitted: no limit refuses more than it says, an expired limit
\* stops applying, and with no limit everything but a password manager is allowed.
RefusedOnlyIfForbidden == last.allowed => last.admitted

\* No request of the browser reaches a site the limits in force don't list, whether it was the page asked
\* for, a redirect from it, or a request the page made.
OnlyListedSitesReached == fetched.allowed

\* No screenshot ever holds a password manager's window.
PasswordManagerNeverShown == picture = {}

\* The rules themselves, stated on their own, so that a mistake in Permits can't hide behind both sides of the
\* two properties above.
\* No password manager is launched or given input, whatever the user allowed.
PasswordManagerNeverUsed ==
    last.admitted /\ last.act.kind \in {"Launch", "Input"} => last.act.app \notin Vaults

\* In read-only mode an agent only looks.
ReadOnlyOnlyLooks ==
    last.admitted /\ InEffect(limits).readOnly => last.act.kind \in {"Look", "None"}

\* The browser starts only through open_url, pointed at the proxy: never with launch_app.
BrowserOnlyThroughOpenUrl ==
    last.admitted /\ last.act.kind = "Launch" => last.act.app # Browser

\* With no limit in force, an agent may do anything but use a password manager or launch the browser.
NoLimitAllowsAll ==
    LET a == last.act IN
    /\ InEffect(limits) = NoLimit
    /\ ~(a.kind \in {"Launch", "Input"} /\ a.app \in Vaults)
    /\ ~(a.kind = "Launch" /\ a.app = Browser)
    => last.admitted

=============================================================================
