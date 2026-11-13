use super::*;
use crate::native_menu::MenuItem;
use crate::{Key, Shortcut};

fn shortcut(accelerator: &str) -> Shortcut { accelerator.parse().unwrap() }

/// An item with `accelerator` as its shortcut, its id its title.
fn item(id: &str, accelerator: &str) -> MenuItem<()> { MenuItem::new(id, |_, _| {}).key_equivalent(shortcut(accelerator)) }

fn d(item: MenuItem<()>) -> Description { item.description }

/// The key event a press of `accelerator`'s chord makes.
fn press(accelerator: &str) -> KeyEvent {
    let shortcut = shortcut(accelerator);
    let c = accelerator.chars().last().unwrap();
    KeyEvent { key: Key::Command(c), mods: shortcut.modifiers() }
}

fn found<'a>(items: &'a [Description], accelerator: &str, menu_bar: bool) -> Option<&'a str> {
    chord_item(items, &press(accelerator), menu_bar).map(|d| d.id.as_str())
}

#[test]
fn a_keys_first_item_takes_its_chord_with_a_menu_bar_and_without_one() {
    let items = [d(item("panel", "cmd+k").keys_first())];
    assert_eq!(found(&items, "cmd+k", true), Some("panel"));
    assert_eq!(found(&items, "cmd+k", false), Some("panel"));
}

#[test]
fn a_plain_item_takes_its_chord_only_where_there_is_no_menu_bar() {
    let items = [d(item("panel", "cmd+k"))];
    assert_eq!(found(&items, "cmd+k", true), None, "UIKit runs the key command itself");
    assert_eq!(found(&items, "cmd+k", false), Some("panel"));
}

#[test]
fn a_disabled_item_never_takes_its_chord() {
    let items = [d(item("panel", "cmd+k").keys_first().enabled(false))];
    assert_eq!(found(&items, "cmd+k", true), None);
    assert_eq!(found(&items, "cmd+k", false), None);
}

#[test]
fn an_item_without_a_shortcut_takes_no_chord() {
    let items = [d(MenuItem::<()>::new("panel", |_, _| {}).keys_first())];
    assert_eq!(found(&items, "cmd+k", true), None);
    assert_eq!(found(&items, "cmd+k", false), None);
}

#[test]
fn the_first_of_two_items_with_a_chord_takes_it() {
    let items = [d(item("first", "cmd+k")), d(item("second", "cmd+k"))];
    assert_eq!(found(&items, "cmd+k", false), Some("first"));
}

#[test]
fn an_item_the_menu_bar_or_its_state_rules_out_leaves_the_chord_to_the_next() {
    let items = [d(item("off", "cmd+k").enabled(false)), d(item("plain", "cmd+k")), d(item("first", "cmd+k").keys_first())];
    assert_eq!(found(&items, "cmd+k", true), Some("first"));
    assert_eq!(found(&items, "cmd+k", false), Some("plain"));
}

#[test]
fn a_chord_no_item_has_finds_none() {
    let items = [d(item("panel", "cmd+k").keys_first()), d(item("save", "cmd+shift+s").keys_first())];
    assert_eq!(found(&items, "cmd+j", false), None);
    assert_eq!(found(&items, "cmd+shift+k", false), None, "Shift makes it another chord");
    assert_eq!(found(&items, "cmd+s", false), None);
    assert_eq!(found(&items, "cmd+shift+s", false), Some("save"));
}
