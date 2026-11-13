//! A view's pop-up menu, as a state machine. The runtime hands the view one menu at a time
//! and waits for its answer: the id of the item that was picked, or nothing when the menu was
//! dismissed. UIKit says a menu is going away before it sends a tapped item's action, so a
//! dismissal waits a short grace for the pick before it counts as one. The view carries the
//! effects out: it presents the system's edit menu, moves it, starts the grace's timer and
//! answers the runtime.

use crate::machine::{Attempt, Effects, Seq};
use crate::popup_menu::Show;

/// A rect in the view's points, as `[x, y, width, height]`, at UIKit's precision.
pub(crate) type Rect = [f64; 4];

/// How long a menu that is going away may still report the item that was tapped. UIKit starts
/// taking the menu down before it sends the item's action.
pub(crate) const PICK_GRACE: f64 = 0.25;

/// Where the view's pop-up menu is. The runtime waits for an answer from `Open` until `Closed`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum State {
    #[default]
    Closed,
    /// Up, or on its way up. A text field's menu points at its caret, as UIKit's fields do,
    /// and follows it: `caret` is where it pointed last. Focusing the field brings up the
    /// keyboard, which can move the field away from where it was pressed.
    Open { show: Show, caret: Option<Rect> },
    /// Going away: a pick may still arrive, and if none has when this attempt's timer fires,
    /// the menu was dismissed.
    Leaving(Attempt),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    /// The runtime has a menu to show. `caret` is the focused field's caret when the menu is
    /// the field's own.
    Present { show: Show, caret: Option<Rect> },
    /// After a frame: where the focused field's caret is now.
    Caret(Rect),
    /// UIKit is taking the menu down.
    Dismissed,
    /// An item's action arrived, with the item's id.
    Picked(Option<String>),
    /// The grace after a dismissal ran out.
    Timer(Attempt),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Effect {
    /// Present the menu from this point of the view.
    Present { at: (f32, f32) },
    /// Ask where the menu points again: its caret moved.
    Move,
    /// Tell the runtime what was picked, and wake it.
    Answer(Option<String>),
    /// Feed `Event::Timer` with this attempt back after this many seconds.
    TimerAfter(Attempt, f64),
}

/// One transition of the menu.
pub(crate) fn step(seq: &mut Seq, state: &mut State, event: Event) -> Effects<Effect> {
    match (&mut *state, event) {
        // A menu shown over another replaces it, and the one it replaces gets no answer. The
        // grace timer of one that was leaving is stale from here on.
        (_, Event::Present { show, caret }) => {
            let at = match caret {
                Some([x, y, w, h]) => (x as f32 + w as f32 / 2.0, y as f32 + h as f32 / 2.0),
                None => show.at,
            };
            *state = State::Open { show, caret };
            Effects::one(Effect::Present { at })
        }
        (State::Open { caret: Some(at), .. }, Event::Caret(now)) if now != *at => {
            *at = now;
            Effects::one(Effect::Move)
        }
        (State::Open { .. }, Event::Dismissed) => {
            let attempt = seq.next();
            *state = State::Leaving(attempt);
            Effects::one(Effect::TimerAfter(attempt, PICK_GRACE))
        }
        (State::Open { .. } | State::Leaving(_), Event::Picked(id)) => {
            *state = State::Closed;
            Effects::one(Effect::Answer(id))
        }
        (State::Leaving(held), Event::Timer(attempt)) if *held == attempt => {
            *state = State::Closed;
            Effects::one(Effect::Answer(None))
        }
        _ => Effects::none(),
    }
}

impl State {
    /// The menu to show, while one is up.
    pub(crate) fn show(&self) -> Option<&Show> {
        match self {
            State::Open { show, .. } => Some(show),
            State::Closed | State::Leaving(_) => None,
        }
    }

    /// Whether the menu that is up points at a caret, and so moves with it.
    pub(crate) fn follows_caret(&self) -> bool {
        matches!(self, State::Open { caret: Some(_), .. })
    }

    /// What the open menu points at: its caret, else the element it was asked for, else the
    /// point. A menu that is closed or leaving has no target, and UIKit doesn't ask for one
    /// then: the rect is empty.
    pub(crate) fn target(&self) -> Rect {
        let [x, y, w, h] = match self {
            State::Open { caret: Some(rect), .. } => return *rect,
            State::Open { show: Show { element: Some(rect), .. }, .. } => *rect,
            State::Open { show, .. } => [show.at.0, show.at.1, 0.0, 0.0],
            State::Closed | State::Leaving(_) => [0.0; 4],
        };
        [x as f64, y as f64, w as f64, h as f64]
    }
}

#[cfg(test)]
#[path = "edit_menu_tests.rs"]
mod tests;
