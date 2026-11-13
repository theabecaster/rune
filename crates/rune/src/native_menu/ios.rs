//! An App's items in UIKit's main menu: the iPad's menu bar and the list a held ⌘ shows.
//! UIKit builds its menus from scratch whenever asked (`buildMenuWithBuilder:` on the app's
//! delegate, which `rune::run` supplies), so the items are kept here and the builder is asked
//! to run again when they change. App-menu items follow About; an item in a named menu goes
//! at the end of the system's menu of that name (File, Edit, Format, View, Window, Help), or
//! into a menu of its own before Window. Separators split a menu into sections.
//!
//! Each item is a command whose action, `runeMenuItem:`, travels the responder chain to the
//! `RuneView` that holds the keyboard, which hands the item's id back here. On an iPad an item
//! with a shortcut is a key command, so UIKit runs it for the chord. A `keys_first` item shows
//! no shortcut: its chord goes to the app's handlers like any key, and the item runs only if
//! none takes it (`keys_first`). An iPhone has no main menu, so there every item's chord works
//! that way.

use std::cell::RefCell;

use objc2_foundation_video::NSString;
use objc2_video::rc::Retained;
use objc2_video::runtime::AnyObject;
use objc2_video::{class, msg_send, sel};

use super::chord::chord_item;
use super::groups::{Group, groups};
use super::key_equivalent::uikit_input;
use super::{Description, Sink};
use crate::runtime::Unhandled;

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    static UIMenuAbout: &'static NSString;
    static UIMenuFile: &'static NSString;
    static UIMenuEdit: &'static NSString;
    static UIMenuFormat: &'static NSString;
    static UIMenuView: &'static NSString;
    static UIMenuWindow: &'static NSString;
    static UIMenuHelp: &'static NSString;
}

/// `UIMenuElementAttributesDisabled`, `UIMenuElementStateOn` and `UIMenuOptionsDisplayInline`.
const DISABLED: usize = 1 << 0;
const ON: isize = 1;
const INLINE: usize = 1 << 0;

struct Installed { items: Vec<Description>, sink: Sink }

thread_local! {
    static INSTALLED: RefCell<Option<Installed>> = const { RefCell::new(None) };
}

/// Puts `items` in the main menu, replacing the ones there; none takes them all out.
pub(super) fn set(items: &[Description], sink: Sink) {
    let next = (!items.is_empty()).then(|| Installed { items: items.to_vec(), sink });
    INSTALLED.with(|installed| *installed.borrow_mut() = next);
    // SAFETY: the main menu system is UIKit's; asking it to rebuild calls the app's builder.
    unsafe {
        let system: Retained<AnyObject> = msg_send![class!(UIMenuSystem), mainSystem];
        let _: () = msg_send![&*system, setNeedsRebuild];
    }
}

/// Adds the items to the menus `builder` (a `UIMenuBuilder`) is building.
pub(crate) fn build(builder: &AnyObject) {
    INSTALLED.with(|installed| {
        let Some(installed) = &*installed.borrow() else { return };
        for group in groups(&installed.items) { place(builder, &group); }
    });
}

/// Runs the item `id` names, as a click on it does. False when no item has that id.
pub(crate) fn perform(id: &str) -> bool {
    let sink = INSTALLED.with(|installed| installed.borrow().as_ref()
        .filter(|i| i.items.iter().any(|d| !d.separator && d.id == id)).map(|i| i.sink.clone()));
    sink.inspect(|sink| sink(id)).is_some()
}

/// What a key no handler takes does when an item has it as its shortcut and UIKit doesn't run
/// the item for it: a `keys_first` item's, or any item's where there is no main menu.
pub(crate) fn keys_first(key: &crate::KeyEvent) -> Option<Unhandled> {
    let menu_bar = has_main_menu();
    INSTALLED.with(|installed| {
        let installed = installed.borrow();
        let installed = installed.as_ref()?;
        let item = chord_item(&installed.items, key, menu_bar)?;
        let (id, sink) = (item.id.clone(), installed.sink.clone());
        Some(Unhandled::Menu(std::sync::Arc::new(move || sink(&id))))
    })
}

/// Whether UIKit runs the main menu's key commands: on an iPad, not on an iPhone.
fn has_main_menu() -> bool {
    /// `UIUserInterfaceIdiomPad`.
    const PAD: isize = 1;
    thread_local! { static PAD_IDIOM: std::cell::OnceCell<bool> = const { std::cell::OnceCell::new() }; }
    // SAFETY: the current device and its idiom, an `NSInteger` enum.
    PAD_IDIOM.with(|pad| *pad.get_or_init(|| unsafe {
        let device: Retained<AnyObject> = msg_send![class!(UIDevice), currentDevice];
        let idiom: isize = msg_send![&*device, userInterfaceIdiom];
        idiom == PAD
    }))
}

/// The system menu a named menu means, by its English name.
fn system_menu(name: &str) -> Option<&'static NSString> {
    // SAFETY: constants UIKit exports.
    unsafe {
        Some(match name.to_lowercase().as_str() {
            "file" => UIMenuFile,
            "edit" => UIMenuEdit,
            "format" => UIMenuFormat,
            "view" => UIMenuView,
            "window" => UIMenuWindow,
            "help" => UIMenuHelp,
            _ => return None,
        })
    }
}

fn place(builder: &AnyObject, group: &Group) {
    let sections: Vec<_> = group.sections.iter().map(|section| menu("", INLINE, None, section.iter().map(|d| command(d)).collect())).collect();
    // SAFETY: `UIMenuBuilder`'s insertions, each with a menu and a menu identifier.
    unsafe {
        match group.menu {
            None => {
                let items = menu("", INLINE, None, sections);
                let _: () = msg_send![builder, insertSiblingMenu: &*items, afterMenuForIdentifier: UIMenuAbout];
            }
            Some(name) => match system_menu(name) {
                Some(id) => {
                    let items = menu("", INLINE, None, sections);
                    let _: () = msg_send![builder, insertChildMenu: &*items, atEndOfMenuForIdentifier: id];
                }
                None => {
                    let title = crate::labels::label(crate::Label::Menu(name));
                    let id = NSString::from_str(&format!("rune.menu.{name}"));
                    let items = menu(&title, 0, Some(&id), sections);
                    let _: () = msg_send![builder, insertSiblingMenu: &*items, beforeMenuForIdentifier: UIMenuWindow];
                }
            },
        }
    }
}

/// `d` as a command, or a key command when it has a shortcut UIKit can take.
fn command(d: &Description) -> Retained<AnyObject> {
    let title = NSString::from_str(&d.title);
    let id = NSString::from_str(&d.id);
    let key = d.equivalent.as_ref().filter(|_| !d.keys_first).and_then(uikit_input);
    // SAFETY: a command with a title, no image, the action `RuneView` implements and the
    // item's id; its attributes and state are option sets of these sizes.
    unsafe {
        let command: Retained<AnyObject> = match key {
            Some((input, flags)) => msg_send![class!(UIKeyCommand), commandWithTitle: &*title, image: None::<&AnyObject>,
                action: sel!(runeMenuItem:), input: &*NSString::from_str(&input), modifierFlags: flags as isize, propertyList: &*id],
            None => msg_send![class!(UICommand), commandWithTitle: &*title, image: None::<&AnyObject>,
                action: sel!(runeMenuItem:), propertyList: &*id],
        };
        if !d.enabled { let _: () = msg_send![&*command, setAttributes: DISABLED]; }
        if d.checked { let _: () = msg_send![&*command, setState: ON]; }
        command
    }
}

fn menu(title: &str, options: usize, id: Option<&NSString>, children: Vec<Retained<AnyObject>>) -> Retained<AnyObject> {
    // SAFETY: an array of menu elements, and a menu made of them.
    unsafe {
        let list: Retained<AnyObject> = msg_send![class!(NSMutableArray), array];
        for child in &children { let _: () = msg_send![&*list, addObject: &**child]; }
        msg_send![class!(UIMenu), menuWithTitle: &*NSString::from_str(title), image: None::<&AnyObject>,
            identifier: id, options: options, children: &*list]
    }
}
