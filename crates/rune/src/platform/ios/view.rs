//! `RuneView`: the `UIView` an app draws in. Its layer is a `CAMetalLayer`; a `CADisplayLink`
//! draws it while something moves and rests otherwise (`pacing`); its touches go to the
//! runtime. An app that owns the screen puts one in its window (`app`), and a UIKit or SwiftUI
//! app puts one wherever it wants a Rune surface (`rune::ios::view`).

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_foundation_video::NSString;
use objc2_video::rc::{Retained, Weak};
use objc2_video::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol};
use objc2_video::{DefinedClass, Encode, Encoding, MainThreadMarker, MainThreadOnly, class, define_class, extern_class, extern_protocol, msg_send, sel};

use super::pacing::{self, Effect, Event};
use super::surface::{Mount, Surface};
use super::accessibility::Accessibility;
use super::edit_menu;
use super::keys;
use crate::a11y::ios::Gesture;
use super::text::input::RuneTextInput;
use crate::machine::Seq;
use crate::platform::main_queue::{after_main, on_main};
use crate::platform::preferences::{reduced_motion, text_scale};
use crate::runtime::{TouchPhase, Waker};
use crate::time::Instant;

mod files;
mod input;
mod menu;

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    static UIApplicationDidEnterBackgroundNotification: &'static AnyObject;
    static UIApplicationWillEnterForegroundNotification: &'static AnyObject;
    static UIKeyboardWillChangeFrameNotification: &'static AnyObject;
    static UIKeyboardFrameEndUserInfoKey: &'static AnyObject;
    static UIAccessibilityReduceMotionStatusDidChangeNotification: &'static AnyObject;
    static UIContentSizeCategoryDidChangeNotification: &'static AnyObject;
}

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {
    /// Seconds on the clock a display link's timestamps are on.
    fn CACurrentMediaTime() -> f64;
}

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {
    static NSRunLoopCommonModes: &'static AnyObject;
}

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
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "UIResponder"]
    pub(crate) struct UIResponder;
);

extern_class!(
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "UIView"]
    pub(crate) struct UIView;
);

/// `UIUserInterfaceStyleDark`.
const STYLE_DARK: isize = 2;
/// `UIApplicationStateBackground`.
const STATE_BACKGROUND: isize = 2;
/// `UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight`.
const FILL_SUPERVIEW: usize = 2 | 16;

/// The frame rates a running link asks for. A ProMotion display gives 120; a 60 Hz display
/// gives 60, the most it has. An iPhone gives more than 60 only to an app whose `Info.plist`
/// sets `CADisableMinimumFrameDurationOnPhone`.
#[repr(C)]
#[derive(Clone, Copy)]
struct FrameRateRange {
    minimum: f32,
    maximum: f32,
    preferred: f32,
}

// SAFETY: `CAFrameRateRange` is three floats in this order.
unsafe impl Encode for FrameRateRange {
    const ENCODING: Encoding = Encoding::Struct("CAFrameRateRange", &[f32::ENCODING, f32::ENCODING, f32::ENCODING]);
}

/// `UIEdgeInsets`.
#[repr(C)]
#[derive(Clone, Copy)]
struct EdgeInsets {
    top: CGFloat,
    left: CGFloat,
    bottom: CGFloat,
    right: CGFloat,
}

// SAFETY: `UIEdgeInsets` is four `CGFloat`s in this order.
unsafe impl Encode for EdgeInsets {
    const ENCODING: Encoding = Encoding::Struct("UIEdgeInsets", &[CGFloat::ENCODING, CGFloat::ENCODING, CGFloat::ENCODING, CGFloat::ENCODING]);
}

const FRAME_RATES: FrameRateRange = FrameRateRange { minimum: 80.0, maximum: 120.0, preferred: 120.0 };

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

/// A weak view that crosses threads but is only read on the main one.
struct MainOnly(Weak<RuneView>);

// SAFETY: the weak reference is copied and dropped from any thread, which the Objective-C
// runtime allows, and loaded only inside `on_main` and `after_main` jobs, on the main thread.
unsafe impl Send for MainOnly {}
// SAFETY: as above.
unsafe impl Sync for MainOnly {}

impl MainOnly {
    /// Feeds `event` to the view's pacing, if the view is still alive. Main thread only.
    pub(super) fn pace(&self, event: Event) {
        if let Some(view) = self.0.load() { view.pace(event); }
    }

    /// Tells the view a dismissed menu's grace ran out.
    fn menu_left(&self, attempt: crate::machine::Attempt) {
        if let Some(view) = self.0.load() { view.menu_left(attempt); }
    }

    /// Feeds `event` to the view's key repeat, as `pace` does its pacing.
    fn keys(&self, event: keys::Event) {
        if let Some(view) = self.0.load() { view.keys(event); }
    }
}

define_class!(
    // SAFETY: `UIView` is made to be subclassed, and `RuneView` has no `Drop`.
    #[unsafe(super(UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RuneView"]
    #[ivars = Ivars]
    pub(crate) struct RuneView;

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
            let dark = self.dark();
            self.with_surface(|surface| surface.set_dark(dark));
            self.pace(Event::Wake);
        }

        #[unsafe(method(safeAreaInsetsDidChange))]
        fn safe_area_insets_did_change(&self) {
            // SAFETY: `UIView` implements it with this signature.
            let _: () = unsafe { msg_send![super(self), safeAreaInsetsDidChange] };
            let insets = self.safe_area();
            self.with_surface(|surface| surface.set_safe_area(insets));
            self.pace(Event::Wake);
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
            let Some((phase, x, y)) = self.gesture(recognizer) else { return };
            // SAFETY: a pinch recognizer's `scale` is a `CGFloat`. Setting it back to 1 makes
            // the next callback's scale the change since this one.
            let scale: CGFloat = unsafe {
                let scale = msg_send![recognizer, scale];
                let _: () = msg_send![recognizer, setScale: 1.0 as CGFloat];
                scale
            };
            self.with_surface(|surface| surface.magnify(scale as f32 - 1.0, x, y, phase));
            self.track(phase);
        }

        #[unsafe(method(runeRotate:))]
        fn rotate(&self, recognizer: &AnyObject) {
            let Some((phase, x, y)) = self.gesture(recognizer) else { return };
            // SAFETY: a rotation recognizer's `rotation` is a `CGFloat` in radians. Setting it
            // back to 0 makes the next callback's rotation the turn since this one.
            let radians: CGFloat = unsafe {
                let radians = msg_send![recognizer, rotation];
                let _: () = msg_send![recognizer, setRotation: 0.0 as CGFloat];
                radians
            };
            // UIKit's angle grows clockwise; a `RotateEvent`'s counterclockwise.
            self.with_surface(|surface| surface.rotate(-(radians as f32).to_degrees(), x, y, phase));
            self.track(phase);
        }

        #[unsafe(method(runeTick:))]
        fn tick(&self, _link: &AnyObject) {
            self.pace(Event::Tick);
        }

        #[unsafe(method(runeKeyboard:))]
        fn keyboard(&self, notification: &AnyObject) {
            // SAFETY: the notification's `userInfo` holds the keyboard's final frame as an
            // `NSValue` of a `CGRect` in the screen's coordinates, which a nil source view
            // converts from; `bounds` is a `CGRect`.
            let overlap = unsafe {
                let info: Option<Retained<AnyObject>> = msg_send![notification, userInfo];
                let Some(info) = info else { return };
                let value: Option<Retained<AnyObject>> = msg_send![&*info, objectForKey: UIKeyboardFrameEndUserInfoKey];
                let Some(value) = value else { return };
                let frame: CGRect = msg_send![&*value, CGRectValue];
                let frame: CGRect = msg_send![self, convertRect: frame, fromView: None::<&AnyObject>];
                let bounds: CGRect = msg_send![self, bounds];
                (bounds.origin.y + bounds.size.height - frame.origin.y).clamp(0.0, bounds.size.height)
            };
            self.ivars().keyboard.set(overlap as f32);
            let insets = self.safe_area();
            self.with_surface(|surface| surface.set_safe_area(insets));
            self.pace(Event::Wake);
        }

        #[unsafe(method(runeTextSize:))]
        fn text_size(&self, _notification: &AnyObject) {
            self.with_surface(|surface| surface.set_text_scale(text_scale()));
            self.pace(Event::Wake);
        }

        #[unsafe(method(runeReduceMotion:))]
        fn reduce_motion(&self, _notification: &AnyObject) {
            self.with_surface(|surface| surface.set_reduced_motion(reduced_motion()));
            self.pace(Event::Wake);
        }

        #[unsafe(method(runeForeground:))]
        fn foreground(&self, _notification: &AnyObject) {
            self.with_surface(|surface| surface.set_visible(true));
            self.pace(Event::Foreground);
        }

        #[unsafe(method(runeBackground:))]
        fn background(&self, _notification: &AnyObject) {
            // The system sends no key-up to an app it puts away.
            self.keys(keys::Event::Reset);
            self.with_surface(|surface| surface.set_visible(false));
            self.pace(Event::Background);
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
                // SAFETY: the responder chain's own handling of presses this view leaves alone.
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

    /// The view's size in points and its scale.
    fn metrics(&self) -> ((f32, f32), f32) {
        // SAFETY: `UIView` properties of these types.
        let (bounds, scale): (CGRect, CGFloat) = unsafe { (msg_send![self, bounds], msg_send![self, contentScaleFactor]) };
        ((bounds.size.width as f32, bounds.size.height as f32), scale as f32)
    }

    fn dark(&self) -> bool {
        // SAFETY: `traitCollection` is never nil, and `userInterfaceStyle` is an `NSInteger` enum.
        let style: isize = unsafe {
            let traits: Retained<AnyObject> = msg_send![self, traitCollection];
            msg_send![&*traits, userInterfaceStyle]
        };
        style == STYLE_DARK
    }

    /// What the screen keeps at the view's top, right, bottom and left edges: the status bar,
    /// the home indicator, the display's corners where the view reaches them, and the soft
    /// keyboard while it shows.
    fn safe_area(&self) -> [f32; 4] {
        // SAFETY: a `UIView` property of type `UIEdgeInsets`.
        let insets: EdgeInsets = unsafe { msg_send![self, safeAreaInsets] };
        // The soft keyboard takes the bottom of the screen as the home indicator does: content
        // padded by the safe area stays above it.
        let bottom = (insets.bottom as f32).max(self.ivars().keyboard.get());
        [insets.top as f32, insets.right as f32, bottom, insets.left as f32]
    }

    fn laid_out(&self) {
        let (size, scale) = self.metrics();
        if size.0 <= 0.0 || size.1 <= 0.0 { return; }
        let mut stage = self.ivars().stage.borrow_mut();
        *stage = match std::mem::replace(&mut *stage, Stage::Failed) {
            Stage::Waiting(mount) => {
                // SAFETY: the view's layer is the `CAMetalLayer` `layerClass` names, and the
                // view keeps it for as long as it lives.
                let layer: Retained<AnyObject> = unsafe { msg_send![self, layer] };
                match mount(Retained::as_ptr(&layer) as *mut c_void, size, scale, self.dark(), self.waker()) {
                    Ok(mut surface) => {
                        surface.set_safe_area(self.safe_area());
                        surface.set_reduced_motion(reduced_motion());
                        surface.set_text_scale(text_scale());
                        // A client that asked before the app started still gets its tree.
                        if self.ivars().accessibility.borrow().asked() { surface.accessibility_wanted(); }
                        Stage::Live(surface)
                    }
                    Err(error) => {
                        eprintln!("rune: this view draws nothing: {error}");
                        Stage::Failed
                    }
                }
            }
            Stage::Live(mut surface) => {
                surface.resize(size, scale);
                Stage::Live(surface)
            }
            Stage::Failed => Stage::Failed,
        };
        drop(stage);
        self.pace(Event::Wake);
    }

    /// What other threads call when the app has something new to draw.
    fn waker(&self) -> Waker {
        let view = Arc::new(MainOnly(Weak::new(self)));
        let queued = self.ivars().wake_queued.clone();
        Arc::new(move || {
            if queued.swap(true, Ordering::AcqRel) { return; }
            let (view, queued) = (view.clone(), queued.clone());
            on_main(move || {
                queued.store(false, Ordering::Release);
                view.pace(Event::Wake);
            });
        })
    }

    fn moved_to_window(&self) {
        // SAFETY: `UIView.window` is a nullable object, a window's screen is never nil and its
        // `nativeScale` is a `CGFloat`, the notification centre and the shared application are
        // singletons, and the two selectors are methods of this class that take the
        // notification.
        unsafe {
            let centre: Retained<AnyObject> = msg_send![class!(NSNotificationCenter), defaultCenter];
            let _: () = msg_send![&*centre, removeObserver: self];
            let window: Option<Retained<AnyObject>> = msg_send![self, window];
            let Some(window) = window else {
                self.pace(Event::Detached);
                return;
            };
            // A view on a `CAMetalLayer` starts at one pixel a point, whatever the screen: left
            // there, the layer is drawn at a third of an iPhone's pixels and stretched.
            let screen: Retained<AnyObject> = msg_send![&*window, screen];
            let scale: CGFloat = msg_send![&*screen, nativeScale];
            let _: () = msg_send![self, setContentScaleFactor: scale];
            let _: () = msg_send![self, setNeedsLayout];
            let _: () = msg_send![&*centre, addObserver: self, selector: sel!(runeBackground:), name: UIApplicationDidEnterBackgroundNotification, object: None::<&AnyObject>];
            let _: () = msg_send![&*centre, addObserver: self, selector: sel!(runeForeground:), name: UIApplicationWillEnterForegroundNotification, object: None::<&AnyObject>];
            let _: () = msg_send![&*centre, addObserver: self, selector: sel!(runeKeyboard:), name: UIKeyboardWillChangeFrameNotification, object: None::<&AnyObject>];
            let _: () = msg_send![&*centre, addObserver: self, selector: sel!(runeReduceMotion:), name: UIAccessibilityReduceMotionStatusDidChangeNotification, object: None::<&AnyObject>];
            let _: () = msg_send![&*centre, addObserver: self, selector: sel!(runeTextSize:), name: UIContentSizeCategoryDidChangeNotification, object: None::<&AnyObject>];
            let app: Retained<AnyObject> = msg_send![class!(UIApplication), sharedApplication];
            let state: isize = msg_send![&*app, applicationState];
            self.pace(Event::Attached { foreground: state != STATE_BACKGROUND });
            // A reader already running reads the first frame.
            if Accessibility::reader_running() { self.accessibility_asked(); }
        }
    }

    /// Draws the frame the display link's next refresh shows, and tells the pacing how it went.
    fn draw(&self) {
        let Some(shown) = self.ivars().link.borrow().as_ref().map(|link| {
            // SAFETY: a display link's `targetTimestamp` is a `CFTimeInterval`.
            let shown: f64 = unsafe { msg_send![&**link, targetTimestamp] };
            shown
        }) else { return };
        // SAFETY: a C function that reads a clock.
        let ahead = (shown - unsafe { CACurrentMediaTime() }).max(0.0);
        let target = Instant::now() + Duration::from_secs_f64(ahead);
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

    /// The view's elements, for a client that asks. The first ask starts the runtime's tree,
    /// and the elements follow the frame it wakes.
    fn accessibility_asked(&self) -> Retained<AnyObject> {
        let (list, first) = self.ivars().accessibility.borrow_mut().elements();
        if first {
            self.with_surface(|surface| surface.accessibility_wanted());
            self.pace(Event::Wake);
        }
        list
    }

    /// What a reader did to element `id`: true when the element takes it, which the next frame
    /// then does.
    pub(crate) fn accessibility_gesture(&self, id: u64, gesture: Gesture) -> bool {
        let Some(request) = self.ivars().accessibility.borrow().request(id, gesture) else { return false };
        self.with_surface(|surface| surface.accessibility_act(request));
        self.pace(Event::Wake);
        true
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
                Effect::Destroy => {
                    if let Some(link) = ivars.link.borrow_mut().take() {
                        // SAFETY: `invalidate` takes the link off its run loop and releases the view.
                        let _: () = unsafe { msg_send![&*link, invalidate] };
                    }
                }
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

    /// A paused display link on the main run loop, in the modes that also run while a finger
    /// tracks. The link retains the view until `Effect::Destroy` invalidates it.
    fn create_link(&self) {
        // SAFETY: `runeTick:` is a method of this class that takes the link; a display link's
        // `paused` is a `BOOL`, its frame rate range a `CAFrameRateRange`.
        let link = unsafe {
            let link: Retained<AnyObject> = msg_send![class!(CADisplayLink), displayLinkWithTarget: self, selector: sel!(runeTick:)];
            let _: () = msg_send![&*link, setPaused: true];
            let _: () = msg_send![&*link, setPreferredFrameRateRange: FRAME_RATES];
            let run_loop: Retained<AnyObject> = msg_send![class!(NSRunLoop), mainRunLoop];
            let _: () = msg_send![&*link, addToRunLoop: &*run_loop, forMode: NSRunLoopCommonModes];
            link
        };
        *self.ivars().link.borrow_mut() = Some(link);
    }

    fn set_paused(&self, paused: bool) {
        if let Some(link) = &*self.ivars().link.borrow() {
            // SAFETY: a display link's `paused` is a `BOOL`.
            let _: () = unsafe { msg_send![&**link, setPaused: paused] };
        }
    }
}
