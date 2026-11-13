//! A `RuneView`'s pop-up menus, and the long press that asks for one. A finger held still on
//! something with a menu (a text field's Cut, Copy and Paste, selectable text's Copy, an
//! element's `on_secondary_press`) is the secondary press a right-click is on a desktop. The
//! menu a handler then asks for (`Tx::popup_menu`) shows as the system's edit menu, the bar
//! above a selection, through a `UIEditMenuInteraction`: its items are `UICommand`s that come
//! back to the view through the responder chain, so nothing here needs a block.

use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_foundation_video::NSString;
use objc2_video::rc::{Allocated, Retained, Weak};
use objc2_video::runtime::AnyObject;
use objc2_video::{DefinedClass, class, msg_send, sel};

use super::super::pacing::Event;
use super::{MainOnly, RuneView};
use crate::machine::Attempt;
use crate::platform::main_queue::after_main;
use crate::popup_menu::{Entry, PopupMenu, Show};

/// `UIGestureRecognizerStateBegan`.
const BEGAN: isize = 1;
/// `UIMenuElementAttributesDisabled`.
const DISABLED: usize = 1 << 0;
/// `UIMenuElementStateOn`.
const ON: isize = 1;
/// `UIMenuOptionsDisplayInline`: the children show in the parent, set apart, with no title.
const INLINE: usize = 1 << 0;

/// How long a menu that is going away may still report the item that was tapped. UIKit starts
/// taking the menu down before it sends the item's action.
const PICK_GRACE: f64 = 0.25;

/// The view's pop-up menu. The runtime waits for an answer from `Open` until `Closed`.
#[derive(Default)]
pub(super) enum Menu {
    #[default]
    Closed,
    /// Up, or on its way up. A text field's menu points at its caret, as UIKit's fields do,
    /// and follows it: `caret` is where it pointed last. Focusing the field brings up the
    /// keyboard, which can move the field away from where it was pressed.
    Open { show: Show, caret: Option<CGRect> },
    /// Going away: a pick may still arrive, and if none has when this attempt's timer fires,
    /// the menu was dismissed.
    Leaving(Attempt),
}

impl RuneView {
    /// Whether a long press at the recognizer's place would open anything. One that wouldn't
    /// never begins, so a finger resting on a list or a slider keeps its touch.
    pub(super) fn long_press_begins(&self, recognizer: &AnyObject) -> bool {
        // SAFETY: a recognizer's location in a view is a `CGPoint`.
        let at: CGPoint = unsafe { msg_send![recognizer, locationInView: self] };
        self.with_surface(|surface| surface.takes_secondary(at.x as f32, at.y as f32)).unwrap_or(false)
    }

    /// The long press began: UIKit has cancelled the finger's touch, and the runtime hears a
    /// secondary press where it rests.
    pub(super) fn long_pressed(&self, recognizer: &AnyObject) {
        // SAFETY: a recognizer's `state` is an `NSInteger` enum and its location a `CGPoint`.
        let (state, at): (isize, CGPoint) = unsafe { (msg_send![recognizer, state], msg_send![recognizer, locationInView: self]) };
        if state != BEGAN { return; }
        self.with_surface(|surface| surface.secondary(at.x as f32, at.y as f32));
        self.pace(Event::Wake);
    }

    /// Puts `show`'s menu up, at the focused field's caret when it is the field's own menu.
    /// The interaction asks the view for its items and its place.
    pub(super) fn open_menu(&self, show: Show, field: bool) {
        let caret = field.then(|| self.caret()).flatten();
        let (x, y) = match caret {
            Some(rect) => (rect.origin.x as f32 + rect.size.width as f32 / 2.0, rect.origin.y as f32 + rect.size.height as f32 / 2.0),
            None => show.at,
        };
        *self.ivars().menu.borrow_mut() = Menu::Open { show, caret };
        let interaction = self.ivars().edit_menu.borrow_mut().get_or_insert_with(|| {
            // SAFETY: `initWithDelegate:` is the interaction's initializer, the view conforms
            // to its delegate protocol, and a view keeps the interactions added to it.
            unsafe {
                let interaction: Allocated<AnyObject> = msg_send![class!(UIEditMenuInteraction), alloc];
                let interaction: Retained<AnyObject> = msg_send![interaction, initWithDelegate: self];
                let _: () = msg_send![self, addInteraction: &*interaction];
                interaction
            }
        }).clone();
        // SAFETY: a configuration with no identifier at a point of the view, presented by the
        // view's own interaction.
        unsafe {
            let at = CGPoint::new(x as CGFloat, y as CGFloat);
            let configuration: Retained<AnyObject> = msg_send![class!(UIEditMenuConfiguration), configurationWithIdentifier: None::<&AnyObject>, sourcePoint: at];
            let _: () = msg_send![&*interaction, presentEditMenuWithConfiguration: &*configuration];
        }
    }

    /// The open menu as a `UIMenu`, for the interaction.
    pub(super) fn menu_items(&self) -> Option<Retained<AnyObject>> {
        match &*self.ivars().menu.borrow() {
            Menu::Open { show, .. } => Some(build(&show.menu, "", 0)),
            Menu::Closed | Menu::Leaving(_) => None,
        }
    }

    /// What the open menu points at: the element it was asked for, else the point.
    pub(super) fn menu_target(&self) -> CGRect {
        let [x, y, w, h] = match &*self.ivars().menu.borrow() {
            Menu::Open { caret: Some(rect), .. } => return *rect,
            Menu::Open { show: Show { element: Some(rect), .. }, .. } => *rect,
            Menu::Open { show, .. } => [show.at.0, show.at.1, 0.0, 0.0],
            Menu::Closed | Menu::Leaving(_) => [0.0; 4],
        };
        CGRect::new(CGPoint::new(x as CGFloat, y as CGFloat), CGSize::new(w as CGFloat, h as CGFloat))
    }

    /// After a frame: a field's menu that is up moves with the caret.
    pub(super) fn menu_follow_caret(&self) {
        let moved = match &mut *self.ivars().menu.borrow_mut() {
            Menu::Open { caret: Some(at), .. } => match self.caret().filter(|now| now != at) {
                Some(now) => { *at = now; true }
                None => false,
            },
            _ => false,
        };
        let interaction = self.ivars().edit_menu.borrow().clone();
        if let (true, Some(interaction)) = (moved, interaction) {
            // SAFETY: the interaction asks the view for the menu's target rect again.
            let _: () = unsafe { msg_send![&*interaction, updateVisibleMenuPositionAnimated: true] };
        }
    }

    /// The focused field's caret, where the keyboard's text input has it.
    fn caret(&self) -> Option<CGRect> {
        self.ivars().text.borrow().as_ref().map(|input| input.last_caret()).filter(|rect| rect.size.height > 0.0)
    }

    /// An item was picked: `command` is its `UICommand`, whose property list is the item's id.
    pub(super) fn menu_picked(&self, command: &AnyObject) {
        // SAFETY: the commands `build` makes carry an `NSString`.
        let id: Option<Retained<NSString>> = unsafe { msg_send![command, propertyList] };
        self.answer_menu(id.map(|id| id.to_string()));
    }

    /// The menu is going away. A tapped item's action follows shortly; with none, the runtime
    /// hears the menu was dismissed.
    pub(super) fn menu_dismissed(&self) {
        let ivars = self.ivars();
        if !matches!(&*ivars.menu.borrow(), Menu::Open { .. }) { return; }
        let mut seq = ivars.menu_seq.get();
        let attempt = seq.next();
        ivars.menu_seq.set(seq);
        *ivars.menu.borrow_mut() = Menu::Leaving(attempt);
        let view = MainOnly(Weak::new(self));
        after_main(PICK_GRACE, move || view.menu_left(attempt));
    }

    /// The grace after a dismissal ran out: unless a pick or another menu came first, nothing
    /// was picked.
    pub(super) fn menu_left(&self, attempt: Attempt) {
        if matches!(&*self.ivars().menu.borrow(), Menu::Leaving(held) if *held == attempt) { self.answer_menu(None); }
    }

    fn answer_menu(&self, picked: Option<String>) {
        if matches!(std::mem::take(&mut *self.ivars().menu.borrow_mut()), Menu::Closed) { return; }
        self.with_surface(|surface| surface.popup_pick(picked));
        self.pace(Event::Wake);
    }
}

/// `menu` as a `UIMenu`: an item is a command that calls `runeMenuPick:` with its id, and the
/// items between separators are a group set apart.
fn build(menu: &PopupMenu, title: &str, options: usize) -> Retained<AnyObject> {
    let groups: Vec<&[Entry]> = menu.entries.split(|entry| matches!(entry, Entry::Separator)).filter(|group| !group.is_empty()).collect();
    let children: Vec<Retained<AnyObject>> = match groups.as_slice() {
        [only] => only.iter().map(element).collect(),
        groups => groups.iter().map(|group| ui_menu("", INLINE, group.iter().map(element).collect())).collect(),
    };
    ui_menu(title, options, children)
}

fn element(entry: &Entry) -> Retained<AnyObject> {
    match entry {
        Entry::Item(item) => {
            // SAFETY: a command with a title, no image, a selector the view implements and a
            // property list; its attributes and state are option sets of these sizes.
            unsafe {
                let command: Retained<AnyObject> = msg_send![class!(UICommand), commandWithTitle: &*NSString::from_str(&item.title),
                    image: None::<&AnyObject>, action: sel!(runeMenuPick:), propertyList: &*NSString::from_str(&item.id)];
                if !item.enabled { let _: () = msg_send![&*command, setAttributes: DISABLED]; }
                if item.checked { let _: () = msg_send![&*command, setState: ON]; }
                command
            }
        }
        Entry::Submenu(title, menu, _) => build(menu, title, 0),
        // `build` splits on separators before it gets here.
        Entry::Separator => ui_menu("", INLINE, Vec::new()),
    }
}

fn ui_menu(title: &str, options: usize, children: Vec<Retained<AnyObject>>) -> Retained<AnyObject> {
    // SAFETY: an array of menu elements, and a menu made of them.
    unsafe {
        let list: Retained<AnyObject> = msg_send![class!(NSMutableArray), array];
        for child in &children { let _: () = msg_send![&*list, addObject: &**child]; }
        msg_send![class!(UIMenu), menuWithTitle: &*NSString::from_str(title), image: None::<&AnyObject>,
            identifier: None::<&AnyObject>, options: options, children: &*list]
    }
}
