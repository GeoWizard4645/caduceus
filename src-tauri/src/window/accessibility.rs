//! Raw Accessibility (AX) bindings and the small amount of Core Foundation
//! plumbing they need.
//!
//! # Why this is hand-written FFI
//!
//! `AXUIElement` lives in HIServices, inside the ApplicationServices umbrella.
//! There is no Rust binding in Caduceus's existing dependency tree, and pulling
//! one in for six functions would add a crate to every build for no gain. The
//! surface used here is six AX calls and eight CF calls, all of them stable
//! since 10.2.
//!
//! # Why it lives in the main binary
//!
//! Accessibility is granted per *code signature*, not per app bundle. The
//! speech helpers in `bin/` are separately (ad-hoc) signed, so anything they did
//! through AX would need its own entry in System Settings — one that a rebuild
//! invalidates, because an ad-hoc signature changes with the binary. Calling AX
//! from `Caduceus.app` itself means there is exactly one switch for the user to
//! find, and it is the one with Caduceus's name on it.
//!
//! # Safety
//!
//! Every `*mut c_void` here is a `CFTypeRef`. The rule this module follows is
//! the Core Foundation ownership rule and nothing cleverer:
//!
//! * anything returned from a `Create` or `Copy` function is owned, and is
//!   wrapped in [`CfType`] on the way out so it is released exactly once;
//! * anything returned from a `Get` function is borrowed and is never released.

#![allow(non_snake_case, non_upper_case_globals)]

use std::ffi::c_void;

// ---------------------------------------------------------------------------
// Core Foundation
// ---------------------------------------------------------------------------

pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFArrayRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFIndex = isize;
pub type CFTypeID = usize;

/// `kCFStringEncodingUTF8`.
const UTF8: u32 = 0x0800_0100;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;

    fn CFStringCreateWithBytes(
        alloc: CFTypeRef,
        bytes: *const u8,
        num_bytes: CFIndex,
        encoding: u32,
        is_external: bool,
    ) -> CFStringRef;
    fn CFStringGetTypeID() -> CFTypeID;
    fn CFStringGetLength(s: CFStringRef) -> CFIndex;
    fn CFStringGetCString(s: CFStringRef, buffer: *mut u8, size: CFIndex, encoding: u32) -> bool;

    fn CFArrayGetTypeID() -> CFTypeID;
    fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(array: CFArrayRef, index: CFIndex) -> CFTypeRef;

    fn CFDictionaryGetValue(dict: CFDictionaryRef, key: CFTypeRef) -> CFTypeRef;

    fn CFNumberGetValue(number: CFTypeRef, the_type: i32, value: *mut c_void) -> bool;

    fn CFBooleanGetValue(boolean: CFTypeRef) -> bool;

    static kCFBooleanTrue: CFTypeRef;
    static kCFBooleanFalse: CFTypeRef;
}

/// `kCFNumberSInt64Type`.
const CF_NUMBER_SINT64: i32 = 4;

/// An owned `CFTypeRef`, released on drop.
///
/// Constructed only from `Create`/`Copy` results, which is what makes the
/// unconditional `CFRelease` in [`Drop`] correct.
pub struct CfType(CFTypeRef);

impl CfType {
    /// Take ownership of a `Create`/`Copy` result.
    ///
    /// # Safety
    /// `ptr` must be null or a `CFTypeRef` the caller owns a reference to.
    pub unsafe fn from_owned(ptr: CFTypeRef) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self(ptr))
        }
    }

    /// Retain a borrowed (`Get`-result) reference so it can outlive its owner.
    ///
    /// # Safety
    /// `ptr` must be null or a live `CFTypeRef`.
    pub unsafe fn retain(ptr: CFTypeRef) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self(CFRetain(ptr)))
        }
    }

    pub fn as_ptr(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for CfType {
    fn drop(&mut self) {
        // Safe by construction: `CfType` is only ever built around a reference
        // this value owns.
        unsafe { CFRelease(self.0) }
    }
}

/// A `CFString` built from a Rust string, released on drop.
pub struct CfString(CFStringRef);

impl CfString {
    pub fn new(value: &str) -> Self {
        let bytes = value.as_bytes();
        // SAFETY: `bytes` is a valid slice for `bytes.len()` bytes, and UTF-8 is
        // exactly what a `&str` guarantees.
        let raw = unsafe {
            CFStringCreateWithBytes(
                std::ptr::null(),
                bytes.as_ptr(),
                bytes.len() as CFIndex,
                UTF8,
                false,
            )
        };
        Self(raw)
    }

    pub fn as_ptr(&self) -> CFStringRef {
        self.0
    }
}

impl Drop for CfString {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: created by `CFStringCreateWithBytes`, so we own it.
            unsafe { CFRelease(self.0) }
        }
    }
}

/// Read a borrowed `CFString` into a Rust `String`.
///
/// # Safety
/// `value` must be null or a live `CFStringRef`.
pub unsafe fn cf_string_to_rust(value: CFTypeRef) -> Option<String> {
    if value.is_null() || CFGetTypeID(value) != CFStringGetTypeID() {
        return None;
    }
    let len = CFStringGetLength(value);
    // Worst case for UTF-8 is 3 bytes per UTF-16 unit, plus the terminator.
    // Surrogate pairs are two units producing four bytes, so this bound holds.
    let capacity = (len * 3 + 1).max(1) as usize;
    let mut buffer = vec![0u8; capacity];
    if !CFStringGetCString(value, buffer.as_mut_ptr(), capacity as CFIndex, UTF8) {
        return None;
    }
    let end = buffer.iter().position(|b| *b == 0).unwrap_or(0);
    buffer.truncate(end);
    String::from_utf8(buffer).ok()
}

/// Read a borrowed `CFNumber` as an `i64`.
///
/// # Safety
/// `value` must be null or a live `CFNumberRef`.
pub unsafe fn cf_number_to_i64(value: CFTypeRef) -> Option<i64> {
    if value.is_null() {
        return None;
    }
    let mut out: i64 = 0;
    if CFNumberGetValue(value, CF_NUMBER_SINT64, (&mut out) as *mut i64 as *mut c_void) {
        Some(out)
    } else {
        None
    }
}

/// Read a borrowed `CFBoolean`.
///
/// # Safety
/// `value` must be null or a live `CFBooleanRef`.
pub unsafe fn cf_bool_to_rust(value: CFTypeRef) -> Option<bool> {
    if value.is_null() {
        return None;
    }
    Some(CFBooleanGetValue(value))
}

/// Look a key up in a borrowed `CFDictionary`.
///
/// # Safety
/// `dict` must be null or a live `CFDictionaryRef`.
pub unsafe fn cf_dict_get(dict: CFDictionaryRef, key: &str) -> CFTypeRef {
    if dict.is_null() {
        return std::ptr::null();
    }
    let key = CfString::new(key);
    CFDictionaryGetValue(dict, key.as_ptr())
}

/// Iterate a borrowed `CFArray`.
///
/// # Safety
/// `array` must be null or a live `CFArrayRef`.
pub unsafe fn cf_array_items(array: CFArrayRef) -> Vec<CFTypeRef> {
    if array.is_null() || CFGetTypeID(array) != CFArrayGetTypeID() {
        return Vec::new();
    }
    let count = CFArrayGetCount(array);
    (0..count).map(|i| CFArrayGetValueAtIndex(array, i)).collect()
}

pub fn cf_boolean(value: bool) -> CFTypeRef {
    // SAFETY: reading two constant globals the framework guarantees are set.
    unsafe {
        if value {
            kCFBooleanTrue
        } else {
            kCFBooleanFalse
        }
    }
}

// ---------------------------------------------------------------------------
// Accessibility
// ---------------------------------------------------------------------------

pub type AXUIElementRef = *const c_void;
pub type AXValueRef = *const c_void;
/// `AXError`; `kAXErrorSuccess` is 0.
pub type AXError = i32;

pub const kAXErrorSuccess: AXError = 0;
/// `kAXErrorAPIDisabled` — the process is not trusted for Accessibility.
pub const kAXErrorAPIDisabled: AXError = -25211;
/// `kAXErrorCannotComplete` — usually an app that is not responding to AX.
pub const kAXErrorCannotComplete: AXError = -25204;

/// `AXValueType` discriminants.
pub const kAXValueCGPointType: u32 = 1;
pub const kAXValueCGSizeType: u32 = 2;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout: f32) -> AXError;
    fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> AXError;
    fn AXValueCreate(the_type: u32, value: *const c_void) -> AXValueRef;
    fn AXValueGetValue(value: AXValueRef, the_type: u32, out: *mut c_void) -> bool;
}

/// Whether Caduceus currently holds the Accessibility permission.
///
/// Never prompts. The prompting variant (`AXIsProcessTrustedWithOptions`) is
/// deliberately not used: it shows a system dialog with a "Open System
/// Settings" button and no way to suppress it, which is the wrong thing to fire
/// off the back of a palette keystroke. Caduceus asks in its own words instead,
/// and sends you to the pane.
pub fn is_trusted() -> bool {
    // SAFETY: no arguments, no ownership, safe to call from any thread.
    unsafe { AXIsProcessTrusted() }
}

/// An owned `AXUIElement`.
pub struct AxElement(AXUIElementRef);

impl AxElement {
    /// The system-wide element, the root of every AX query.
    pub fn system_wide() -> Option<Self> {
        // SAFETY: returns an owned reference or null.
        let raw = unsafe { AXUIElementCreateSystemWide() };
        Self::adopt(raw)
    }

    /// The AX element for a running process.
    pub fn for_pid(pid: i32) -> Option<Self> {
        // SAFETY: returns an owned reference or null.
        let raw = unsafe { AXUIElementCreateApplication(pid) };
        Self::adopt(raw)
    }

    fn adopt(raw: AXUIElementRef) -> Option<Self> {
        if raw.is_null() {
            return None;
        }
        // An app that has stopped servicing its AX port would otherwise hang
        // the calling thread for the default 6 seconds. Half a second is far
        // longer than a healthy app needs and short enough that a wedged one
        // reports an error rather than freezing the palette.
        // SAFETY: `raw` is a live element.
        unsafe { AXUIElementSetMessagingTimeout(raw, 0.5) };
        Some(Self(raw))
    }

    pub fn as_ptr(&self) -> AXUIElementRef {
        self.0
    }

    /// Copy an attribute, returning the owned value.
    pub fn attribute(&self, name: &str) -> Option<CfType> {
        let key = CfString::new(name);
        let mut value: CFTypeRef = std::ptr::null();
        // SAFETY: `self.0` is live, `key` outlives the call, and `value` is a
        // valid out-pointer. A success result transfers ownership to us.
        let err = unsafe { AXUIElementCopyAttributeValue(self.0, key.as_ptr(), &mut value) };
        if err != kAXErrorSuccess {
            return None;
        }
        // SAFETY: a successful copy hands us an owned reference.
        unsafe { CfType::from_owned(value) }
    }

    /// Copy an attribute that is itself an AX element.
    pub fn element_attribute(&self, name: &str) -> Option<AxElement> {
        let value = self.attribute(name)?;
        let raw = value.as_ptr();
        // Take the reference out of the `CfType` wrapper rather than retaining
        // and dropping: the element owns it from here.
        std::mem::forget(value);
        AxElement::adopt(raw)
    }

    pub fn string_attribute(&self, name: &str) -> Option<String> {
        let value = self.attribute(name)?;
        // SAFETY: `value` is live for the duration of the call.
        unsafe { cf_string_to_rust(value.as_ptr()) }
    }

    pub fn bool_attribute(&self, name: &str) -> Option<bool> {
        let value = self.attribute(name)?;
        // SAFETY: `value` is live for the duration of the call.
        unsafe { cf_bool_to_rust(value.as_ptr()) }
    }

    pub fn point_attribute(&self, name: &str) -> Option<CGPoint> {
        let value = self.attribute(name)?;
        let mut point = CGPoint::default();
        // SAFETY: `value` is an AXValue and `point` matches `kAXValueCGPointType`.
        let ok = unsafe {
            AXValueGetValue(
                value.as_ptr(),
                kAXValueCGPointType,
                (&mut point) as *mut CGPoint as *mut c_void,
            )
        };
        ok.then_some(point)
    }

    pub fn size_attribute(&self, name: &str) -> Option<CGSize> {
        let value = self.attribute(name)?;
        let mut size = CGSize::default();
        // SAFETY: `value` is an AXValue and `size` matches `kAXValueCGSizeType`.
        let ok = unsafe {
            AXValueGetValue(
                value.as_ptr(),
                kAXValueCGSizeType,
                (&mut size) as *mut CGSize as *mut c_void,
            )
        };
        ok.then_some(size)
    }

    pub fn set_point(&self, name: &str, point: CGPoint) -> AXError {
        let key = CfString::new(name);
        // SAFETY: `point` matches the declared type; `AXValueCreate` copies it.
        let value = unsafe {
            AXValueCreate(kAXValueCGPointType, (&point) as *const CGPoint as *const c_void)
        };
        if value.is_null() {
            return kAXErrorCannotComplete;
        }
        // SAFETY: `value` is owned here and released below.
        let err = unsafe { AXUIElementSetAttributeValue(self.0, key.as_ptr(), value) };
        // SAFETY: created by `AXValueCreate`, released exactly once.
        unsafe { CFRelease(value) };
        err
    }

    pub fn set_size(&self, name: &str, size: CGSize) -> AXError {
        let key = CfString::new(name);
        // SAFETY: `size` matches the declared type; `AXValueCreate` copies it.
        let value = unsafe {
            AXValueCreate(kAXValueCGSizeType, (&size) as *const CGSize as *const c_void)
        };
        if value.is_null() {
            return kAXErrorCannotComplete;
        }
        // SAFETY: `value` is owned here and released below.
        let err = unsafe { AXUIElementSetAttributeValue(self.0, key.as_ptr(), value) };
        // SAFETY: created by `AXValueCreate`, released exactly once.
        unsafe { CFRelease(value) };
        err
    }

    pub fn set_bool(&self, name: &str, value: bool) -> AXError {
        let key = CfString::new(name);
        // SAFETY: `cf_boolean` returns a framework constant, not an owned value.
        unsafe { AXUIElementSetAttributeValue(self.0, key.as_ptr(), cf_boolean(value)) }
    }

    pub fn perform(&self, action: &str) -> AXError {
        let name = CfString::new(action);
        // SAFETY: `self.0` is live and `name` outlives the call.
        unsafe { AXUIElementPerformAction(self.0, name.as_ptr()) }
    }
}

impl Drop for AxElement {
    fn drop(&mut self) {
        // SAFETY: `AxElement` only ever wraps an owned, non-null reference.
        unsafe { CFRelease(self.0) }
    }
}

// AX elements are ordinary CF objects; the framework serialises access to the
// target process behind its own messaging port, and every use here is either
// confined to one call or guarded by the caller.
unsafe impl Send for AxElement {}

/// Turn an `AXError` into a sentence a user can act on.
pub fn describe_error(err: AXError) -> String {
    match err {
        kAXErrorSuccess => "Done.".into(),
        kAXErrorAPIDisabled => {
            "Caduceus does not have Accessibility permission yet. Grant it in System Settings → \
             Privacy & Security → Accessibility."
                .into()
        }
        kAXErrorCannotComplete => {
            "That window did not respond. Some apps (and most Java or Electron windows in a \
             modal state) refuse to be moved."
                .into()
        }
        -25205 => "That window does not allow itself to be moved or resized.".into(),
        -25200 => "The window went away before Caduceus could move it.".into(),
        other => format!("The window manager returned error {other}."),
    }
}
