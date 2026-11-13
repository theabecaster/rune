//! Work for the main thread from anywhere: the main dispatch queue, which a window's event
//! loop drains on macOS and UIKit's run loop on iOS.

use std::ffi::c_void;

#[link(name = "System", kind = "dylib")]
unsafe extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(queue: *const u8, context: *mut c_void, work: extern "C" fn(*mut c_void));
    #[cfg(target_os = "ios")]
    fn dispatch_time(when: u64, delta: i64) -> u64;
    #[cfg(target_os = "ios")]
    fn dispatch_after_f(when: u64, queue: *const u8, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

type Job = Box<dyn FnOnce() + Send>;

/// `f` as the context a dispatch function takes, which `run` frees.
fn context(f: impl FnOnce() + Send + 'static) -> *mut c_void {
    let job: Job = Box::new(f);
    Box::into_raw(Box::new(job)).cast()
}

extern "C" fn run(context: *mut c_void) {
    // SAFETY: `context` is the box `context()` leaked, delivered exactly once.
    let job = unsafe { Box::from_raw(context.cast::<Job>()) };
    job();
}

/// Run `f` on the main queue: a view's wake from another thread on iOS, an icon or a video's
/// turn on macOS.
pub(crate) fn on_main(f: impl FnOnce() + Send + 'static) {
    // SAFETY: the main queue outlives the process; `run` frees the context.
    unsafe { dispatch_async_f(&raw const _dispatch_main_q, context(f), run) };
}

/// Run `f` on the main queue `seconds` from now.
#[cfg(target_os = "ios")]
pub(crate) fn after_main(seconds: f64, f: impl FnOnce() + Send + 'static) {
    /// `DISPATCH_TIME_NOW`.
    const NOW: u64 = 0;
    let nanoseconds = (seconds.max(0.0) * 1e9).min(i64::MAX as f64) as i64;
    // SAFETY: the main queue outlives the process; `run` frees the context.
    unsafe { dispatch_after_f(dispatch_time(NOW, nanoseconds), &raw const _dispatch_main_q, context(f), run) };
}
