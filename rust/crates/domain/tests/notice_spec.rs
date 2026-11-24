//! The notice against the properties of specs/Notice.tla, over random sequences of events for several sessions,
//! arriving in any order. The API is described at the top of tests/notice.rs.
//!
//! The test plays the world: each session's notification service is up (it shows the notice and answers its
//! id), down (the call fails) or hung (it never answers), and the user answers notices on screen. The service
//! shows a notice the moment the daemon learns its id, and "done" replaces it the moment the daemon asks, which
//! is the spec's world with its answers on the bus delivered at once. The daemon's timer may fire at any time,
//! and more than once; events for sessions that are gone, and answers on other apps' notifications, come too.
#![forbid(unsafe_code)]

use domain::notice::{Answer, Effect, Event, Notice, Notices};
use proptest::prelude::*;

type Id = u32;
type Note = u32;

/// Sessions, as the spec's `Sessions`. Session `i` has id `i + 1`; ids aren't reused.
const SESSIONS: usize = 3;

/// A notification id that is none of the daemon's: another app's.
const STRANGER: Note = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Service {
    Up,
    Down,
    Hung,
}

const SERVICES: [Service; 3] = [Service::Up, Service::Down, Service::Hung];
const ANSWERS: [Answer; 3] = [Answer::Ok, Answer::Stop, Answer::Closed];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Conn {
    New,
    Open,
    Gone,
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    /// The session opens (or says its name again, once open).
    Connect,
    Disconnect,
    /// The service takes the session's Notify call: it answers, or fails, as it does.
    ServiceAnswers,
    /// The daemon's timer for the session fires.
    Timer,
    /// The user answers the session's notice, if it is on screen.
    UserAnswers,
    /// The user answers another app's notification.
    Stranger,
    /// The user answers a notice of the session's that is no longer on screen.
    Stale,
}

const KINDS: [Kind; 7] = [
    Kind::Connect,
    Kind::Disconnect,
    Kind::ServiceAnswers,
    Kind::Timer,
    Kind::UserAnswers,
    Kind::Stranger,
    Kind::Stale,
];

/// One session, as the world sees it.
#[derive(Clone, Copy, Debug)]
struct World {
    conn: Conn,
    service: Service,
    /// A Notify call the service hasn't answered yet.
    pending: bool,
    /// The id the service gave the session's notice.
    note: Option<Note>,
    /// The notice is on screen, with OK and Stop.
    card: bool,
    /// The user answered it.
    answered: bool,
    /// The daemon's answer from the service: an id or an error.
    heard: bool,
    timer_fired: bool,
    shows: usize,
    hosts: usize,
    dones: usize,
}

impl World {
    fn new(service: Service) -> Self {
        World {
            conn: Conn::New,
            service,
            pending: false,
            note: None,
            card: false,
            answered: false,
            heard: false,
            timer_fired: false,
            shows: 0,
            hosts: 0,
            dones: 0,
        }
    }
}

fn id_of(i: usize) -> Id {
    Id::try_from(i).unwrap_or(0) + 1
}

/// The notification id the service gives session `i`'s notice.
fn note_of(i: usize) -> Note {
    Note::try_from(i).unwrap_or(0) + 7
}

fn slot(id: Id) -> Option<usize> {
    usize::try_from(id)
        .ok()?
        .checked_sub(1)
        .filter(|&i| i < SESSIONS)
}

fn named(effect: Effect<Id, Note>) -> Id {
    match effect {
        Effect::Show(k) | Effect::Host(k) | Effect::Done(k, _) | Effect::Stop(k) => k,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    #[test]
    fn every_order_of_events_keeps_the_specs_properties(
        services in proptest::array::uniform3(0..SERVICES.len()),
        picks in proptest::collection::vec((0..KINDS.len(), 0..SESSIONS, 0..ANSWERS.len()), 0..64)
    ) {
        let mut notices: Notices<Id, Note> = Notices::new();
        let mut world: Vec<World> = services.iter().map(|&s| World::new(SERVICES[s])).collect();

        for (kind, i, a) in picks {
            let k = id_of(i);
            let answer = ANSWERS[a];
            let w = world[i];
            let event = match KINDS[kind] {
                // Ids aren't reused: a session that has ended doesn't connect again.
                Kind::Connect if w.conn == Conn::Gone => continue,
                Kind::Connect => Event::Connect(k),
                Kind::Disconnect => Event::Disconnect(k),
                Kind::ServiceAnswers => {
                    if !w.pending || w.service == Service::Hung {
                        continue;
                    }
                    world[i].pending = false;
                    world[i].heard = true;
                    if w.service == Service::Up {
                        world[i].note = Some(note_of(i));
                        world[i].card = true;
                        Event::Shown(k, note_of(i))
                    } else {
                        Event::Failed(k)
                    }
                }
                Kind::Timer => {
                    if w.shows > 0 {
                        world[i].timer_fired = true;
                    }
                    Event::Timeout(k)
                }
                Kind::UserAnswers => {
                    let Some(note) = w.note.filter(|_| w.card) else { continue };
                    world[i].card = false;
                    world[i].answered = true;
                    Event::Answer(note, answer)
                }
                Kind::Stranger => Event::Answer(STRANGER, answer),
                Kind::Stale => {
                    let Some(note) = w.note.filter(|_| !w.card) else { continue };
                    Event::Answer(note, answer)
                }
            };
            if let Event::Connect(_) = event && w.conn == Conn::New {
                world[i].conn = Conn::Open;
            }
            if let Event::Disconnect(_) = event && w.conn == Conn::Open {
                world[i].conn = Conn::Gone;
            }

            let before = notices.clone();
            let effects = notices.step(event);

            // Every effect is about the event's own session: for an answer, the session whose notice it was on.
            let about = match event {
                Event::Connect(k) | Event::Disconnect(k) | Event::Shown(k, _) | Event::Failed(k) | Event::Timeout(k) => Some(k),
                Event::Answer(note, _) => (0..SESSIONS).find(|&j| note_of(j) == note).map(id_of),
            };
            for &effect in &effects {
                prop_assert_eq!(Some(named(effect)), about, "an effect for another session: {:?} gave {:?}", event, effects);
            }

            for &effect in &effects {
                let Some(j) = slot(named(effect)) else {
                    return Err(TestCaseError::fail(format!("{effect:?} for a session that never was")));
                };
                let was = before.get(id_of(j));
                match effect {
                    Effect::Show(_) => {
                        prop_assert!(matches!(event, Event::Connect(_)), "Show on {:?}", event);
                        prop_assert_eq!(world[j].conn, Conn::Open, "Show for a session that isn't open");
                        world[j].shows += 1;
                        world[j].pending = true;
                    }
                    Effect::Host(_) => {
                        prop_assert!(matches!(event, Event::Failed(_) | Event::Timeout(_)), "Host on {:?}", event);
                        prop_assert_eq!(was, Some(Notice::Asking), "the host's notice shown from {:?}, on {:?}", was, event);
                        prop_assert_eq!(world[j].conn, Conn::Open, "the host's notice for a session that has ended");
                        world[j].hosts += 1;
                    }
                    Effect::Done(_, note) => {
                        prop_assert!(matches!(event, Event::Disconnect(_) | Event::Shown(..)), "Done on {:?}", event);
                        prop_assert_eq!(world[j].conn, Conn::Gone, "NoNoticeOutlivesItsSession: done before the session ended");
                        prop_assert_eq!(Some(note), world[j].note, "done replaces a notice that isn't the session's");
                        prop_assert!(!world[j].answered, "done after the user answered: there was nothing to replace");
                        world[j].card = false;
                        world[j].dones += 1;
                    }
                    Effect::Stop(_) => {
                        // StopOnlyItsOwn, StopOnlyLive.
                        let own = matches!(event, Event::Answer(note, Answer::Stop) if Some(note) == world[j].note);
                        prop_assert!(own, "StopOnlyItsOwn: session {} stopped on {:?}", id_of(j), event);
                        prop_assert_eq!(was, world[j].note.map(Notice::Shown), "Stop on a notice that isn't shown");
                        prop_assert_eq!(world[j].conn, Conn::Open, "StopOnlyLive: stopped a session that isn't open");
                    }
                }
            }

            // StopTakesEffect: Stop on an open session's notice, shown to the daemon, stops it at once.
            if let Event::Answer(note, Answer::Stop) = event
                && let Some(j) = (0..SESSIONS).find(|&j| note_of(j) == note)
                && before.get(id_of(j)) == Some(Notice::Shown(note))
            {
                prop_assert_eq!(&effects, &vec![Effect::Stop(id_of(j))], "StopTakesEffect");
            }
            // OK and closing stop nothing.
            if let Event::Answer(_, Answer::Ok | Answer::Closed) = event {
                prop_assert_eq!(&effects, &vec![], "OK or close on {:?}", event);
            }

            for (j, w) in world.iter().enumerate() {
                let k = id_of(j);
                let now = notices.get(k);
                // AtMostOneNotice: the notice is asked for once.
                prop_assert!(w.shows <= 1, "AtMostOneNotice: session {} asked for {} notices", k, w.shows);
                prop_assert!(w.hosts <= 1, "the host showed session {}'s notice {} times", k, w.hosts);
                prop_assert!(w.dones <= 1, "session {} got {} done notices", k, w.dones);
                if w.conn != Conn::New {
                    prop_assert_eq!(w.shows, 1, "session {} connected and no notice was asked for", k);
                }
                // NoNoticeOutlivesItsSession: once every answer has come, which here is at once, no notice of an
                // ended session is on screen.
                if w.conn == Conn::Gone {
                    prop_assert!(!w.card, "NoNoticeOutlivesItsSession: session {}'s notice outlives it, after {:?}", k, event);
                    // ... and the daemon remembers it only while an answer may still come.
                    let expected = if w.pending { Some(Notice::Ending) } else { None };
                    prop_assert_eq!(now, expected, "session {} has ended, pending {}", k, w.pending);
                }
                if w.conn == Conn::Open {
                    // UserIsTold, bounded: once the service has answered, or the timer has fired, the user has
                    // been told, by the service's notice or the host's.
                    if w.heard || w.timer_fired {
                        prop_assert!(w.card || w.answered || w.hosts == 1, "UserIsTold: session {} isn't told: {:?}", k, w);
                    }
                    // What the daemon believes matches the world.
                    let expected = if w.answered {
                        Notice::Answered
                    } else if w.card {
                        Notice::Shown(note_of(j))
                    } else if w.heard {
                        Notice::Fallback
                    } else if w.timer_fired {
                        Notice::Late
                    } else {
                        Notice::Asking
                    };
                    prop_assert_eq!(now, Some(expected), "session {}: {:?}", k, w);
                }
                if w.conn == Conn::New {
                    prop_assert_eq!(now, None, "session {} hasn't connected", k);
                }
            }
        }

        // NoticeGoes: once every service that answers has answered, no notice of an ended session is left, and
        // the daemon has forgotten every ended session.
        for (j, w) in world.iter_mut().enumerate() {
            let k = id_of(j);
            if w.pending && w.service != Service::Hung {
                let event = if w.service == Service::Up {
                    w.note = Some(note_of(j));
                    w.card = true;
                    Event::Shown(k, note_of(j))
                } else {
                    Event::Failed(k)
                };
                w.pending = false;
                for effect in notices.step(event) {
                    if let Effect::Done(_, note) = effect {
                        prop_assert_eq!(Some(note), w.note, "done for another notice");
                        w.card = false;
                    }
                }
            }
            if w.conn == Conn::Gone {
                prop_assert!(!w.card, "NoticeGoes: session {}'s notice is left on screen", k);
                if !w.pending {
                    prop_assert_eq!(notices.get(k), None, "session {} has ended and is still remembered", k);
                }
            }
        }
    }
}
