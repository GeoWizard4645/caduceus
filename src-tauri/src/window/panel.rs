//! Turning Caduceus's windows into `NSPanel`s, which is what makes them work
//! over another app's full-screen Space.
//!
//! # Why an ordinary window is not enough
//!
//! Collection behaviour and window level put a window *on screen* in a
//! full-screen Space. Neither gives it the **keyboard**. A plain `NSWindow` can
//! only be the key window while its application is the active one, and Caduceus
//! is an Accessory app that is never active — so the honest way to focus the
//! palette is `activateIgnoringOtherApps:`, and activating an app while another
//! one owns a full-screen Space is precisely the thing that yanks the user out
//! of that Space. The palette then arrives somewhere you were not looking.
//!
//! `NSWindowStyleMask::NonactivatingPanel` is the flag that resolves it: a panel
//! carrying it becomes key **without its application becoming active**. That is
//! the whole trick behind Spotlight, Alfred and Raycast, and the one piece the
//! previous implementation was missing.
//!
//! # Why this re-classes a live window
//!
//! Tauri builds every window as an `NSWindow` subclass (`TaoWindow`) and offers
//! no way to ask for an `NSPanel` instead. `object_setClass` swaps the class of
//! the already-created window, which is how every Tauri launcher does this.
//!
//! The swap is guarded rather than assumed:
//!
//! * the replacement is only ever installed if it is **no larger** than the
//!   class it replaces, because a bigger object would read past its allocation;
//! * failure is non-fatal and returns `false` — the caller keeps the old
//!   behaviour, which is a palette that does not follow you into full screen
//!   rather than a crash.
//!
//! One known consequence, recorded rather than solved: by the time this runs
//! the window is usually a `NSKVONotifying_TaoWindow`, the class KVO installs
//! when something starts observing it. Replacing that class detaches those
//! observers. It is survivable here because Caduceus never closes these two
//! windows — `CloseRequested` hides them — so neither is ever deallocated with
//! a stale observer attached. Anything that later needs to *observe* one of
//! these windows should do so through AppKit's delegate callbacks rather than
//! KVO.
//!
//! # The two archetypes, from the same primitive
//!
//! | window         | key?  | why                                              |
//! |----------------|-------|--------------------------------------------------|
//! | Command Center | yes   | you type into it the instant it opens            |
//! | staff          | never | it is a HUD; clicking it must not take your focus |

use std::ffi::CStr;
use std::ptr;
use std::sync::OnceLock;

use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use objc2_app_kit::{NSWindow, NSWindowStyleMask};

/// Runtime class for the Command Center: takes keys, never activates the app.
const COMMAND_PANEL_CLASS: &CStr = c"CaduceusCommandPanel";
/// Runtime class for the staff: clickable, but never the key window.
const STAFF_PANEL_CLASS: &CStr = c"CaduceusStaffPanel";

extern "C" fn yes(_this: &AnyObject, _cmd: Sel) -> Bool {
    Bool::YES
}

extern "C" fn no(_this: &AnyObject, _cmd: Sel) -> Bool {
    Bool::NO
}

/// Which of the two panel personalities a window should take on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The Command Center: becomes key so it can be typed into.
    Command,
    /// The staff: clickable, but never takes the keyboard.
    Staff,
}

impl Kind {
    fn class_name(self) -> &'static CStr {
        match self {
            Self::Command => COMMAND_PANEL_CLASS,
            Self::Staff => STAFF_PANEL_CLASS,
        }
    }
}

/// Register (once) the `NSPanel` subclass backing a [`Kind`].
fn panel_class(kind: Kind) -> Option<&'static AnyClass> {
    static COMMAND: OnceLock<Option<&'static AnyClass>> = OnceLock::new();
    static STAFF: OnceLock<Option<&'static AnyClass>> = OnceLock::new();

    let cell = match kind {
        Kind::Command => &COMMAND,
        Kind::Staff => &STAFF,
    };

    *cell.get_or_init(|| {
        let name = kind.class_name();
        // A previous registration in this image wins: `objc_allocateClassPair`
        // refuses a duplicate name and would leave us with nothing.
        if let Some(existing) = AnyClass::get(name) {
            return Some(existing);
        }

        let mut builder = ClassBuilder::new(name, class!(NSPanel))?;
        // SAFETY: both selectors are `- (BOOL)` with no arguments, which is
        // exactly the signature of the functions being installed.
        unsafe {
            match kind {
                Kind::Command => {
                    builder.add_method(
                        sel!(canBecomeKeyWindow),
                        yes as extern "C" fn(_, _) -> _,
                    );
                    // Key, never main. Main-window status is what tells AppKit
                    // this is the application's front document window, and
                    // claiming it is how a panel ends up pulling Caduceus into
                    // the foreground — which, from inside a full-screen Space,
                    // means pulling the user out of it.
                    builder.add_method(
                        sel!(canBecomeMainWindow),
                        no as extern "C" fn(_, _) -> _,
                    );
                }
                Kind::Staff => {
                    builder.add_method(sel!(canBecomeKeyWindow), no as extern "C" fn(_, _) -> _);
                    builder.add_method(sel!(canBecomeMainWindow), no as extern "C" fn(_, _) -> _);
                }
            }
        }
        Some(builder.register())
    })
}

/// An `NSWindow` seen as the plain object it is, for class-level work.
///
/// A pointer cast rather than `AsRef`: every Objective-C object *is* an
/// `AnyObject`, and spelling it out keeps this readable next to the
/// `object_setClass` call it exists for.
fn as_object(window: &NSWindow) -> &AnyObject {
    // SAFETY: `NSWindow` is a `#[repr(C)]` Objective-C class type, so a
    // reference to one is a reference to an Objective-C object.
    unsafe { &*(window as *const NSWindow as *const AnyObject) }
}

/// Whether this window has already been re-classed as one of our panels.
pub fn is_panel(window: &NSWindow) -> bool {
    let name = as_object(window).class().name();
    name == COMMAND_PANEL_CLASS || name == STAFF_PANEL_CLASS
}

/// Re-class `window` as the panel subclass for `kind`. Idempotent.
///
/// Returns whether the window is (now) one of our panels. `false` means the
/// swap was refused and the caller is still dealing with a plain window.
pub fn ensure_panel(window: &NSWindow, kind: Kind) -> bool {
    let Some(target) = panel_class(kind) else {
        log::warn!("could not register the {kind:?} panel class");
        return false;
    };

    let object = as_object(window);
    let current = object.class();
    if ptr::eq(current, target) {
        return true;
    }

    // The one hard requirement of `object_setClass`: the new class must not
    // want more storage than the object actually has. AppKit has never grown
    // NSPanel past NSWindow, but "has never" is not "cannot", and the failure
    // mode is memory corruption rather than a bug report.
    if target.instance_size() > current.instance_size() {
        log::warn!(
            "refusing to re-class {} ({} bytes) as {} ({} bytes)",
            current.name().to_string_lossy(),
            current.instance_size(),
            target.name().to_string_lossy(),
            target.instance_size(),
        );
        return false;
    }

    // SAFETY: `target` is a registered NSPanel subclass that adds no instance
    // variables and is no larger than the class it replaces (checked above).
    // Its two overrides are `- (BOOL)` methods that AppKit calls to ask a
    // question, so answering differently is always valid.
    unsafe {
        objc2::ffi::object_setClass(
            object as *const AnyObject as *mut AnyObject,
            target as *const AnyClass,
        );
    }

    // Worth a line in the log. It happens once per window, it is the single
    // thing standing between Caduceus and being invisible inside a full-screen
    // Space, and if it ever stops happening the symptom — a hotkey that does
    // nothing — gives no hint at all about where to look.
    log::info!(
        "{} is now a {}",
        current.name().to_string_lossy(),
        target.name().to_string_lossy(),
    );
    true
}

/// Apply the panel-only settings that keep an overlay out of the user's way.
///
/// Must run on the main thread. Does nothing unless [`ensure_panel`] has already
/// accepted this window — every message below is `NSPanel`'s, not `NSWindow`'s,
/// and sending them to a plain window is an unrecognised-selector crash.
pub fn configure_panel(window: &NSWindow, nonactivating: bool) {
    if !is_panel(window) {
        return;
    }

    let mask = window.styleMask();
    let next = if nonactivating {
        mask | NSWindowStyleMask::NonactivatingPanel
    } else {
        mask & !NSWindowStyleMask::NonactivatingPanel
    };
    if next != mask {
        window.setStyleMask(next);
    }

    // A utility panel hides itself when its app is deactivated, which for an
    // app that is *never* active means "always".
    window.setHidesOnDeactivate(false);

    let object = as_object(window);
    // SAFETY: all three are documented `NSPanel` setters taking a single BOOL,
    // and this only ever runs on a window that is now an `NSPanel`.
    unsafe {
        // Floating panels stay above their app's ordinary windows and, with the
        // level set alongside, above everyone else's.
        let _: () = msg_send![object, setFloatingPanel: true];
        // Otherwise the panel declines key status until something in it insists
        // it needs the keyboard — a race the search field loses on open.
        let _: () = msg_send![object, setBecomesKeyOnlyIfNeeded: false];
        // Another app's modal sheet should not make the palette unreachable.
        let _: () = msg_send![object, setWorksWhenModal: true];
    }
}

/// Order the panel forward and give it the keyboard, without activating the app.
///
/// This is the part that behaves differently over a full-screen Space:
/// `makeKeyAndOrderFront:` on a non-activating panel raises and focuses it where
/// the user is looking, instead of switching Spaces to wherever Caduceus is.
pub fn present_without_activating(window: &NSWindow) {
    // `orderFrontRegardless` first: it is the one that ignores "the app is not
    // active" and puts the window on screen at all.
    window.orderFrontRegardless();
    window.makeKeyAndOrderFront(None);
}
