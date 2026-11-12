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
use objc2_video::rc::{Retained, Weak};
use objc2_video::runtime::{AnyClass, AnyObject, NSObject};
use objc2_video::{DefinedClass, Encode, Encoding, MainThreadMarker, MainThreadOnly, class, define_class, extern_class, msg_send, sel};

use super::pacing::{self, Effect, Event};
use super::surface::{Mount, Surface};
use crate::machine::Seq;
use crate::platform::main_queue::{after_main, on_main};
use crate::runtime::{TouchPhase, Waker};
use crate::time::Instant;

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    static UIApplicationDidEnterBackgroundNotification: &'static AnyObject;
    static UIApplicationWillEnterForegroundNotification: &'static AnyObject;
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
}

define_class!(
    // SAFETY: `UIView` is made to be subclassed, and `RuneView` has no `Drop`.
    #[unsafe(super(UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RuneView"]
    #[ivars = Ivars]
    pub(crate) struct RuneView;

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

        #[unsafe(method(runeTick:))]
        fn tick(&self, link: &AnyObject) {
            // SAFETY: a display link's `targetTimestamp` is a `CFTimeInterval`.
            let shown: f64 = unsafe { msg_send![link, targetTimestamp] };
            // SAFETY: a C function that reads a clock.
            let ahead = (shown - unsafe { CACurrentMediaTime() }).max(0.0);
            let target = Instant::now() + Duration::from_secs_f64(ahead);
            let frame = self.with_surface(|surface| surface.frame(target));
            let (animating, wake_after) = frame.map_or((false, None), |f| (f.animating, f.wake_after_secs));
            self.pace(Event::Frame { animating, wake_after });
        }

        #[unsafe(method(runeForeground:))]
        fn foreground(&self, _notification: &AnyObject) {
            self.pace(Event::Foreground);
        }

        #[unsafe(method(runeBackground:))]
        fn background(&self, _notification: &AnyObject) {
            self.pace(Event::Background);
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
        });
        let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0));
        // SAFETY: `initWithFrame:` is `UIView`'s designated initializer.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        // SAFETY: a `UIView` property that takes a `UIViewAutoresizing` mask.
        let _: () = unsafe { msg_send![&*view, setAutoresizingMask: FILL_SUPERVIEW] };
        view
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
    /// the home indicator and the display's corners where the view reaches them.
    fn safe_area(&self) -> [f32; 4] {
        // SAFETY: a `UIView` property of type `UIEdgeInsets`.
        let insets: EdgeInsets = unsafe { msg_send![self, safeAreaInsets] };
        [insets.top, insets.right, insets.bottom, insets.left].map(|inset| inset as f32)
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
        // SAFETY: `UIView.window` is a nullable object, the notification centre and the shared
        // application are singletons, and the two selectors are methods of this class that
        // take the notification.
        unsafe {
            let centre: Retained<AnyObject> = msg_send![class!(NSNotificationCenter), defaultCenter];
            let _: () = msg_send![&*centre, removeObserver: self];
            let window: Option<Retained<AnyObject>> = msg_send![self, window];
            if window.is_none() {
                self.pace(Event::Detached);
                return;
            }
            let _: () = msg_send![&*centre, addObserver: self, selector: sel!(runeBackground:), name: UIApplicationDidEnterBackgroundNotification, object: None::<&AnyObject>];
            let _: () = msg_send![&*centre, addObserver: self, selector: sel!(runeForeground:), name: UIApplicationWillEnterForegroundNotification, object: None::<&AnyObject>];
            let app: Retained<AnyObject> = msg_send![class!(UIApplication), sharedApplication];
            let state: isize = msg_send![&*app, applicationState];
            self.pace(Event::Attached { foreground: state != STATE_BACKGROUND });
        }
    }

    fn touches(&self, touches: &AnyObject, phase: TouchPhase) {
        self.with_surface(|surface| {
            // SAFETY: `touches` is the `NSSet<UITouch *>` UIKit passes; a touch's location and
            // timestamp are a `CGPoint` and an `NSTimeInterval`.
            unsafe {
                let all: Retained<AnyObject> = msg_send![touches, allObjects];
                let count: usize = msg_send![&*all, count];
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
