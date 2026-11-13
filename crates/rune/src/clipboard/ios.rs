//! The iOS pasteboard. One item holds every form a copy carries: the text, HTML, RTF and the
//! image as PNG. Files aren't written: an app's own paths mean nothing to another app's
//! sandbox. A secret stays on this device (`UIPasteboardOptionLocalOnly`), off the Universal
//! Clipboard, and the worker clears it when its time is up as everywhere else.
//!
//! Reading what another app copied makes iOS ask the user to allow the paste, and the read
//! blocks its thread until they answer. So the pasteboard is used on the clipboard worker, as
//! in the other backends and never on the main thread, which UIKit's headers do not keep
//! `UIPasteboard` to; and nothing here reads content until a paste or a read was asked for.
//! Watching a held secret reads only the change count.
use objc2_foundation_video::{NSData, NSString};
use objc2_video::rc::{Retained, autoreleasepool};
use objc2_video::runtime::AnyObject;
use objc2_video::{class, msg_send};
use zeroize::Zeroizing;

use super::html::MAX_RICH_BYTES;
use super::{Backend, ClipboardContent, ClipboardImage, ClipboardWrite, MAX_IMAGE_BYTES};

const TEXT: &str = "public.utf8-plain-text";
const HTML: &str = "public.html";
const RTF: &str = "public.rtf";
const PNG: &str = "public.png";

// SAFETY: UIKit's headers declare it as a `UIPasteboardOption`, an `NSString` that lives for
// the process.
#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    static UIPasteboardOptionLocalOnly: &'static NSString;
}

/// The general pasteboard, looked up per call: it is a shared object UIKit owns.
pub(super) struct Pasteboard;

fn general() -> Retained<AnyObject> {
    // SAFETY: `+[UIPasteboard generalPasteboard]` returns the shared pasteboard, never nil. This
    // and every other pasteboard call here runs on the clipboard worker's thread: a read of
    // another app's copy blocks until the user answers the system's paste prompt, which the
    // main thread must never do, and UIKit's header does not keep `UIPasteboard` to it.
    unsafe { msg_send![class!(UIPasteboard), generalPasteboard] }
}

/// The pasteboard's data of one type, when it has any and no more than `limit` bytes.
fn data(pb: &AnyObject, kind: &str, limit: usize) -> Option<Vec<u8>> {
    // SAFETY: `dataForPasteboardType:` takes a type identifier and returns `NSData` or nil. Off
    // the main thread, as in `general`.
    let data: Option<Retained<NSData>> = unsafe { msg_send![pb, dataForPasteboardType: &*NSString::from_str(kind)] };
    data.filter(|d| !d.is_empty() && d.len() <= limit).map(|d| d.to_vec())
}

impl Backend for Pasteboard {
    fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String> {
        let content = &copy.content;
        autoreleasepool(|_| {
            // SAFETY: a mutable dictionary of type identifiers to `NSString` and `NSData`
            // values is a pasteboard item, and the options dictionary maps
            // `UIPasteboardOptionLocalOnly` to a boolean number. The dictionaries, the array
            // and the number are this thread's own; the pasteboard is used off the main
            // thread, as in `general`.
            unsafe {
                let item: Retained<AnyObject> = msg_send![class!(NSMutableDictionary), new];
                let set = |value: &AnyObject, kind: &str| { let _: () = msg_send![&*item, setObject: value, forKey: &*NSString::from_str(kind)]; };
                let text = content.text();
                if !text.is_empty() || !content.is_rich() { set(&NSString::from_str(text), TEXT); }
                if let Some(html) = content.html() { set(&NSString::from_str(html), HTML); }
                if let Some(rtf) = content.rtf() { set(&NSData::with_bytes(rtf.as_bytes()), RTF); }
                if let Some(image) = content.image() { set(&NSData::with_bytes(image.png()), PNG); }
                let items: Retained<AnyObject> = msg_send![class!(NSArray), arrayWithObject: &*item];
                let options: Retained<AnyObject> = msg_send![class!(NSMutableDictionary), new];
                if copy.is_secret() {
                    let yes: Retained<AnyObject> = msg_send![class!(NSNumber), numberWithBool: true];
                    let _: () = msg_send![&*options, setObject: &*yes, forKey: UIPasteboardOptionLocalOnly];
                }
                let _: () = msg_send![&*general(), setItems: &*items, options: &*options];
            }
            Ok(())
        })
    }

    fn text(&mut self) -> Option<Zeroizing<String>> {
        // SAFETY: `string` is the pasteboard's first string, or nil. Off the main thread, as in
        // `general`.
        let text: Option<Retained<NSString>> = autoreleasepool(|_| unsafe { msg_send![&*general(), string] });
        text.map(|text| Zeroizing::new(text.to_string()))
    }

    fn content(&mut self, all: bool) -> Option<ClipboardContent> {
        autoreleasepool(|_| {
            let pb = general();
            let text = self.text();
            let html = data(&pb, HTML, MAX_RICH_BYTES).and_then(|d| String::from_utf8(d).ok());
            let rtf = data(&pb, RTF, MAX_RICH_BYTES).map(Zeroizing::new);
            let image = if all { data(&pb, PNG, MAX_IMAGE_BYTES).and_then(ClipboardImage::from_png) } else { None };
            if text.is_none() && html.is_none() && rtf.is_none() && image.is_none() { return None; }
            let mut content = ClipboardContent::from(text.unwrap_or_default());
            if let Some(html) = html { content = content.with_html(html); }
            if let Some(rtf) = rtf { content = content.with_rtf(String::from_utf8_lossy(&rtf)); }
            if let Some(image) = image { content = content.with_image(image); }
            Some(content)
        })
    }

    fn revision(&self) -> Option<u64> {
        // SAFETY: `changeCount` is an `NSInteger`. Off the main thread, as in `general`.
        let count: isize = unsafe { msg_send![&*general(), changeCount] };
        Some(count as u64)
    }

    fn clear(&mut self) -> Result<(), String> {
        // SAFETY: `setItems:` takes an `NSArray` of items, here none, which empties the
        // pasteboard. Off the main thread, as in `general`.
        unsafe {
            let none: Retained<AnyObject> = msg_send![class!(NSArray), array];
            let _: () = msg_send![&*general(), setItems: &*none];
        }
        Ok(())
    }
}
