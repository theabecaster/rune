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
use objc2_video::rc::{Allocated, Retained, Weak};
use objc2_video::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol, Sel};
use objc2_video::{DefinedClass, Encode, Encoding, MainThreadMarker, MainThreadOnly, class, define_class, extern_class, extern_protocol, msg_send, sel};

use super::pacing::{self, Effect, Event};
use super::surface::{Mount, Surface};
use super::keys;
use super::text_doc::Effect as Edit;
use super::text_input::RuneTextInput;
use crate::machine::{Effects, Seq};
use crate::{Ime, Key, KeyEvent, Mods};
use crate::platform::main_queue::{after_main, on_main};
use crate::runtime::{TouchPhase, Waker};
use crate::GesturePhase;
use crate::time::Instant;

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    static UIApplicationDidEnterBackgroundNotification: &'static AnyObject;
    static UIApplicationWillEnterForegroundNotification: &'static AnyObject;
    static UIKeyboardWillChangeFrameNotification: &'static AnyObject;
    static UIKeyboardFrameEndUserInfoKey: &'static AnyObject;
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
/// `UIGestureRecognizerState`'s `Began`, `Changed`, `Ended` and `Cancelled`.
const GESTURE_BEGAN: isize = 1;
const GESTURE_CHANGED: isize = 2;
const GESTURE_ENDED: isize = 3;
const GESTURE_CANCELLED: isize = 4;
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
    fn pace(&self, event: Event) {
        if let Some(view) = self.0.load() { view.pace(event); }
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
        fn touches_began(&self, touches: &AnyObject, _event: Option<&AnyObject>) {
            self.touches(touches, TouchPhase::Started);
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

        #[unsafe(method(runeForeground:))]
        fn foreground(&self, _notification: &AnyObject) {
            self.pace(Event::Foreground);
        }

        #[unsafe(method(runeBackground:))]
        fn background(&self, _notification: &AnyObject) {
            // The system sends no key-up to an app it puts away.
            self.keys(keys::Event::Reset);
            self.pace(Event::Background);
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
        view
    }

    /// Adds a recognizer of `class` that calls `action` on the view. When it begins, UIKit
    /// cancels the touches the view holds, so a pinch never also pans or presses.
    fn recognize(&self, class: &AnyClass, action: Sel) {
        // SAFETY: `initWithTarget:action:` is a gesture recognizer's designated initializer,
        // `action` is a method of this class that takes the recognizer, and the view conforms
        // to `UIGestureRecognizerDelegate`. A recognizer keeps neither its target nor its
        // delegate, and the view keeps the recognizer.
        unsafe {
            let recognizer: Allocated<AnyObject> = msg_send![class, alloc];
            let recognizer: Retained<AnyObject> = msg_send![recognizer, initWithTarget: self, action: action];
            let _: () = msg_send![&*recognizer, setDelegate: self];
            let _: () = msg_send![self, addGestureRecognizer: &*recognizer];
        }
    }

    /// A recognizer's phase and where its fingers' centre is in the view, or `None` in a state
    /// that carries no gesture (possible, failed).
    fn gesture(&self, recognizer: &AnyObject) -> Option<(GesturePhase, f32, f32)> {
        // SAFETY: a recognizer's `state` is an `NSInteger` enum and its location a `CGPoint`.
        let (state, at): (isize, CGPoint) = unsafe { (msg_send![recognizer, state], msg_send![recognizer, locationInView: self]) };
        let phase = match state {
            GESTURE_BEGAN => GesturePhase::Began,
            GESTURE_CHANGED => GesturePhase::Moved,
            GESTURE_ENDED | GESTURE_CANCELLED => GesturePhase::Ended,
            _ => return None,
        };
        Some((phase, at.x as f32, at.y as f32))
    }

    fn with_surface<R>(&self, f: impl FnOnce(&mut dyn Surface) -> R) -> Option<R> {
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
            let app: Retained<AnyObject> = msg_send![class!(UIApplication), sharedApplication];
            let state: isize = msg_send![&*app, applicationState];
            self.pace(Event::Attached { foreground: state != STATE_BACKGROUND });
        }
    }

    /// Tells the pacing a two-finger gesture began, moved or ended.
    fn track(&self, phase: GesturePhase) {
        match phase {
            GesturePhase::Began => self.pace(Event::Hold),
            GesturePhase::Ended => self.pace(Event::Release),
            GesturePhase::Moved => {}
        }
        self.pace(Event::Wake);
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
        let drawn = self.with_surface(|surface| (surface.frame(target), surface.text()));
        let Some((frame, text)) = drawn else {
            self.pace(Event::Frame { animating: false, wake_after: None });
            return;
        };
        // The frame applied the input before it: the keyboard hears what the field holds now.
        let input = self.ivars().text.borrow().clone();
        if let Some(input) = input { input.sync(text.wanted, text.snapshot, text.caret, text.secure); }
        self.pace(Event::Frame { animating: frame.animating, wake_after: frame.wake_after_secs });
    }

    /// Carries the keyboard's edits to the runtime, which applies them on its next frame.
    pub(crate) fn text_edits(&self, edits: Effects<Edit>) {
        self.with_surface(|surface| {
            for edit in edits {
                match edit {
                    Edit::Key(key) => surface.key(KeyEvent { key, mods: Mods::default() }),
                    Edit::Commit(text) => surface.ime(Ime::Commit(text)),
                    Edit::Preedit(text, cursor) => surface.ime(Ime::Preedit(text, cursor, Vec::new())),
                    Edit::Select(range) => surface.ime(Ime::Select(range)),
                    // For the keyboard, not the runtime.
                    Edit::TextChanged | Edit::SelectionChanged => {}
                }
            }
        });
        self.pace(Event::Wake);
    }

    /// Takes the hardware keyboard, unless the keyboard's text input has it for a field.
    pub(crate) fn take_keys(&self) {
        // SAFETY: `UIResponder` methods of these signatures.
        unsafe {
            let first: bool = msg_send![self, isFirstResponder];
            if !self.typing() && !first { let _: bool = msg_send![self, becomeFirstResponder]; }
        }
    }

    /// Whether the keyboard's text input is first responder, for a focused field.
    fn typing(&self) -> bool {
        // SAFETY: a `UIResponder` method of this signature.
        self.ivars().text.borrow().as_ref().is_some_and(|text| unsafe { msg_send![&**text, isFirstResponder] })
    }

    /// Hardware keys going down or up, for this view or (`typing`) for its text input. True
    /// when the runtime took every one of them; otherwise they are UIKit's.
    pub(crate) fn presses(&self, presses: &AnyObject, down: bool, typing: bool) -> bool {
        // Presses the text input left to UIKit come up the responder chain to this view, its
        // superview: they are still UIKit's.
        if !typing && self.typing() { return false; }
        let mut read = Vec::new();
        // SAFETY: `presses` is the `NSSet<UIPress *>` UIKit passes; a press's `key` is a
        // `UIKey` or nil, whose properties have these types.
        unsafe {
            let all: Retained<AnyObject> = msg_send![presses, allObjects];
            let count: usize = msg_send![&*all, count];
            for index in 0..count {
                let press: Retained<AnyObject> = msg_send![&*all, objectAtIndex: index];
                let key: Option<Retained<AnyObject>> = msg_send![&*press, key];
                let Some(key) = key else { return false };
                let base: Retained<NSString> = msg_send![&*key, charactersIgnoringModifiers];
                let typed: Retained<NSString> = msg_send![&*key, characters];
                read.push(keys::Press {
                    usage: msg_send![&*key, keyCode],
                    flags: msg_send![&*key, modifierFlags],
                    base: base.to_string(),
                    typed: typed.to_string(),
                });
            }
        }
        if !down {
            for press in &read { self.keys(keys::Event::Up { usage: press.usage }); }
            return true;
        }
        let events: Option<Vec<_>> = read.iter().map(|press| keys::event(press, typing).map(|event| (press.usage, event))).collect();
        let Some(events) = events else { return false };
        for (usage, event) in events { self.keys(keys::Event::Down { usage, event }); }
        true
    }

    /// The shortcut Command plus `c`, for an edit action UIKit sent (copy, cut, paste, select all).
    pub(crate) fn command(&self, c: char) {
        let mods = Mods { cmd: true, logo: true, ..Mods::default() };
        self.with_surface(|surface| surface.key(KeyEvent { key: Key::Command(c), mods }));
        self.pace(Event::Wake);
    }

    /// Steps the key repeat and does what it asks.
    fn keys(&self, event: keys::Event) {
        let ivars = self.ivars();
        let mut seq = ivars.key_seq.get();
        let effects = keys::step(&mut seq, &mut ivars.keys.borrow_mut(), event);
        ivars.key_seq.set(seq);
        let mut sent = false;
        for effect in effects {
            match effect {
                keys::Effect::Send(key) => sent |= self.with_surface(|surface| surface.key(key)).is_some(),
                keys::Effect::TimerAfter(attempt, seconds) => {
                    let view = MainOnly(Weak::new(self));
                    after_main(seconds, move || view.keys(keys::Event::Timer(attempt)));
                }
            }
        }
        if sent { self.pace(Event::Wake); }
    }

    fn touches(&self, touches: &AnyObject, phase: TouchPhase) {
        if phase == TouchPhase::Started { self.take_keys(); }
        let mut count = 0;
        self.with_surface(|surface| {
            // SAFETY: `touches` is the `NSSet<UITouch *>` UIKit passes; a touch's location and
            // timestamp are a `CGPoint` and an `NSTimeInterval`.
            unsafe {
                let all: Retained<AnyObject> = msg_send![touches, allObjects];
                count = msg_send![&*all, count];
                for index in 0..count {
                    let touch: Retained<AnyObject> = msg_send![&*all, objectAtIndex: index];
                    let at: CGPoint = msg_send![&*touch, locationInView: self];
                    let time: f64 = msg_send![&*touch, timestamp];
                    // UIKit keeps one `UITouch` for a finger from down to up, so its address
                    // tells fingers apart.
                    surface.touch(Retained::as_ptr(&touch) as usize as u64, phase, at.x as f32, at.y as f32, time);
                }
            }
        });
        // Each finger holds the display link from when it lands until it lifts.
        for _ in 0..count {
            match phase {
                TouchPhase::Started => self.pace(Event::Hold),
                TouchPhase::Ended | TouchPhase::Cancelled => self.pace(Event::Release),
                TouchPhase::Moved => {}
            }
        }
        self.pace(Event::Wake);
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
