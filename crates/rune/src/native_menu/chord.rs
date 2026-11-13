//! Which item a key no handler took runs: the first enabled one with the key as its shortcut
//! that UIKit doesn't run for it. On an iPad an item with a shortcut is a key command of the
//! main menu, so only a `keys_first` item is left to the app's keys; an iPhone has no main
//! menu, so there every item is. Plain data, testable on every platform.

use super::Description;
use crate::KeyEvent;

/// The first enabled item with `key` as its shortcut that UIKit doesn't run for it: a
/// `keys_first` item, or any item where there is no `menu_bar`. Never a separator, which
/// has nothing to run.
pub(crate) fn chord_item<'a>(items: &'a [Description], key: &KeyEvent, menu_bar: bool) -> Option<&'a Description> {
    items.iter().find(|d| !d.separator && (d.keys_first || !menu_bar) && d.enabled
        && d.equivalent.as_ref().is_some_and(|s| s.matches_key(key)))
}

#[cfg(test)]
#[path = "chord_tests.rs"]
mod tests;
