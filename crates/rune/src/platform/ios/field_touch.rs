//! A finger on the focused text field, and the field's menu. An iOS text field shows its menu
//! for a tap that leaves the caret where it was, keeps the selection under a long press, and
//! shows the menu again after Select All, for what is now selected. The runtime knows none of
//! this: a finger that lands in a field places the caret at once, as a mouse press does, and a
//! menu that was answered is gone.
//!
//! So the view watches the finger and the menu here, and asks the runtime for a secondary
//! press, the press that opens a field's menu, when one of these calls for it. The finger and
//! the menu are two machines stepped together, since a finger can land while a menu shows.
//!
//! Points are the view's, as UIKit reports them. What the transition needs from the runtime
//! (the field's text, whether a point is on the field, whether a menu is up) comes with the
//! event.

use std::ops::Range;

use crate::edit::ImeText;
use crate::machine::Effects;
use crate::runtime::TouchPhase;

/// How far a finger may move, in points, and still be a tap.
pub(crate) const TAP_SLOP: f32 = 10.0;

/// What the finger that last landed is doing to the focused field.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Finger {
    /// Nothing to watch: no field has the focus, or the finger landed off it.
    #[default]
    Away,
    /// It landed while the field, which held `before`, had text selected. The landing placed
    /// the caret, so a secondary press puts the selection back first. Kept after the finger
    /// lifts, until that press or the next finger.
    OnSelection { before: ImeText },
    /// Down at this point of the field, which held `before` with nothing selected, and not
    /// moved since: it may be a tap.
    Down { at: (f32, f32), before: ImeText },
    /// Lifted without moving: the frame that applies the tap tells whether the caret moved.
    Lifted { at: (f32, f32), before: ImeText },
}

/// What the field's menu is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Menu {
    /// No field menu shows.
    #[default]
    Hidden,
    /// The field's menu shows, asked for at this point.
    Showing { at: (f32, f32) },
    /// Select All was picked from the menu asked for at this point: once the next frame is
    /// done, a secondary press there opens it again. It ends at that frame, before any menu
    /// is shown or answered: a view shows a menu only after a frame, and no menu is up to
    /// answer until one is shown.
    Reopening { at: (f32, f32) },
}

/// The finger's machine and the menu's, stepped together.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct State {
    pub(crate) finger: Finger,
    pub(crate) menu: Menu,
}

impl State {
    /// Whether the next frame's end changes anything: only then is `Event::Frame`, and the
    /// copy of the field's text it carries, worth making.
    pub(crate) fn awaits_frame(&self) -> bool {
        matches!(self.finger, Finger::Lifted { .. }) || matches!(self.menu, Menu::Reopening { .. })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    /// A finger went down at `at` with no other on the view. `text` is the focused field's
    /// text and selection before the runtime hears of the finger, and `on_field` whether the
    /// point is on that field.
    Landed { at: (f32, f32), text: Option<ImeText>, on_field: bool },
    /// Any other touch event: the first finger moving or lifting, or another finger.
    Touch { phase: TouchPhase, at: (f32, f32) },
    /// A secondary press is about to reach the runtime: a long press, a mouse's secondary
    /// button, or the press `Effect::Press` asked for. `text` is the focused field's now.
    Pressing { text: Option<ImeText> },
    /// A frame is done. `text` is the focused field's now, and `popup` whether a pop-up menu
    /// shows or waits to.
    Frame { text: Option<ImeText>, popup: bool },
    /// A pop-up menu is shown at `at`: the focused field's own (`field`), or an app's.
    Shown { at: (f32, f32), field: bool },
    /// The menu showing was answered: with Select All (`select_all`), with another item, or
    /// dismissed.
    Answered { select_all: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Effect {
    /// Select this byte range of the focused field's text, before the press reaches it.
    Select(Range<usize>),
    /// Make a secondary press at this point.
    Press((f32, f32)),
    /// Ask for another frame: the press lands in the next one, which nothing else may ask for.
    Wake,
}

pub(crate) fn step(state: &mut State, event: Event) -> Effects<Effect> {
    match event {
        Event::Landed { at, text, on_field } => {
            state.finger = match text {
                Some(before) if !before.selection.is_empty() => Finger::OnSelection { before },
                Some(before) if on_field => Finger::Down { at, before },
                _ => Finger::Away,
            };
            Effects::none()
        }
        Event::Touch { phase, at: (x, y) } => {
            state.finger = match (std::mem::take(&mut state.finger), phase) {
                (Finger::Down { at, before }, TouchPhase::Moved) if (x - at.0).hypot(y - at.1) <= TAP_SLOP => Finger::Down { at, before },
                (Finger::Down { at, before }, TouchPhase::Ended) => Finger::Lifted { at, before },
                // Another finger landing leaves the first one's tap as it was.
                (Finger::Down { at, before }, TouchPhase::Started) => Finger::Down { at, before },
                // The selection a finger landed on outlives the finger.
                (Finger::OnSelection { before }, _) => Finger::OnSelection { before },
                // A drag, a cancelled touch, or a touch after the lift: not a tap.
                _ => Finger::Away,
            };
            Effects::none()
        }
        Event::Pressing { text } => {
            let Finger::OnSelection { before } = &state.finger else { return Effects::none() };
            // Only into the text the finger landed on, and only over the caret its landing
            // placed: an edit or a selection made since stands.
            let same = text.is_some_and(|now| now.text == before.text && now.start == before.start && now.selection.is_empty());
            let range = before.start + before.selection.start..before.start + before.selection.end;
            state.finger = Finger::Away;
            if same { Effects::one(Effect::Select(range)) } else { Effects::none() }
        }
        Event::Frame { text, popup } => {
            // A tap on the focused field that left its caret where it was asks for the menu,
            // unless one shows already.
            let tapped = match std::mem::take(&mut state.finger) {
                Finger::Lifted { at, before } => (text.as_ref() == Some(&before) && !popup).then_some(at),
                finger => {
                    state.finger = finger;
                    None
                }
            };
            let reopened = match state.menu {
                Menu::Reopening { at } => {
                    state.menu = Menu::Hidden;
                    Some(at)
                }
                Menu::Hidden | Menu::Showing { .. } => None,
            };
            match tapped.or(reopened) {
                Some(at) => Effects::two(Effect::Press(at), Effect::Wake),
                None => Effects::none(),
            }
        }
        Event::Shown { at, field } => {
            state.menu = if field { Menu::Showing { at } } else { Menu::Hidden };
            Effects::none()
        }
        Event::Answered { select_all } => {
            state.menu = match (state.menu, select_all) {
                // After Select All a text field shows its menu again, for what is now selected.
                (Menu::Showing { at }, true) => Menu::Reopening { at },
                _ => Menu::Hidden,
            };
            Effects::none()
        }
    }
}

#[cfg(test)]
#[path = "field_touch_tests.rs"]
mod tests;
