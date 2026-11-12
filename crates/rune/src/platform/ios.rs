//! iOS: an app draws in a `RuneView`, a `UIView` on a `CAMetalLayer` that a display link drives
//! while something moves. [`App::run`](crate::App::run) makes the app's window and fills it
//! with one; [`view`] hands one to a UIKit or SwiftUI app that owns its own screens.

#[cfg(target_os = "ios")]
mod accessibility;
#[cfg(target_os = "ios")]
mod app;
pub(crate) mod keys;
pub(crate) mod pacing;
#[cfg(target_os = "ios")]
mod surface;
pub(crate) mod text_doc;
#[cfg(target_os = "ios")]
mod text_input;
#[cfg(target_os = "ios")]
mod view;

#[cfg(target_os = "ios")]
pub(crate) use app::run;

/// A new `UIView *` that draws `app`, for a host that owns its own window: add it to a view
/// hierarchy, or return it from a `UIViewRepresentable`. It fills its superview, starts the app
/// when it first has a size, draws while something moves and stops in the background.
///
/// The pointer is retained: the caller releases it (`Unmanaged.fromOpaque(_:).takeRetainedValue()`
/// in Swift). Null off the main thread.
#[cfg(target_os = "ios")]
pub fn view<S: 'static>(app: crate::App<S>) -> *mut std::ffi::c_void {
    let Some(mtm) = objc2_video::MainThreadMarker::new() else { return std::ptr::null_mut() };
    objc2_video::rc::Retained::into_raw(view::RuneView::new(mtm, surface::mount(app))).cast()
}
