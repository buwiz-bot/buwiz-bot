//! Device-owner auth before every secret read: LocalAuthentication's
//! `LAPolicyDeviceOwnerAuthentication` (Touch ID, an Apple Watch, or the
//! account password when there is no sensor), ported from nativechat
//! `src/site_login/touch_id.rs` onto the objc2 that nsh already links.
//!
//! It is `-[LAContext evaluatePolicy:localizedReason:reply:]`, not an
//! access control on the keychain item (that needs the data-protection
//! keychain and an entitlement a dev build lacks). So the gate is in this
//! process: it stops a bystander, not code running as the same user.
//!
//! [`OwnerGate`] is the seam: the vault asks through it, and tests answer
//! with a fixed outcome instead of a sheet.

/// What the sheet came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthOutcome {
    Verified,
    /// Cancelled, or left up past the timeout.
    Cancelled,
    /// The Mac cannot ask (the OS's sentence).
    Unavailable(String),
    /// The sheet ran and the OS refused (the OS's sentence).
    Failed(String),
}

pub(crate) trait OwnerGate: Send + Sync {
    /// Blocks until the sheet closes: never call it on the UI thread.
    /// `reason` finishes "Buwiz Bot is trying to …".
    fn confirm(&self, reason: &str) -> AuthOutcome;
}

/// The real sheet.
pub(crate) struct DeviceOwner;

impl OwnerGate for DeviceOwner {
    fn confirm(&self, reason: &str) -> AuthOutcome {
        #[cfg(target_os = "macos")]
        {
            imp::ask(reason)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = reason;
            AuthOutcome::Unavailable("device-owner auth needs macOS".into())
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use nsh_platform_macos::{
        block2::RcBlock,
        objc2::{
            msg_send,
            rc::{Retained, autoreleasepool},
            runtime::{AnyClass, AnyObject, Bool},
        },
        objc2_foundation::NSString,
    };

    use super::AuthOutcome;

    #[link(name = "LocalAuthentication", kind = "framework")]
    unsafe extern "C" {}

    /// `LAPolicyDeviceOwnerAuthentication`.
    const DEVICE_OWNER_AUTHENTICATION: isize = 2;

    /// How long the sheet may stay up (the person walked away).
    const SHEET_TIMEOUT: Duration = Duration::from_secs(120);

    /// One sheet at a time: a second ask would stack two system dialogs.
    static SHEET_UP: AtomicBool = AtomicBool::new(false);

    pub(super) fn ask(reason: &str) -> AuthOutcome {
        if SHEET_UP.swap(true, Ordering::SeqCst) {
            return AuthOutcome::Unavailable("another Touch ID sheet is already up".into());
        }
        let outcome = autoreleasepool(|_| prompt(reason));
        SHEET_UP.store(false, Ordering::SeqCst);
        outcome
    }

    fn prompt(reason: &str) -> AuthOutcome {
        let Some(cls) = AnyClass::get(c"LAContext") else {
            return AuthOutcome::Unavailable("LocalAuthentication is not available".into());
        };
        // SAFETY: documented LocalAuthentication selectors on an LAContext we
        // own, with their documented argument types; the reply block is
        // retained by the context for as long as it needs it.
        unsafe {
            let ctx: Option<Retained<AnyObject>> = msg_send![cls, new];
            let Some(ctx) = ctx else {
                return AuthOutcome::Unavailable("LAContext could not be created".into());
            };
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let can: Bool = msg_send![&*ctx, canEvaluatePolicy: DEVICE_OWNER_AUTHENTICATION, error: &mut error];
            if !can.as_bool() {
                return AuthOutcome::Unavailable(describe(error));
            }
            let (tx, rx) = mpsc::channel::<(bool, String)>();
            let reply = RcBlock::new(move |ok: Bool, err: *mut AnyObject| {
                let why = if ok.as_bool() { String::new() } else { describe(err) };
                let _ = tx.send((ok.as_bool(), why));
            });
            let text = NSString::from_str(reason);
            let () = msg_send![
                &*ctx,
                evaluatePolicy: DEVICE_OWNER_AUTHENTICATION,
                localizedReason: &*text,
                reply: &*reply
            ];
            match rx.recv_timeout(SHEET_TIMEOUT) {
                Ok((true, _)) => AuthOutcome::Verified,
                Ok((false, why)) if is_cancel(&why) => AuthOutcome::Cancelled,
                Ok((false, why)) => AuthOutcome::Failed(why),
                Err(_) => {
                    let () = msg_send![&*ctx, invalidate];
                    AuthOutcome::Cancelled
                }
            }
        }
    }

    /// An NSError's `localizedDescription`.
    fn describe(error: *mut AnyObject) -> String {
        if error.is_null() {
            return "no reason given".into();
        }
        // SAFETY: a non-null NSError handed over by LocalAuthentication,
        // alive for this call.
        let text: Option<Retained<NSString>> = unsafe { msg_send![&*error, localizedDescription] };
        text.map(|t| t.to_string()).unwrap_or_else(|| "no reason given".into())
    }

    /// "Canceled by user", "Cancelled", "Fallback"… depending on the OS:
    /// any of them is the person choosing not to go on.
    pub(super) fn is_cancel(why: &str) -> bool {
        let w = why.to_ascii_lowercase();
        w.contains("cancel") || w.contains("user interaction is required")
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn a_cancelled_sheet_is_a_cancel_whatever_the_os_calls_it() {
        assert!(super::imp::is_cancel("Canceled by user."));
        assert!(super::imp::is_cancel("Cancelled"));
        assert!(!super::imp::is_cancel("Biometry is not available on this device."));
    }
}
