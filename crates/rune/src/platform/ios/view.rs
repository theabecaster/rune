//! `RuneView`: the `UIView` an app draws in. Its layer is a `CAMetalLayer`; a `CADisplayLink`
//! draws it while something moves and rests otherwise (`pacing`); its touches go to the
//! runtime. An app that owns the screen puts one in its window (`app`), and a UIKit or SwiftUI
//! app puts one wherever it wants a Rune surface (`rune::ios::view`).

use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation_video::NSString;
use objc2_video::rc::{Retained, Weak};
use objc2_video::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol};
use objc2_video::{DefinedClass, MainThreadMarker, MainThreadOnly, class, define_class, extern_class, extern_protocol, msg_send, sel};

use super::accessibility::Accessibility;
use super::edit_menu;
use super::keys;
use super::pacing::{self, Effect, Event};
use super::surface::{Mount, Surface};
use super::text::input::RuneTextInput;
use crate::machine::Seq;
use crate::platform::main_queue::after_main;
use crate::runtime::TouchPhase;

mod accessibility;
mod files;
mod input;
mod lifecycle;
mod link;
mod menu;

use link::MainOnly;

extern_protocol!(
    /// A gesture recognizer asks its delegate whether another may recognize alongside it.
    #[name = "UIGestureRecognizerDelegate"]
    unsafe trait GestureRecognizerDelegate: NSObjectProtocol {}
);

extern_protocol!(
    /// A document picker tells its delegate what was picked, or that nothing was.
    #[name = "UIDocumentPickerDelegate"]
    unsafe trait DocumentPickerDelegate: NSObjectProtocol {}
);

extern_protocol!(
    /// An edit menu interaction asks its delegate for the menu, its place, and says when it goes.
    #[name = "UIEditMenuInteractionDelegate"]
    unsafe trait EditMenuInteractionDelegate: NSObjectProtocol {}
);

extern_class!(
    // SAFETY: UIKit's class of this name is an `NSObject` subclass for the main thread only.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "UIResponder"]
    pub(crate) struct UIResponder;
);

extern_class!(
    // SAFETY: UIKit's class of this name is a `UIResponder` subclass for the main thread only.
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "UIView"]
    pub(crate) struct UIView;
);

/// `UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight`.
const FILL_SUPERVIEW: usize = 2 | 16;

/// What the view draws: an app waiting for the view's first size, an app drawing, or nothing
/// because its GPU surface couldn't be made.
enum Stage {
    Waiting(Mount),
    Live(Box<dyn Surface>),
    Failed,
}

pub(crate) struct Ivars {
    stage: RefCell<Stage>,
    pacing: Cell<pacing::State>,
    seq: Cell<Seq>,
    link: RefCell<Option<Retained<AnyObject>>>,
    /// A wake from another thread is on its way to the main queue: later ones ride with it.
    wake_queued: Arc<AtomicBool>,
    /// What the keyboard talks to, a hidden view inside this one.
    text: RefCell<Option<Retained<RuneTextInput>>>,
    /// How far the soft keyboard reaches up over the view's bottom edge, in points.
    keyboard: Cell<f32>,
    accessibility: RefCell<Accessibility>,
    /// The file dialog whose picker is up.
    file_dialog: RefCell<Option<files::Picking>>,
    menu: RefCell<edit_menu::State>,
    menu_seq: Cell<Seq>,
    /// What presents the menu, made when the first one opens.
    edit_menu: RefCell<Option<Retained<AnyObject>>>,
    /// The hardware key that repeats while held.
    keys: RefCell<keys::Repeat>,
    key_seq: Cell<Seq>,
}

define_class!(
    // SAFETY: `UIView` is made to be subclassed, and `RuneView` has no `Drop`.
    #[unsafe(super(UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RuneView"]
    #[ivars = Ivars]
    pub(crate) struct RuneView;

    // SAFETY: the superclass chain ends in `NSObject`, which implements the protocol.
    unsafe impl NSObjectProtocol for RuneView {}

    // SAFETY: every method of the protocol is optional.
    unsafe impl GestureRecognizerDelegate for RuneView {
        /// A pinch and a turn are one two-finger gesture: both run at once, as on a trackpad.
        #[unsafe(method(gestureRecognizer:shouldRecognizeSimultaneouslyWithGestureRecognizer:))]
        fn recognizes_with(&self, _recognizer: &AnyObject, _other: &AnyObject) -> bool {
            true
        }

        /// A long press begins only over something with a menu to open.
        #[unsafe(method(gestureRecognizerShouldBegin:))]
        fn recognizer_begins(&self, recognizer: &AnyObject) -> bool {
            // SAFETY: `isKindOfClass:` takes a class and answers a `BOOL`.
            let long_press: bool = unsafe { msg_send![recognizer, isKindOfClass: class!(UILongPressGestureRecognizer)] };
            !long_press || self.long_press_begins(recognizer)
        }
    }

    // SAFETY: every method of the protocol is optional.
    unsafe impl DocumentPickerDelegate for RuneView {
        #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
        fn document_picked(&self, _picker: &AnyObject, urls: &AnyObject) {
            self.file_picked(Some(urls));
        }

        #[unsafe(method(documentPickerWasCancelled:))]
        fn document_picker_cancelled(&self, _picker: &AnyObject) {
            self.file_picked(None);
        }
    }

    // SAFETY: every method of the protocol is optional.
    unsafe impl EditMenuInteractionDelegate for RuneView {
        #[unsafe(method_id(editMenuInteraction:menuForConfiguration:suggestedActions:))]
        fn edit_menu(&self, _interaction: &AnyObject, _configuration: &AnyObject, _suggested: &AnyObject) -> Option<Retained<AnyObject>> {
            self.menu_items()
        }

        #[unsafe(method(editMenuInteraction:targetRectForConfiguration:))]
        fn edit_menu_target(&self, _interaction: &AnyObject, _configuration: &AnyObject) -> CGRect {
            self.menu_target()
        }

        #[unsafe(method(editMenuInteraction:willDismissMenuForConfiguration:animator:))]
        fn edit_menu_dismissed(&self, _interaction: &AnyObject, _configuration: &AnyObject, _animator: &AnyObject) {
            self.menu_dismissed();
        }
    }

    impl RuneView {
        #[unsafe(method(layerClass))]
        fn layer_class() -> &'static AnyClass {
            class!(CAMetalLayer)
        }

        #[unsafe(method(layoutSubviews))]
        fn layout_subviews(&self) {
            // SAFETY: `UIView` implements it with this signature.
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };
            self.laid_out();
        }

        #[unsafe(method(didMoveToWindow))]
        fn did_move_to_window(&self) {
            // SAFETY: `UIView` implements it with this signature.
            let _: () = unsafe { msg_send![super(self), didMoveToWindow] };
            self.moved_to_window();
        }

        #[unsafe(method(traitCollectionDidChange:))]
        fn trait_collection_did_change(&self, previous: Option<&AnyObject>) {
            // SAFETY: `UIView` implements it with this signature.
            let _: () = unsafe { msg_send![super(self), traitCollectionDidChange: previous] };
            self.traits_changed();
        }

        #[unsafe(method(safeAreaInsetsDidChange))]
        fn safe_area_insets_did_change(&self) {
            // SAFETY: `UIView` implements it with this signature.
            let _: () = unsafe { msg_send![super(self), safeAreaInsetsDidChange] };
            self.safe_area_changed();
        }

        #[unsafe(method(touchesBegan:withEvent:))]
        fn touches_began(&self, touches: &AnyObject, event: Option<&AnyObject>) {
            if !self.secondary_click(touches, event) { self.touches(touches, TouchPhase::Started); }
        }

        #[unsafe(method(touchesMoved:withEvent:))]
        fn touches_moved(&self, touches: &AnyObject, _event: Option<&AnyObject>) {
            self.touches(touches, TouchPhase::Moved);
        }

        #[unsafe(method(touchesEnded:withEvent:))]
        fn touches_ended(&self, touches: &AnyObject, _event: Option<&AnyObject>) {
            self.touches(touches, TouchPhase::Ended);
        }

        #[unsafe(method(touchesCancelled:withEvent:))]
        fn touches_cancelled(&self, touches: &AnyObject, _event: Option<&AnyObject>) {
            self.touches(touches, TouchPhase::Cancelled);
        }

        #[unsafe(method(runeHover:))]
        fn hover(&self, recognizer: &AnyObject) {
            self.hovered(recognizer);
        }

        #[unsafe(method(runeScroll:))]
        fn scroll(&self, recognizer: &AnyObject) {
            self.scrolled(recognizer);
        }

        #[unsafe(method(runeLongPress:))]
        fn long_press(&self, recognizer: &AnyObject) {
            self.long_pressed(recognizer);
        }

        /// An item of the app's own menus (`App::menu_item`), picked from the menu bar or by
        /// its shortcut: the command carries the item's id.
        #[unsafe(method(runeMenuItem:))]
        fn menu_item(&self, command: &AnyObject) {
            // SAFETY: the commands `native_menu` makes carry an `NSString`.
            let id: Option<Retained<NSString>> = unsafe { msg_send![command, propertyList] };
            if let Some(id) = id && crate::native_menu::perform_ios_item(&id.to_string()) { self.pace(Event::Wake); }
        }

        #[unsafe(method(runeMenuPick:))]
        fn menu_pick(&self, command: &AnyObject) {
            self.menu_picked(command);
        }

        #[unsafe(method(runePinch:))]
        fn pinch(&self, recognizer: &AnyObject) {
            self.pinched(recognizer);
        }

        #[unsafe(method(runeRotate:))]
        fn rotate(&self, recognizer: &AnyObject) {
            self.rotated(recognizer);
        }

        #[unsafe(method(runeTick:))]
        fn tick(&self, _link: &AnyObject) {
            self.pace(Event::Tick);
        }

        #[unsafe(method(runeKeyboard:))]
        fn keyboard(&self, notification: &AnyObject) {
            self.keyboard_moved(notification);
        }

        #[unsafe(method(runeTextSize:))]
        fn text_size(&self, _notification: &AnyObject) {
            self.text_size_changed();
        }

        #[unsafe(method(runeReduceMotion:))]
        fn reduce_motion(&self, _notification: &AnyObject) {
            self.reduce_motion_changed();
        }

        #[unsafe(method(runeForeground:))]
        fn foreground(&self, _notification: &AnyObject) {
            self.entered_foreground();
        }

        #[unsafe(method(runeBackground:))]
        fn background(&self, _notification: &AnyObject) {
            self.entered_background();
        }

        /// The view is a container: a reader lands on its elements, never on it.
        #[unsafe(method(isAccessibilityElement))]
        fn is_accessibility_element(&self) -> bool {
            false
        }

        #[unsafe(method_id(accessibilityElements))]
        fn accessibility_elements(&self) -> Option<Retained<AnyObject>> {
            Some(self.accessibility_asked())
        }

        /// With no text field focused the view itself hears a hardware keyboard.
        #[unsafe(method(canBecomeFirstResponder))]
        fn can_become_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(pressesBegan:withEvent:))]
        fn presses_began(&self, presses: &AnyObject, event: Option<&AnyObject>) {
            if !self.presses(presses, true, false) {
                // SAFETY: `presses` and `event` are UIKit's own objects, passed unchanged to
                // `UIView`'s method of the same signature.
                let _: () = unsafe { msg_send![super(self), pressesBegan: presses, withEvent: event] };
            }
        }

        #[unsafe(method(pressesEnded:withEvent:))]
        fn presses_ended(&self, presses: &AnyObject, event: Option<&AnyObject>) {
            self.presses(presses, false, false);
            // SAFETY: as in `pressesBegan:withEvent:`.
            let _: () = unsafe { msg_send![super(self), pressesEnded: presses, withEvent: event] };
        }

        #[unsafe(method(pressesCancelled:withEvent:))]
        fn presses_cancelled(&self, presses: &AnyObject, event: Option<&AnyObject>) {
            self.presses(presses, false, false);
            // SAFETY: as in `pressesBegan:withEvent:`.
            let _: () = unsafe { msg_send![super(self), pressesCancelled: presses, withEvent: event] };
        }
    }
);

impl RuneView {
    /// A view that starts `mount`'s app when it first has a size, filling whatever it is put in.
    pub(crate) fn new(mtm: MainThreadMarker, mount: Mount) -> Retained<Self> {
        crate::embed::prewarm();
        let this = Self::alloc(mtm).set_ivars(Ivars {
            stage: RefCell::new(Stage::Waiting(mount)),
            pacing: Cell::new(pacing::State::Detached),
            seq: Cell::new(Seq::default()),
            link: RefCell::new(None),
            wake_queued: Arc::new(AtomicBool::new(false)),
            text: RefCell::new(None),
            keyboard: Cell::new(0.0),
            accessibility: RefCell::new(Accessibility::new()),
            file_dialog: RefCell::new(None),
            menu: RefCell::default(),
            menu_seq: Cell::new(Seq::default()),
            edit_menu: RefCell::new(None),
            keys: RefCell::new(keys::Repeat::Idle),
            key_seq: Cell::new(Seq::default()),
        });
        let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0));
        // SAFETY: `initWithFrame:` is `UIView`'s designated initializer.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        // SAFETY: a `UIView` property that takes a `UIViewAutoresizing` mask.
        let _: () = unsafe { msg_send![&*view, setAutoresizingMask: FILL_SUPERVIEW] };
        let text = RuneTextInput::new(mtm, &view);
        // SAFETY: `addSubview:` takes a view, which the text input is.
        let _: () = unsafe { msg_send![&*view, addSubview: &*text] };
        *view.ivars().text.borrow_mut() = Some(text);
        view.recognize(class!(UIPinchGestureRecognizer), sel!(runePinch:));
        view.recognize(class!(UIRotationGestureRecognizer), sel!(runeRotate:));
        view.recognize(class!(UILongPressGestureRecognizer), sel!(runeLongPress:));
        view.recognize_pointer();
        view
    }

    pub(super) fn with_surface<R>(&self, f: impl FnOnce(&mut dyn Surface) -> R) -> Option<R> {
        match &mut *self.ivars().stage.borrow_mut() {
            Stage::Live(surface) => Some(f(surface.as_mut())),
            Stage::Waiting(_) | Stage::Failed => None,
        }
    }

    /// Draws the frame the display link's next refresh shows, and tells the pacing how it went.
    fn draw(&self) {
        let Some(target) = self.frame_target() else { return };
        let drawn = self.with_surface(|surface| (surface.frame(target), surface.text(), surface.accessibility(), surface.popup(), surface.file_dialog()));
        let Some((frame, text, tree, popup, file)) = drawn else {
            self.pace(Event::Frame { animating: false, wake_after: None });
            return;
        };
        // The frame applied the input before it: the keyboard hears what the field holds now.
        let input = self.ivars().text.borrow().clone();
        if let Some(input) = input { input.sync(text.wanted, text.snapshot, text.caret, text.secure); }
        match popup {
            Some((popup, field)) => self.open_menu(popup, field),
            None => self.menu_follow_caret(),
        }
        if let Some((id, kind, options)) = file { self.pick_file(id, kind, &options); }
        if let Some((tree, zoom)) = tree {
            let posts = self.ivars().accessibility.borrow_mut().apply(self.mtm(), self, tree, zoom);
            posts.post();
        }
        self.pace(Event::Frame { animating: frame.animating, wake_after: frame.wake_after_secs });
    }

    fn pace(&self, event: Event) {
        let ivars = self.ivars();
        let (mut seq, mut state) = (ivars.seq.get(), ivars.pacing.get());
        let effects = pacing::step(&mut seq, &mut state, event);
        ivars.seq.set(seq);
        ivars.pacing.set(state);
        for effect in effects {
            match effect {
                Effect::Create => self.create_link(),
                Effect::Destroy => self.destroy_link(),
                Effect::Draw => self.draw(),
                Effect::Resume => self.set_paused(false),
                Effect::Pause => self.set_paused(true),
                Effect::TimerAfter(attempt, seconds) => {
                    let view = MainOnly(Weak::new(self));
                    after_main(seconds, move || view.pace(Event::Timer(attempt)));
                }
            }
        }
    }
}
