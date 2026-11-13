//! A `RuneView` joining and leaving a window, and what the system tells it while it is in one.
//! The view's first size starts its app on the view's layer. From then on its size, the safe
//! area, the soft keyboard's frame, the appearance, the text size and Reduce Motion settings,
//! and the app going to the background and coming back each reach the surface and wake the
//! pacing.

use std::ffi::c_void;

use objc2_core_foundation::{CGFloat, CGRect};
use objc2_video::rc::Retained;
use objc2_video::runtime::AnyObject;
use objc2_video::{DefinedClass, Encode, Encoding, class, msg_send, sel};

use super::super::accessibility::Accessibility;
use super::super::keys;
use super::super::pacing::Event;
use super::{RuneView, Stage};
use crate::platform::preferences::{reduced_motion, text_scale};

// SAFETY: UIKit's headers declare each as an `NSNotificationName` or an `NSString`, an object
// that lives for the process.
#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    static UIApplicationDidEnterBackgroundNotification: &'static AnyObject;
    static UIApplicationWillEnterForegroundNotification: &'static AnyObject;
    static UIKeyboardWillChangeFrameNotification: &'static AnyObject;
    static UIKeyboardFrameEndUserInfoKey: &'static AnyObject;
    static UIAccessibilityReduceMotionStatusDidChangeNotification: &'static AnyObject;
    static UIContentSizeCategoryDidChangeNotification: &'static AnyObject;
}

/// `UIUserInterfaceStyleDark`.
const STYLE_DARK: isize = 2;
/// `UIApplicationStateBackground`.
const STATE_BACKGROUND: isize = 2;

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

impl RuneView {
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

    /// The view has a size, or a new one: its first starts the app on the view's layer.
    pub(super) fn laid_out(&self) {
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

    /// The view joined a window, or left the one it was in: in a window it draws at the
    /// screen's scale and hears the system's notifications.
    pub(super) fn moved_to_window(&self) {
        // SAFETY: `UIView.window` is a nullable object, a window's screen is never nil and its
        // `nativeScale` is a `CGFloat`, the notification centre and the shared application are
        // singletons, and each selector is a method of this class that takes the
        // notification.
        unsafe {
            let centre: Retained<AnyObject> = msg_send![class!(NSNotificationCenter), defaultCenter];
            let _: () = msg_send![&*centre, removeObserver: self];
            let window: Option<Retained<AnyObject>> = msg_send![self, window];
            let Some(window) = window else {
                // A view out of its window hears no key-up.
                self.keys(keys::Event::Reset);
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

    /// The system's appearance, or another of the view's traits, changed.
    pub(super) fn traits_changed(&self) {
        let dark = self.dark();
        self.with_surface(|surface| surface.set_dark(dark));
        self.pace(Event::Wake);
    }

    /// The view's safe area changed.
    pub(super) fn safe_area_changed(&self) {
        let insets = self.safe_area();
        self.with_surface(|surface| surface.set_safe_area(insets));
        self.pace(Event::Wake);
    }

    /// The soft keyboard is moving to a new frame: what it covers of the view's bottom joins
    /// the safe area.
    pub(super) fn keyboard_moved(&self, notification: &AnyObject) {
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

    /// The text size setting changed.
    pub(super) fn text_size_changed(&self) {
        self.with_surface(|surface| surface.set_text_scale(text_scale()));
        self.pace(Event::Wake);
    }

    /// The Reduce Motion setting changed.
    pub(super) fn reduce_motion_changed(&self) {
        self.with_surface(|surface| surface.set_reduced_motion(reduced_motion()));
        self.pace(Event::Wake);
    }

    /// The app is coming back to the front.
    pub(super) fn entered_foreground(&self) {
        self.with_surface(|surface| surface.set_visible(true));
        self.pace(Event::Foreground);
    }

    /// The app went to the background.
    pub(super) fn entered_background(&self) {
        // The system sends no key-up to an app it puts away.
        self.keys(keys::Event::Reset);
        self.with_surface(|surface| surface.set_visible(false));
        self.pace(Event::Background);
    }
}
