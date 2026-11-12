//! An app that owns the screen: `UIApplicationMain`, one window scene, and a `RuneView` that
//! fills its window. The scene's configuration is made in code, so an app's `Info.plist` needs
//! only an empty `UIApplicationSceneManifest`.

use std::cell::RefCell;
use std::ffi::{c_char, c_int};

use objc2_foundation_video::NSString;
use objc2_video::rc::{Allocated, Retained};
use objc2_video::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2_video::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, class, define_class, extern_protocol, msg_send};

use super::surface::{self, Mount};
use super::view::{RuneView, UIResponder};
use crate::app::App;

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    fn UIApplicationMain(argc: c_int, argv: *mut *mut c_char, principal: Option<&NSString>, delegate: Option<&NSString>) -> c_int;
}

unsafe extern "C" {
    fn _NSGetArgc() -> *mut c_int;
    fn _NSGetArgv() -> *mut *mut *mut c_char;
}

thread_local! {
    /// The app `run` was given, until its scene connects.
    static MOUNT: RefCell<Option<Mount>> = const { RefCell::new(None) };
}

extern_protocol!(
    /// The methods below are declared where each delegate implements them.
    #[name = "UIApplicationDelegate"]
    unsafe trait ApplicationDelegate: NSObjectProtocol {}
);

extern_protocol!(
    /// UIKit refuses a scene delegate class that doesn't conform to `UISceneDelegate`, which
    /// this protocol inherits.
    #[name = "UIWindowSceneDelegate"]
    unsafe trait WindowSceneDelegate: NSObjectProtocol {}
);

define_class!(
    // SAFETY: `UIResponder` is made to be subclassed, and `AppDelegate` has no `Drop`.
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RuneAppDelegate"]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    // SAFETY: every method of the protocol is optional.
    unsafe impl ApplicationDelegate for AppDelegate {}

    impl AppDelegate {
        #[unsafe(method_id(application:configurationForConnectingSceneSession:options:))]
        fn configuration(&self, _application: &AnyObject, session: &AnyObject, _options: &AnyObject) -> Retained<AnyObject> {
            // SAFETY: a session's role is an `NSString`, and `UISceneConfiguration`'s
            // initializer and `delegateClass` take these types.
            unsafe {
                let role: Retained<AnyObject> = msg_send![session, role];
                let configuration: Allocated<AnyObject> = msg_send![class!(UISceneConfiguration), alloc];
                let configuration: Retained<AnyObject> = msg_send![configuration, initWithName: &*NSString::from_str("Rune"), sessionRole: &*role];
                let _: () = msg_send![&*configuration, setDelegateClass: SceneDelegate::class()];
                configuration
            }
        }
    }
);

/// The scene's window, kept for as long as the scene has it.
struct SceneIvars {
    window: RefCell<Option<Retained<AnyObject>>>,
}

define_class!(
    // SAFETY: `UIResponder` is made to be subclassed, and `SceneDelegate` has no `Drop`.
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RuneSceneDelegate"]
    #[ivars = SceneIvars]
    struct SceneDelegate;

    unsafe impl NSObjectProtocol for SceneDelegate {}

    // SAFETY: every method of the protocol is optional.
    unsafe impl WindowSceneDelegate for SceneDelegate {}

    impl SceneDelegate {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Option<Retained<Self>> {
            let this = this.set_ivars(SceneIvars { window: RefCell::new(None) });
            // SAFETY: `init` is `UIResponder`'s designated initializer.
            unsafe { msg_send![super(this), init] }
        }

        #[unsafe(method(scene:willConnectToSession:options:))]
        fn will_connect(&self, scene: &AnyObject, _session: &AnyObject, _options: &AnyObject) {
            // One app, one window: a second scene has nothing to show.
            let Some(mount) = MOUNT.take() else { return };
            let view = RuneView::new(MainThreadMarker::from(self), mount);
            // SAFETY: the scene of a window-scene session is a `UIWindowScene`; a view
            // controller's view and a window's root controller are settable objects.
            let window = unsafe {
                let window: Allocated<AnyObject> = msg_send![class!(UIWindow), alloc];
                let window: Retained<AnyObject> = msg_send![window, initWithWindowScene: scene];
                let controller: Retained<AnyObject> = msg_send![class!(UIViewController), new];
                let _: () = msg_send![&*controller, setView: &*view];
                let _: () = msg_send![&*window, setRootViewController: &*controller];
                let _: () = msg_send![&*window, makeKeyAndVisible];
                // The app's one view hears a hardware keyboard from the start.
                let _: bool = msg_send![&*view, becomeFirstResponder];
                window
            };
            *self.ivars().window.borrow_mut() = Some(window);
        }

        #[unsafe(method(sceneDidDisconnect:))]
        fn did_disconnect(&self, _scene: &AnyObject) {
            self.ivars().window.borrow_mut().take();
        }
    }
);

/// Runs `app` as the whole iOS app. Returns only when it can't start: `UIApplicationMain`
/// never does.
pub(crate) fn run<S: 'static>(app: App<S>) -> Result<(), String> {
    if MainThreadMarker::new().is_none() {
        return Err("an iOS app runs from the main thread".into());
    }
    MOUNT.set(Some(surface::mount(app)));
    // UIKit finds the delegates by name, so their classes are registered first.
    let _ = (AppDelegate::class(), SceneDelegate::class());
    // SAFETY: the process's own argument vector, and the name of a class registered above.
    unsafe { UIApplicationMain(*_NSGetArgc(), *_NSGetArgv(), None, Some(&NSString::from_str("RuneAppDelegate"))) };
    Err("UIApplicationMain returned".into())
}
