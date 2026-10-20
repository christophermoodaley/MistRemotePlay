//! Per-application audio on Windows: which programs are playing sound, and
//! the process IDs that WASAPI process loopback (`wasapi2src`
//! `loopback-mode`) includes or excludes.
//!
//! Apps are named by executable (`Discord.exe`). Programs such as Discord or
//! browsers play sound from a child process, so loopback targets the root
//! process of each app: process loopback always covers the whole tree below
//! it.

use serde::Serialize;

/// A program that currently has an audio session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AudioApp {
    /// Executable file name, e.g. `Discord.exe`.
    pub exe: String,
    /// Name for display (the executable without `.exe`).
    pub name: String,
}

/// One entry of the process table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessEntry {
    pub pid: u32,
    pub parent: u32,
    pub exe: String,
}

/// Whether `exe` is the app the user named (case-insensitive, `.exe`
/// optional).
pub fn same_app(exe: &str, name: &str) -> bool {
    let strip = |s: &str| {
        let s = s.trim();
        if s.len() > 4 && s[s.len() - 4..].eq_ignore_ascii_case(".exe") {
            s[..s.len() - 4].to_owned()
        } else {
            s.to_owned()
        }
    };
    strip(exe).eq_ignore_ascii_case(&strip(name))
}

/// Root processes of the named app: matching processes whose parent is not
/// itself the same app. Sorted, so a changed set can be detected.
pub fn root_pids(processes: &[ProcessEntry], name: &str) -> Vec<u32> {
    let mut out: Vec<u32> = processes
        .iter()
        .filter(|p| same_app(&p.exe, name))
        .filter(|p| !processes.iter().any(|q| q.pid == p.parent && q.pid != p.pid && same_app(&q.exe, name)))
        .map(|p| p.pid)
        .collect();
    out.sort_unstable();
    out
}

#[cfg(windows)]
pub use self::win::{audio_apps, processes};

#[cfg(windows)]
mod win {
    use super::{AudioApp, ProcessEntry};
    use windows::core::Interface;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Media::Audio::{
        eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, MMDeviceEnumerator,
        DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };

    /// Snapshot of all running processes.
    pub fn processes() -> std::io::Result<Vec<ProcessEntry>> {
        let mut out = Vec::new();
        // SAFETY: plain Win32 calls; the entry struct is sized before use
        // and the snapshot handle is closed below.
        unsafe {
            let snap =
                CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(|e| std::io::Error::other(e.to_string()))?;
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut ok = Process32FirstW(snap, &mut e).is_ok();
            while ok {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                out.push(ProcessEntry {
                    pid: e.th32ProcessID,
                    parent: e.th32ParentProcessID,
                    exe: String::from_utf16_lossy(&e.szExeFile[..len]),
                });
                ok = Process32NextW(snap, &mut e).is_ok();
            }
            let _ = CloseHandle(snap);
        }
        Ok(out)
    }

    /// Programs with an audio session on any active playback device, by
    /// executable name, without duplicates.
    pub fn audio_apps() -> std::io::Result<Vec<AudioApp>> {
        let err = |e: windows::core::Error| std::io::Error::other(e.to_string());
        let procs = processes()?;
        let mut out: Vec<AudioApp> = Vec::new();
        // SAFETY: COM calls on interfaces owned by this function. A thread
        // already initialised in another apartment mode is fine for these
        // free-threaded objects, so that result is ignored.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(err)?;
            let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE).map_err(err)?;
            for i in 0..devices.GetCount().map_err(err)? {
                let Ok(device) = devices.Item(i) else { continue };
                let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
                let Ok(sessions) = manager.GetSessionEnumerator() else { continue };
                for j in 0..sessions.GetCount().unwrap_or(0) {
                    let Ok(control) = sessions.GetSession(j) else { continue };
                    let Ok(control) = control.cast::<IAudioSessionControl2>() else { continue };
                    let Ok(pid) = control.GetProcessId() else { continue };
                    // PID 0 is the "System sounds" session.
                    let Some(p) = procs.iter().find(|p| p.pid == pid && pid != 0) else { continue };
                    if !out.iter().any(|a| a.exe.eq_ignore_ascii_case(&p.exe)) {
                        let name = p.exe.strip_suffix(".exe").or_else(|| p.exe.strip_suffix(".EXE")).unwrap_or(&p.exe);
                        out.push(AudioApp { exe: p.exe.clone(), name: name.to_owned() });
                    }
                }
            }
        }
        out.sort_by_key(|a| a.name.to_lowercase());
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, parent: u32, exe: &str) -> ProcessEntry {
        ProcessEntry { pid, parent, exe: exe.into() }
    }

    #[test]
    fn roots_skip_child_processes_of_the_same_app() {
        let procs = [
            p(10, 1, "Update.exe"),
            p(20, 10, "Discord.exe"),
            p(21, 20, "Discord.exe"),
            p(22, 20, "Discord.exe"),
            p(30, 1, "chrome.exe"),
            p(31, 30, "chrome.exe"),
            p(40, 1, "chrome.exe"),
        ];
        assert_eq!(root_pids(&procs, "discord"), vec![20]);
        assert_eq!(root_pids(&procs, "Discord.exe"), vec![20]);
        assert_eq!(root_pids(&procs, "CHROME.EXE"), vec![30, 40]);
        assert!(root_pids(&procs, "spotify.exe").is_empty());
    }

    #[test]
    fn app_names_match_with_or_without_exe() {
        assert!(same_app("Discord.exe", "discord"));
        assert!(same_app("Discord.exe", " Discord.EXE "));
        assert!(!same_app("Discord.exe", "DiscordPTB.exe"));
    }
}
