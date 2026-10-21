//! Read text from the system clipboard (for pasting pairing invites
//! without the web view's clipboard permission prompt).

/// The clipboard's text, if it holds any.
#[cfg(windows)]
pub fn read_text() -> Option<String> {
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    // Another program may hold the clipboard for a moment.
    // SAFETY: plain Win32 clipboard calls; the clipboard is closed on every path.
    let opened = (0..5).any(|i| {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        unsafe { OpenClipboard(None) }.is_ok()
    });
    if !opened {
        return None;
    }
    let text = unsafe {
        let handle: Option<HANDLE> = GetClipboardData(CF_UNICODETEXT.0 as u32).ok();
        handle.and_then(|h| {
            let mem = HGLOBAL(h.0);
            let ptr = GlobalLock(mem) as *const u16;
            if ptr.is_null() {
                return None;
            }
            // Bounded by the allocation; the text ends at the first NUL.
            let max = GlobalSize(mem) / 2;
            let slice = std::slice::from_raw_parts(ptr, max);
            let len = slice.iter().position(|&c| c == 0).unwrap_or(max);
            let s = String::from_utf16_lossy(&slice[..len]);
            let _ = GlobalUnlock(mem);
            Some(s)
        })
    };
    // SAFETY: we opened it above.
    unsafe {
        let _ = CloseClipboard();
    }
    text
}

#[cfg(not(windows))]
pub fn read_text() -> Option<String> {
    None
}
