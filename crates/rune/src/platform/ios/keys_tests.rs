use super::*;

fn press(usage: isize, flags: isize, base: &str, typed: &str) -> Press {
    Press { usage, flags, base: base.into(), typed: typed.into() }
}

fn key(press: &Press, typing: bool) -> Option<Key> { event(press, typing).map(|event| event.key.clone()) }

fn run(seq: &mut Seq, state: &mut Repeat, event: Event) -> Vec<Effect> {
    step(seq, state, event).into_iter().collect()
}

/// What the left arrow says going down or up.
fn left() -> Stroke { Stroke { code: Some(PhysicalKeyCode::ArrowLeft), mods: Mods::default(), key: Some(KeyEvent { key: Key::Left, mods: Mods::default() }), base: None } }

fn right() -> Stroke { Stroke { code: Some(PhysicalKeyCode::ArrowRight), key: Some(KeyEvent { key: Key::Right, mods: Mods::default() }), ..left() } }

/// An "a" typed into a field: UIKit types it, the runtime only hears the key.
fn typed_a() -> Stroke { Stroke { code: Some(PhysicalKeyCode::KeyA), mods: Mods::default(), key: None, base: Some('a') } }

#[test]
fn keys_that_type_nothing_are_named_by_their_usage_with_or_without_a_field() {
    for typing in [false, true] {
        assert_eq!(key(&press(0x50, 0, "UIKeyInputLeftArrow", "UIKeyInputLeftArrow"), typing), Some(Key::Left));
        assert_eq!(key(&press(0x28, 0, "\r", "\r"), typing), Some(Key::Enter));
        assert_eq!(key(&press(0x29, 0, "UIKeyInputEscape", "UIKeyInputEscape"), typing), Some(Key::Escape));
        assert_eq!(key(&press(0x2A, 0, "\u{8}", "\u{8}"), typing), Some(Key::Backspace));
        assert_eq!(key(&press(0x4C, 0, "\u{7f}", "\u{7f}"), typing), Some(Key::Delete));
        assert_eq!(key(&press(0x3A, 0, "UIKeyInputF1", "UIKeyInputF1"), typing), Some(Key::Function(1)));
        assert_eq!(key(&press(0x45, 0, "UIKeyInputF12", "UIKeyInputF12"), typing), Some(Key::Function(12)));
        assert_eq!(key(&press(0x68, 0, "", ""), typing), Some(Key::Function(13)));
    }
}

#[test]
fn modifiers_ride_with_the_key() {
    let event = event(&press(0x50, SHIFT | ALTERNATE, "UIKeyInputLeftArrow", "UIKeyInputLeftArrow"), true).unwrap();
    assert_eq!(event, KeyEvent { key: Key::Left, mods: Mods { shift: true, alt: true, ..Mods::default() } });
    let event = super::event(&press(0x2A, COMMAND, "\u{8}", "\u{8}"), true).unwrap();
    assert_eq!((event.key.clone(), event.mods.cmd, event.mods.logo), (Key::Backspace, true, true));
}

#[test]
fn a_character_typed_with_command_is_a_command_in_lower_case() {
    assert_eq!(key(&press(0x19, COMMAND, "v", "v"), true), Some(Key::Command('v')));
    assert_eq!(key(&press(0x1D, COMMAND | SHIFT, "Z", "Z"), true), Some(Key::Command('z')));
    assert_eq!(key(&press(0x36, COMMAND, ",", ","), false), Some(Key::Command(',')));
    assert_eq!(key(&press(0x2C, COMMAND, " ", " "), false), Some(Key::Text(" ".into())), "Command-Space is the space, as on a Mac");
}

#[test]
fn a_character_is_uikits_while_a_field_has_the_focus_and_text_otherwise() {
    let a = press(0x04, 0, "a", "a");
    assert_eq!(key(&a, true), None);
    assert_eq!(key(&a, false), Some(Key::Text("a".into())));
    assert_eq!(key(&press(0x04, ALTERNATE, "a", "å"), false), Some(Key::Text("å".into())));
    assert_eq!(key(&press(0x04, CONTROL, "a", "\u{1}"), false), None, "Control-A types a control character: no text");
}

#[test]
fn a_modifier_on_its_own_is_no_key() {
    assert_eq!(key(&press(0xE1, SHIFT, "", ""), false), None);
    assert_eq!(key(&press(0xE3, COMMAND, "", ""), true), None);
}

#[test]
fn a_held_key_repeats_after_a_delay_and_then_steadily_until_it_lifts() {
    let (mut seq, mut state) = (Seq::default(), Repeat::Idle);
    let first = run(&mut seq, &mut state, Event::Down { usage: 0x50, stroke: left() });
    let attempt = seq.current();
    assert_eq!(first, [Effect::Down { stroke: left(), repeat: false }, Effect::TimerAfter(attempt, REPEAT_DELAY)]);
    let again = run(&mut seq, &mut state, Event::Timer(attempt));
    let attempt = seq.current();
    assert_eq!(again, [Effect::Down { stroke: left(), repeat: true }, Effect::TimerAfter(attempt, REPEAT_EVERY)]);
    assert_eq!(run(&mut seq, &mut state, Event::Up { usage: 0x50, stroke: left() }), [Effect::Up(left())]);
    assert_eq!(state, Repeat::Idle);
    assert_eq!(run(&mut seq, &mut state, Event::Timer(attempt)), [], "the timer of a lifted key sends nothing");
}

#[test]
fn the_newest_key_repeats_and_an_earlier_keys_timer_and_lift_change_nothing_but_its_release() {
    let (mut seq, mut state) = (Seq::default(), Repeat::Idle);
    run(&mut seq, &mut state, Event::Down { usage: 0x50, stroke: left() });
    let stale = seq.current();
    run(&mut seq, &mut state, Event::Down { usage: 0x4F, stroke: right() });
    assert_eq!(run(&mut seq, &mut state, Event::Timer(stale)), []);
    assert_eq!(run(&mut seq, &mut state, Event::Up { usage: 0x50, stroke: left() }), [Effect::Up(left())]);
    let attempt = seq.current();
    assert_eq!(run(&mut seq, &mut state, Event::Timer(attempt))[0], Effect::Down { stroke: right(), repeat: true });
}

#[test]
fn a_key_uikit_types_is_heard_once_and_never_repeated_here() {
    let (mut seq, mut state) = (Seq::default(), Repeat::Idle);
    run(&mut seq, &mut state, Event::Down { usage: 0x50, stroke: left() });
    let held = seq.current();
    assert_eq!(run(&mut seq, &mut state, Event::Down { usage: 0x04, stroke: typed_a() }), [Effect::Down { stroke: typed_a(), repeat: false }]);
    assert_eq!(state, Repeat::Idle, "it ends the arrow's repeat, as any new key does");
    assert_eq!(run(&mut seq, &mut state, Event::Timer(held)), []);
    assert_eq!(run(&mut seq, &mut state, Event::Up { usage: 0x04, stroke: typed_a() }), [Effect::Up(typed_a())]);
}

#[test]
fn a_reset_ends_the_repeat() {
    let (mut seq, mut state) = (Seq::default(), Repeat::Idle);
    run(&mut seq, &mut state, Event::Down { usage: 0x50, stroke: left() });
    let attempt = seq.current();
    assert_eq!(run(&mut seq, &mut state, Event::Reset), []);
    assert_eq!(run(&mut seq, &mut state, Event::Timer(attempt)), []);
}

#[test]
fn usages_name_the_keys_of_the_hid_keyboard_page() {
    use PhysicalKeyCode::*;
    let cases = [(0x04, KeyA), (0x1D, KeyZ), (0x1E, Digit1), (0x27, Digit0), (0x28, Enter), (0x2C, Space), (0x31, Backslash),
        (0x3A, F1), (0x45, F12), (0x68, F13), (0x73, F24), (0x4F, ArrowRight), (0x52, ArrowUp), (0x59, Numpad1), (0x62, Numpad0),
        (0x64, IntlBackslash), (0xE0, ControlLeft), (0xE3, SuperLeft), (0xE7, SuperRight)];
    for (usage, key) in cases { assert_eq!(code(usage), Some(key), "usage {usage:#x}"); }
    for usage in [0x00, 0x03, 0x66, 0x74, 0xE8, -1] { assert_eq!(code(usage), None, "usage {usage:#x}"); }
}

#[test]
fn a_stroke_carries_the_physical_key_the_logical_one_the_runtime_takes_and_the_unmodified_character() {
    let typed = stroke(&press(0x04, SHIFT, "a", "A"), true).unwrap();
    assert_eq!(typed, Stroke { code: Some(PhysicalKeyCode::KeyA), mods: Mods { shift: true, ..Mods::default() }, key: None, base: Some('a') });
    let command = stroke(&press(0x19, COMMAND, "v", "v"), true).unwrap();
    assert_eq!((command.code, command.key.map(|k| k.key.clone()), command.base), (Some(PhysicalKeyCode::KeyV), Some(Key::Command('v')), Some('v')));
    let arrow = stroke(&press(0x50, 0, "UIKeyInputLeftArrow", "UIKeyInputLeftArrow"), true).unwrap();
    assert_eq!((arrow.code, arrow.base), (Some(PhysicalKeyCode::ArrowLeft), None));
    assert_eq!(stroke(&press(0x66, 0, "", ""), false), None, "no code and nothing typed: nothing to say");
}
