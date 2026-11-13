//! An App's menu items laid out as a menu system without one: UIKit's builds its menus from
//! scratch, so the order AppKit's insertions make is worked out here. Items go to the app menu
//! or a named one, in the order given; an item `after` an earlier one of its menu follows it,
//! and several after one anchor keep their order, as on macOS. An anchor that names no
//! earlier item of the menu (an AppKit action such as `"paste:"`) leaves the item at the end.
//! Separators split a menu into the sections UIKit draws apart. Dock items have no place.
//! Plain data, testable on every platform.

use super::{Description, Location};

/// One menu's items: the app menu (`menu` none) or a named one, as sections.
pub(crate) struct Group<'a> {
    pub(crate) menu: Option<&'a str>,
    pub(crate) sections: Vec<Vec<&'a Description>>,
}

/// `items` by menu, the app menu first, then the named ones in the order they first appear.
pub(crate) fn groups(items: &[Description]) -> Vec<Group<'_>> {
    let mut menus: Vec<Option<&str>> = Vec::new();
    for d in items {
        let menu = match &d.location {
            Location::App => None,
            Location::Named(name) => Some(name.as_str()),
            Location::Dock => continue,
        };
        if !menus.contains(&menu) { menus.push(menu); }
    }
    menus.sort_by_key(Option::is_some);
    menus.into_iter().map(|menu| {
        let mine = items.iter().filter(|d| match &d.location {
            Location::App => menu.is_none(),
            Location::Named(name) => menu == Some(name.as_str()),
            Location::Dock => false,
        });
        Group { menu, sections: sections(ordered(mine)) }
    }).collect()
}

/// One menu's items with each anchored one placed after its anchor.
fn ordered<'a>(items: impl Iterator<Item = &'a Description>) -> Vec<&'a Description> {
    let mut out: Vec<&Description> = Vec::new();
    // The last item placed after each anchor, so the next one goes after it.
    let mut chained: Vec<(&str, &str)> = Vec::new();
    for d in items {
        let placed = |id: &str| out.iter().position(|o| !o.separator && o.id == id);
        let anchor = d.after.as_deref().and_then(|anchor| {
            let last = chained.iter().find(|(a, _)| *a == anchor).map(|(_, last)| *last);
            last.and_then(placed).or_else(|| placed(anchor)).map(|at| (anchor, at))
        });
        match anchor {
            Some((anchor, at)) => {
                out.insert(at + 1, d);
                chained.retain(|(a, _)| *a != anchor);
                if !d.separator { chained.push((anchor, &d.id)); }
            }
            None => out.push(d),
        }
    }
    out
}

/// `items` split at its separators, with no empty section.
fn sections(items: Vec<&Description>) -> Vec<Vec<&Description>> {
    items.split(|d| d.separator).filter(|s| !s.is_empty()).map(<[_]>::to_vec).collect()
}

#[cfg(test)]
#[path = "groups_tests.rs"]
mod tests;
