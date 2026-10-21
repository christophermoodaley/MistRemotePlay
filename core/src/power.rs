//! Keep the display awake while streaming.
//!
//! Without this, the screen saver (or display sleep) starts on the host
//! while someone plays with a controller, since game controllers do not
//! count as user activity on Windows, and on the client because the
//! player may touch nothing but a controller. On the host that locks the
//! session (screen capture and input stop at the lock screen); on the
//! client it covers the stream.
//!
//! A [`KeepAwake`] holds a power request for as long as it lives; dropping
//! it lets the screen saver and sleep timers run again.

pub struct KeepAwake {
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
}

// SAFETY: the handle is a kernel object usable from any thread.
#[cfg(windows)]
unsafe impl Send for KeepAwake {}
#[cfg(windows)]
unsafe impl Sync for KeepAwake {}

impl KeepAwake {
    /// Keep the display on (which also holds off the screen saver) and the
    /// PC out of sleep. `reason` shows in `powercfg /requests`.
    #[cfg(windows)]
    pub fn new(reason: &str) -> Option<KeepAwake> {
        use windows::core::PWSTR;
        use windows::Win32::System::Power::{
            PowerCreateRequest, PowerRequestDisplayRequired, PowerRequestSystemRequired, PowerSetRequest,
        };
        use windows::Win32::System::SystemServices::POWER_REQUEST_CONTEXT_VERSION;
        use windows::Win32::System::Threading::{POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT};

        let mut text: Vec<u16> = reason.encode_utf16().chain(std::iter::once(0)).collect();
        let mut context = REASON_CONTEXT {
            Version: POWER_REQUEST_CONTEXT_VERSION,
            Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
            ..Default::default()
        };
        context.Reason.SimpleReasonString = PWSTR(text.as_mut_ptr());
        // SAFETY: `context` and the string it points to outlive the call
        // (Windows copies the reason).
        let handle = match unsafe { PowerCreateRequest(&context) } {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("cannot keep the display awake: {e}");
                return None;
            }
        };
        let awake = KeepAwake { handle };
        // SAFETY: `handle` is the power request just created.
        for kind in [PowerRequestDisplayRequired, PowerRequestSystemRequired] {
            if let Err(e) = unsafe { PowerSetRequest(handle, kind) } {
                tracing::warn!("power request {kind:?} failed: {e}");
            }
        }
        tracing::debug!("display kept awake: {reason}");
        Some(awake)
    }

    #[cfg(not(windows))]
    pub fn new(_reason: &str) -> Option<KeepAwake> {
        None
    }
}

#[cfg(windows)]
impl Drop for KeepAwake {
    fn drop(&mut self) {
        // SAFETY: closing the request handle clears every request it holds.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}
