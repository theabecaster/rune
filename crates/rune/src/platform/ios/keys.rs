//! A hardware keyboard's keys. UIKit hands a first responder each key going down and up as a
//! `UIPress` with the key's HID usage, the modifiers held and the characters it types. The
//! keys that edit or move (arrows, Return, Escape, Tab, the function row) and anything typed
//! with Command go to the runtime as the same `KeyEvent`s a desktop window sends. A key that
//! types a character while a text field has the focus is left to UIKit, which gives it to the
//! keyboard's text input with the layout, dead keys and input method applied; with no field
//! focused it reaches the runtime as text, for an app's own shortcuts.
//!
//! UIKit reports a held key once, so the repeat is made here: after a delay the key is sent
//! again at a steady rate until it lifts or another key goes down.

use crate::machine::{Attempt, Effects, Seq};
use crate::{Key, KeyEvent, Mods};

/// Seconds a key is held before it repeats, and between repeats: iOS's own defaults.
pub(crate) const REPEAT_DELAY: f64 = 0.4;
pub(crate) const REPEAT_EVERY: f64 = 0.05;

/// `UIKeyModifierFlags`.
const SHIFT: isize = 1 << 17;
const CONTROL: isize = 1 << 18;
const ALTERNATE: isize = 1 << 19;
const COMMAND: isize = 1 << 20;

/// One key of a `UIPress`: its `UIKeyboardHIDUsage`, its `UIKeyModifierFlags`, what it types
/// with no modifier but Shift's case removed (`charactersIgnoringModifiers`), and what it
/// types as held (`characters`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Press {
    pub(crate) usage: isize,
    pub(crate) flags: isize,
    pub(crate) base: String,
    pub(crate) typed: String,
}

fn mods(flags: isize) -> Mods {
    let command = flags & COMMAND != 0;
    Mods { shift: flags & SHIFT != 0, cmd: command, alt: flags & ALTERNATE != 0, ctrl: flags & CONTROL != 0, logo: command }
}

/// The key a HID usage names, for the keys that type no character.
fn named(usage: isize) -> Option<Key> {
    Some(match usage {
        0x28 | 0x58 => Key::Enter,
        0x29 => Key::Escape,
        0x2A => Key::Backspace,
        0x2B => Key::Tab,
        0x3A..=0x45 => Key::Function((usage - 0x3A + 1) as u8),
        0x68..=0x73 => Key::Function((usage - 0x68 + 13) as u8),
        0x46 => Key::PrintScreen,
        0x47 => Key::ScrollLock,
        0x48 => Key::Pause,
        0x49 => Key::Insert,
        0x4A => Key::Home,
        0x4B => Key::PageUp,
        0x4C => Key::Delete,
        0x4D => Key::End,
        0x4E => Key::PageDown,
        0x4F => Key::Right,
        0x50 => Key::Left,
        0x51 => Key::Down,
        0x52 => Key::Up,
        0x65 => Key::ContextMenu,
        _ => return None,
    })
}

/// What the runtime hears for `press`, or none when the key is UIKit's to handle: a character
/// for the focused text field (`typing`), or a modifier on its own.
pub(crate) fn event(press: &Press, typing: bool) -> Option<KeyEvent> {
    let mods = mods(press.flags);
    if let Some(key) = named(press.usage) { return Some(KeyEvent { key, mods }); }
    let mut base = press.base.chars();
    if mods.cmd && let (Some(c), None) = (base.next(), base.next()) && !c.is_control() {
        // Command-Space arrives as the space it would type, as on a Mac.
        let key = if c == ' ' { Key::Text(" ".into()) } else { Key::Command(c.to_lowercase().next().unwrap_or(c)) };
        return Some(KeyEvent { key, mods });
    }
    let types = !press.typed.is_empty() && !press.typed.chars().any(char::is_control);
    (!typing && types).then(|| KeyEvent { key: Key::Text(press.typed.clone()), mods })
}

/// The key that repeats, if one is held.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Repeat {
    #[default]
    Idle,
    /// The key with this usage is down and sends `event` again when `attempt`'s timer fires.
    Held { usage: isize, event: KeyEvent, attempt: Attempt },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    /// A key the runtime hears went down.
    Down { usage: isize, event: KeyEvent },
    /// A key went up, or the system cancelled its press.
    Up { usage: isize },
    /// Every key is up as far as the view knows: it left its window or the app went back.
    Reset,
    Timer(Attempt),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Effect {
    /// Send the runtime this key.
    Send(KeyEvent),
    /// Send `Event::Timer` with this attempt after this many seconds.
    TimerAfter(Attempt, f64),
}

pub(crate) fn step(seq: &mut Seq, state: &mut Repeat, event: Event) -> Effects<Effect> {
    match (&*state, event) {
        // The newest key repeats, as on every desktop: one going down ends another's repeat.
        (_, Event::Down { usage, event }) => {
            let attempt = seq.next();
            *state = Repeat::Held { usage, event: event.clone(), attempt };
            Effects::two(Effect::Send(event), Effect::TimerAfter(attempt, REPEAT_DELAY))
        }
        (Repeat::Held { usage: held, .. }, Event::Up { usage }) if *held == usage => {
            *state = Repeat::Idle;
            Effects::none()
        }
        (_, Event::Reset) => {
            *state = Repeat::Idle;
            Effects::none()
        }
        (Repeat::Held { usage, event, attempt: held }, Event::Timer(attempt)) if *held == attempt => {
            let (usage, event, attempt) = (*usage, event.clone(), seq.next());
            *state = Repeat::Held { usage, event: event.clone(), attempt };
            Effects::two(Effect::Send(event), Effect::TimerAfter(attempt, REPEAT_EVERY))
        }
        (_, Event::Up { .. } | Event::Timer(_)) => Effects::none(),
    }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
