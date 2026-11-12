//! Text entry on iOS: the keyboard's copy of the focused field (`doc`), where the field's
//! characters are (`geometry`), and the `UITextInput` responder that answers UIKit from both
//! (`input`).

pub(crate) mod doc;
pub(crate) mod geometry;
#[cfg(target_os = "ios")]
pub(crate) mod input;
