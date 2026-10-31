//! The pasteboard: copied cells and text, as plain text and as HTML.

/// The copied cells on the pasteboard as text and as an HTML table (a
/// paste into Numbers, Pages or a mail keeps the grid).
pub fn to_clipboard(tsv: &str, html: &str) {
    #[cfg(target_os = "macos")]
    {
        use nsh_platform_macos::objc2::{msg_send, rc::Retained, runtime::{AnyClass, AnyObject}};
        use nsh_platform_macos::objc2_foundation::NSString;
        let Some(class) = AnyClass::get(c"NSPasteboard") else { return };
        let (text, html) = (NSString::from_str(tsv), NSString::from_str(html));
        let (text_kind, html_kind) = (NSString::from_str("public.utf8-plain-text"), NSString::from_str("public.html"));
        // SAFETY: NSPasteboard's documented methods with arguments of their
        // types, on the main thread (the engine's).
        unsafe {
            let board: Retained<AnyObject> = msg_send![class, generalPasteboard];
            let _: isize = msg_send![&*board, clearContents];
            let _: bool = msg_send![&*board, setString: &*text, forType: &*text_kind];
            let _: bool = msg_send![&*board, setString: &*html, forType: &*html_kind];
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = html;
        nsh_core::prelude::set_clipboard(tsv.to_owned());
    }
}
