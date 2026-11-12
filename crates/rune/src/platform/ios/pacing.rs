//! When an iOS view's display link runs. A `CADisplayLink` that keeps ticking costs a frame of
//! CPU and GPU sixty or a hundred and twenty times a second, so it runs only while there is
//! something to draw: it rests when a frame says nothing is moving, and starts again on input,
//! a wake from another thread, or the runtime's next timer. In the background it never runs:
//! Metal refuses drawables there.

use crate::machine::{Attempt, Effects, Seq};

/// What the display link is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum State {
    /// The view is in no window: there is no display link.
    Detached,
    /// The app is in the background: the link is paused until it comes forward.
    Background,
    /// Nothing is moving: the link is paused. `timer` is the wake the runtime asked for, if any.
    Resting { timer: Option<Attempt> },
    /// The link ticks and each tick draws.
    Running,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Event {
    /// The view joined a window, in an app that is in the foreground or not.
    Attached { foreground: bool },
    /// The view left its window.
    Detached,
    /// The app came forward.
    Foreground,
    /// The app went to the background.
    Background,
    /// Something may need drawing: a touch, a resize, a wake from another thread.
    Wake,
    /// The timer of this attempt came due.
    Timer(Attempt),
    /// A tick drew: whether anything is still moving, and the seconds until the runtime's next
    /// timer when it has one.
    Frame { animating: bool, wake_after: Option<f64> },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Effect {
    /// Make the display link, paused.
    Create,
    /// Invalidate the display link.
    Destroy,
    /// Let the link tick.
    Resume,
    /// Stop its ticks.
    Pause,
    /// Send `Event::Timer` with this attempt after this many seconds.
    TimerAfter(Attempt, f64),
}

pub(crate) fn step(seq: &mut Seq, state: &mut State, event: Event) -> Effects<Effect> {
    match (*state, event) {
        (State::Detached, Event::Attached { foreground: true }) => {
            *state = State::Running;
            Effects::two(Effect::Create, Effect::Resume)
        }
        (State::Detached, Event::Attached { foreground: false }) => {
            *state = State::Background;
            Effects::one(Effect::Create)
        }
        (State::Detached, _) => Effects::none(),
        (_, Event::Detached) => {
            *state = State::Detached;
            Effects::one(Effect::Destroy)
        }
        (_, Event::Attached { .. }) => Effects::none(),

        (State::Background, Event::Foreground) => {
            *state = State::Running;
            Effects::one(Effect::Resume)
        }
        (State::Background, _) => Effects::none(),
        (_, Event::Background) => {
            *state = State::Background;
            Effects::one(Effect::Pause)
        }

        (State::Resting { .. }, Event::Wake) => {
            *state = State::Running;
            Effects::one(Effect::Resume)
        }
        (State::Resting { timer: Some(held) }, Event::Timer(attempt)) if held == attempt => {
            *state = State::Running;
            Effects::one(Effect::Resume)
        }
        (State::Resting { .. }, _) => Effects::none(),

        (State::Running, Event::Frame { animating: false, wake_after }) => match wake_after {
            Some(seconds) => {
                let attempt = seq.next();
                *state = State::Resting { timer: Some(attempt) };
                Effects::two(Effect::Pause, Effect::TimerAfter(attempt, seconds.max(0.0)))
            }
            None => {
                *state = State::Resting { timer: None };
                Effects::one(Effect::Pause)
            }
        },
        (State::Running, _) => Effects::none(),
    }
}

#[cfg(test)]
#[path = "pacing_tests.rs"]
mod tests;
