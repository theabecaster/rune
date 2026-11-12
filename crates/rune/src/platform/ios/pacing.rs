//! When an iOS view's display link runs, and which of its ticks draw. A `CADisplayLink` that
//! keeps drawing costs a frame of CPU and GPU sixty or a hundred and twenty times a second, so
//! a tick draws only when something may have changed, and the link rests when a frame says
//! nothing is moving. It starts again on input, a wake from another thread, or the runtime's
//! next timer. In the background it never runs: Metal refuses drawables there.
//!
//! While a finger or a two-finger gesture is down the link keeps ticking, drawing or not. A
//! link that rested between a finger's moves would start again for each one, and a started
//! link's first tick comes a refresh or two late: every move would reach the screen that much
//! behind the finger. A running link also keeps the display, and so the touches it delivers,
//! at its full rate.

use crate::machine::{Attempt, Effects, Seq};

/// What the display link is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum State {
    /// The view is in no window: there is no display link.
    Detached,
    /// The app is in the background: the link is paused until it comes forward.
    Background,
    /// Nothing is moving and nothing is held: the link is paused. `timer` is the wake the
    /// runtime asked for, if any.
    Resting { timer: Option<Attempt> },
    /// The link ticks. `held` fingers and gestures are down. A tick draws when `stale`:
    /// something may have changed since the last frame. `timer` is the wake the runtime asked
    /// for while held with nothing moving.
    Running { held: u32, stale: bool, timer: Option<Attempt> },
}

impl State {
    /// A link that has just started: its first tick draws.
    const STARTED: State = State::Running { held: 0, stale: true, timer: None };
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
    /// A finger or a two-finger gesture went down.
    Hold,
    /// One of them lifted or was cancelled. The lift itself is input, which `Wake` reports: a
    /// link that only stops being held rests on its next still frame.
    Release,
    /// The timer of this attempt came due.
    Timer(Attempt),
    /// The display link ticked.
    Tick,
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
    /// Draw a frame, then send `Event::Frame`.
    Draw,
    /// Send `Event::Timer` with this attempt after this many seconds.
    TimerAfter(Attempt, f64),
}

pub(crate) fn step(seq: &mut Seq, state: &mut State, event: Event) -> Effects<Effect> {
    match (*state, event) {
        (State::Detached, Event::Attached { foreground: true }) => {
            *state = State::STARTED;
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

        // The system cancels the touches of an app it sends to the background, so none is held
        // when it comes forward.
        (State::Background, Event::Foreground) => {
            *state = State::STARTED;
            Effects::one(Effect::Resume)
        }
        (State::Background, _) => Effects::none(),
        (_, Event::Background) => {
            *state = State::Background;
            Effects::one(Effect::Pause)
        }

        (State::Resting { .. }, Event::Wake) => {
            *state = State::STARTED;
            Effects::one(Effect::Resume)
        }
        (State::Resting { .. }, Event::Hold) => {
            *state = State::Running { held: 1, stale: true, timer: None };
            Effects::one(Effect::Resume)
        }
        (State::Resting { timer: Some(held) }, Event::Timer(attempt)) if held == attempt => {
            *state = State::STARTED;
            Effects::one(Effect::Resume)
        }
        (State::Resting { .. }, _) => Effects::none(),

        (State::Running { held, timer, .. }, Event::Wake) => {
            *state = State::Running { held, stale: true, timer };
            Effects::none()
        }
        (State::Running { held, timer, .. }, Event::Hold) => {
            *state = State::Running { held: held + 1, stale: true, timer };
            Effects::none()
        }
        (State::Running { held, stale, timer }, Event::Release) => {
            *state = State::Running { held: held.saturating_sub(1), stale, timer };
            Effects::none()
        }
        (State::Running { held, timer: Some(due), .. }, Event::Timer(attempt)) if due == attempt => {
            *state = State::Running { held, stale: true, timer: None };
            Effects::none()
        }
        (State::Running { stale: true, .. }, Event::Tick) => Effects::one(Effect::Draw),
        (State::Running { held, .. }, Event::Frame { animating: true, .. }) => {
            *state = State::Running { held, stale: true, timer: None };
            Effects::none()
        }
        (State::Running { held, .. }, Event::Frame { animating: false, wake_after }) => {
            let timer = wake_after.map(|seconds| (seq.next(), seconds.max(0.0)));
            let attempt = timer.map(|(attempt, _)| attempt);
            let mut effects = Effects::none();
            if held == 0 {
                *state = State::Resting { timer: attempt };
                effects.push(Effect::Pause);
            } else {
                *state = State::Running { held, stale: false, timer: attempt };
            }
            if let Some((attempt, seconds)) = timer { effects.push(Effect::TimerAfter(attempt, seconds)); }
            effects
        }
        (State::Running { .. }, _) => Effects::none(),
    }
}

#[cfg(test)]
#[path = "pacing_tests.rs"]
mod tests;
