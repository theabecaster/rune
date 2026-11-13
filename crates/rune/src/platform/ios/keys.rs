//! A hardware keyboard's keys. UIKit hands a first responder each key going down and up as a
//! `UIPress` with the key's HID usage, the modifiers held and the characters it types. The
//! keys that edit or move (arrows, Return, Escape, Tab, the function row) and anything typed
//! with Command go to the runtime as the same `KeyEvent`s a desktop window sends. A key that
//! types a character while a text field has the focus is left to UIKit, which gives it to the
//! keyboard's text input with the layout, dead keys and input method applied; with no field
//! focused it reaches the runtime as text, for an app's own shortcuts.
//!
//! Every press and release also reaches the runtime as the physical key its HID usage names, as
//! a desktop window sends them: `El::on_physical_input` hears it first, and the logical key is
//! what happens when no handler takes it. A character UIKit types into a field reaches the
//! runtime as an observation only, since the field has it already.
//!
//! UIKit reports a held key once, so the repeat is made here: after a delay the key is sent
//! again at a steady rate until it lifts or another key goes down.

use crate::machine::{Attempt, Effects, Seq};
use crate::{Key, KeyEvent, Mods, PhysicalKeyCode};

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

/// What one key going down or up says to the runtime: the key it is, the modifiers held, the
/// logical key when the runtime takes it (`event`), and the character it types unmodified
/// (`Tx::key_base`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Stroke {
    pub(crate) code: Option<PhysicalKeyCode>,
    pub(crate) mods: Mods,
    pub(crate) key: Option<KeyEvent>,
    pub(crate) base: Option<char>,
}

/// What `press` says to the runtime, or none when it names no key the runtime knows and types
/// nothing it takes (a modifier UIKit has no code for).
pub(crate) fn stroke(press: &Press, typing: bool) -> Option<Stroke> {
    let code = code(press.usage);
    let key = event(press, typing);
    let mut base = press.base.chars();
    let base = match (base.next(), base.next()) {
        (Some(c), None) if !c.is_control() => c.to_lowercase().next(),
        _ => None,
    };
    (code.is_some() || key.is_some()).then(|| Stroke { code, mods: mods(press.flags), key, base })
}

/// The physical key a `UIKeyboardHIDUsage` names: the USB HID keyboard page, as every
/// platform's scancodes map to it.
pub(crate) fn code(usage: isize) -> Option<PhysicalKeyCode> {
    use PhysicalKeyCode::*;
    const LETTERS: [PhysicalKeyCode; 26] = [KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM,
        KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ];
    const DIGITS: [PhysicalKeyCode; 10] = [Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9, Digit0];
    const FUNCTIONS: [PhysicalKeyCode; 12] = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    const FUNCTIONS_HIGH: [PhysicalKeyCode; 12] = [F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24];
    const NUMPAD: [PhysicalKeyCode; 10] = [Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9, Numpad0];
    let at = |table: &[PhysicalKeyCode], first: isize| usize::try_from(usage - first).ok().and_then(|i| table.get(i).copied());
    Some(match usage {
        0x04..=0x1D => return at(&LETTERS, 0x04),
        0x1E..=0x27 => return at(&DIGITS, 0x1E),
        0x28 => Enter,
        0x29 => Escape,
        0x2A => Backspace,
        0x2B => Tab,
        0x2C => Space,
        0x2D => Minus,
        0x2E => Equal,
        0x2F => BracketLeft,
        0x30 => BracketRight,
        0x31 | 0x32 => Backslash,
        0x33 => Semicolon,
        0x34 => Quote,
        0x35 => Backquote,
        0x36 => Comma,
        0x37 => Period,
        0x38 => Slash,
        0x39 => CapsLock,
        0x3A..=0x45 => return at(&FUNCTIONS, 0x3A),
        0x46 => PrintScreen,
        0x47 => ScrollLock,
        0x48 => Pause,
        0x49 => Insert,
        0x4A => Home,
        0x4B => PageUp,
        0x4C => Delete,
        0x4D => End,
        0x4E => PageDown,
        0x4F => ArrowRight,
        0x50 => ArrowLeft,
        0x51 => ArrowDown,
        0x52 => ArrowUp,
        0x53 => NumLock,
        0x54 => NumpadDivide,
        0x55 => NumpadMultiply,
        0x56 => NumpadSubtract,
        0x57 => NumpadAdd,
        0x58 => NumpadEnter,
        0x59..=0x62 => return at(&NUMPAD, 0x59),
        0x63 => NumpadDecimal,
        0x64 => IntlBackslash,
        0x65 => ContextMenu,
        0x67 => NumpadEqual,
        0x68..=0x73 => return at(&FUNCTIONS_HIGH, 0x68),
        0x87 => IntlRo,
        0x89 => IntlYen,
        0xE0 => ControlLeft,
        0xE1 => ShiftLeft,
        0xE2 => AltLeft,
        0xE3 => SuperLeft,
        0xE4 => ControlRight,
        0xE5 => ShiftRight,
        0xE6 => AltRight,
        0xE7 => SuperRight,
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
    /// The key with this usage is down and sends `stroke` again when `attempt`'s timer fires.
    Held { usage: isize, stroke: Stroke, attempt: Attempt },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    /// A key went down.
    Down { usage: isize, stroke: Stroke },
    /// A key went up, or the system cancelled its press.
    Up { usage: isize, stroke: Stroke },
    /// Every key is up as far as the view knows: the app went to the background.
    Reset,
    Timer(Attempt),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Effect {
    /// Send the runtime this key going down, or with `repeat` held down still.
    Down { stroke: Stroke, repeat: bool },
    /// Send the runtime this key going up.
    Up(Stroke),
    /// Send `Event::Timer` with this attempt after this many seconds.
    TimerAfter(Attempt, f64),
}

pub(crate) fn step(seq: &mut Seq, state: &mut Repeat, event: Event) -> Effects<Effect> {
    match (&*state, event) {
        // The newest key repeats, as on every desktop: one going down ends another's repeat.
        // Only a key the runtime takes repeats: UIKit repeats what it types itself.
        (_, Event::Down { usage, stroke }) if stroke.key.is_some() => {
            let attempt = seq.next();
            *state = Repeat::Held { usage, stroke: stroke.clone(), attempt };
            Effects::two(Effect::Down { stroke, repeat: false }, Effect::TimerAfter(attempt, REPEAT_DELAY))
        }
        (_, Event::Down { stroke, .. }) => {
            *state = Repeat::Idle;
            Effects::one(Effect::Down { stroke, repeat: false })
        }
        (Repeat::Held { usage: held, .. }, Event::Up { usage, stroke }) if *held == usage => {
            *state = Repeat::Idle;
            Effects::one(Effect::Up(stroke))
        }
        (_, Event::Up { stroke, .. }) => Effects::one(Effect::Up(stroke)),
        (_, Event::Reset) => {
            *state = Repeat::Idle;
            Effects::none()
        }
        (Repeat::Held { usage, stroke, attempt: held }, Event::Timer(attempt)) if *held == attempt => {
            let (usage, stroke, attempt) = (*usage, stroke.clone(), seq.next());
            *state = Repeat::Held { usage, stroke: stroke.clone(), attempt };
            Effects::two(Effect::Down { stroke, repeat: true }, Effect::TimerAfter(attempt, REPEAT_EVERY))
        }
        (_, Event::Timer(_)) => Effects::none(),
    }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
