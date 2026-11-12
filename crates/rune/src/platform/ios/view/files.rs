//! `Tx::open_file` on iOS: the system's document picker, over the screen the view is on. The
//! picker hands back a copy of the chosen file inside the app's own container, so the path the
//! app gets is one it may read with no further permission.

use std::path::PathBuf;

use objc2_foundation_video::NSString;
use objc2_video::rc::{Allocated, Retained};
use objc2_video::runtime::AnyObject;
use objc2_video::{DefinedClass, class, msg_send};

use super::super::pacing::Event;
use super::RuneView;
use crate::file_dialog::FileDialogOptions;

#[link(name = "UniformTypeIdentifiers", kind = "framework")]
unsafe extern "C" {}

impl RuneView {
    /// Shows the picker for dialog `id`. A view on no screen answers with an error.
    pub(super) fn open_file(&self, id: u64, options: &FileDialogOptions) {
        let Some(screen) = self.view_controller() else {
            self.with_surface(|surface| surface.file_dialog_answer(id, Err("the view is on no screen".into())));
            return;
        };
        self.ivars().file_dialog.set(Some(id));
        // SAFETY: a picker that opens copies of files of these types, whose delegate the view
        // is; a view controller presents another with no completion block.
        unsafe {
            let types: Retained<AnyObject> = msg_send![class!(NSMutableArray), array];
            for extension in options.filters.iter().flat_map(|(_, extensions)| extensions) {
                let kind: Option<Retained<AnyObject>> = msg_send![class!(UTType), typeWithFilenameExtension: &*NSString::from_str(extension)];
                if let Some(kind) = kind { let _: () = msg_send![&*types, addObject: &*kind]; }
            }
            let count: usize = msg_send![&*types, count];
            if count == 0 {
                // No filter, or none the system knows: any file.
                let any: Option<Retained<AnyObject>> = msg_send![class!(UTType), typeWithIdentifier: &*NSString::from_str("public.item")];
                if let Some(any) = any { let _: () = msg_send![&*types, addObject: &*any]; }
            }
            let picker: Allocated<AnyObject> = msg_send![class!(UIDocumentPickerViewController), alloc];
            let picker: Retained<AnyObject> = msg_send![picker, initForOpeningContentTypes: &*types, asCopy: true];
            let _: () = msg_send![&*picker, setDelegate: self];
            let _: () = msg_send![&*screen, presentViewController: &*picker, animated: true, completion: std::ptr::null::<std::ffi::c_void>()];
        }
    }

    /// The picker closed with `urls` (an `NSArray<NSURL *>`), or none when it was cancelled.
    pub(super) fn file_picked(&self, urls: Option<&AnyObject>) {
        let Some(id) = self.ivars().file_dialog.take() else { return };
        // SAFETY: the array's first object is an `NSURL`, whose `path` is an `NSString` or nil.
        let path = urls.and_then(|urls| unsafe {
            let first: Option<Retained<AnyObject>> = msg_send![urls, firstObject];
            let path: Option<Retained<NSString>> = msg_send![&*first?, path];
            path.map(|path| PathBuf::from(path.to_string()))
        });
        self.with_surface(|surface| surface.file_dialog_answer(id, Ok(path)));
        self.pace(Event::Wake);
    }

    /// The view controller whose screen the view is on: the first one up the responder chain.
    fn view_controller(&self) -> Option<Retained<AnyObject>> {
        // SAFETY: `nextResponder` is a responder or nil, and `isKindOfClass:` answers a `BOOL`.
        unsafe {
            let mut next: Option<Retained<AnyObject>> = msg_send![self, nextResponder];
            while let Some(responder) = next {
                let found: bool = msg_send![&*responder, isKindOfClass: class!(UIViewController)];
                if found { return Some(responder); }
                next = msg_send![&*responder, nextResponder];
            }
            None
        }
    }
}
