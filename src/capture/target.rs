// Capture target resolution: monitor index → HMONITOR, process name + index → HWND

use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
};

// ---------------------------------------------------------------------------
// DPI
// ---------------------------------------------------------------------------

/// Enable Per-Monitor DPI awareness
///
/// Ensures capturing physical resolution rather than scaled logical resolution.
/// Repeated calls are safe (silently ignored if already set).
pub fn enable_dpi_awareness() {
    unsafe {
        // SAFETY: best-effort call, failure indicates it was already set
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

// ---------------------------------------------------------------------------
// Monitor lookup
// ---------------------------------------------------------------------------

/// Find monitor by index
///
/// Indices are ordered by system enumeration order, not guaranteed that `0` is the primary monitor.
pub fn find_monitor(index: usize) -> Result<HMONITOR> {
    let monitors = enumerate_monitors()?;

    if monitors.is_empty() {
        bail!("No monitors detected");
    }

    monitors.get(index).copied().with_context(|| {
        format!(
            "Monitor index {} out of range (found {})",
            index,
            monitors.len()
        )
    })
}

// --- Internal enumeration ---

fn enumerate_monitors() -> Result<Vec<HMONITOR>> {
    unsafe {
        let mut monitors = Vec::new();
        let ok = EnumDisplayMonitors(
            Some(HDC::default()),
            None,
            Some(enum_monitor_proc),
            LPARAM(&mut monitors as *mut _ as isize),
        );

        if !ok.as_bool() {
            bail!("EnumDisplayMonitors failed");
        }

        Ok(monitors)
    }
}

unsafe extern "system" fn enum_monitor_proc(
    hmonitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    // SAFETY: lparam points to a Vec<HMONITOR> on the caller's stack in enumerate_monitors().
    // The Vec's lifetime spans the entire EnumDisplayMonitors call, and the callback
    // executes synchronously on the same thread.
    let monitors = &mut *(lparam.0 as *mut Vec<HMONITOR>);
    monitors.push(hmonitor);
    BOOL(1)
}

// ---------------------------------------------------------------------------
// Window lookup
// ---------------------------------------------------------------------------

/// Find window by process name
///
/// Enumerates all visible top-level windows belonging to the specified process, selecting by `index`.
/// `index` defaults to 0 (first matching window).
///
/// # Examples
/// ```no_run
/// # use hdrcapture::capture::find_window;
/// let hwnd = find_window("notepad.exe", None).unwrap();       // first notepad window
/// let hwnd = find_window("notepad.exe", Some(1)).unwrap();    // second
/// ```
pub fn find_window(process_name: &str, index: Option<usize>) -> Result<HWND> {
    let index = index.unwrap_or(0);

    // Phase 1: Collect target PIDs via process snapshot
    let pids = get_pids_by_name(process_name)?;
    if pids.is_empty() {
        bail!("No running process found for \"{}\"", process_name);
    }

    // Phase 2: Enumerate windows, filter quickly using PID set
    let windows = enumerate_windows_by_pids(&pids);
    if windows.is_empty() {
        bail!("No visible windows found for process \"{}\"", process_name);
    }

    windows.get(index).copied().with_context(|| {
        format!(
            "Window index {} out of range for \"{}\" (found {})",
            index,
            process_name,
            windows.len()
        )
    })
}

// --- Phase 1: PID collection ---

/// Get all PIDs matching the process name via process snapshot
fn get_pids_by_name(process_name: &str) -> Result<HashSet<u32>> {
    let target = process_name.to_lowercase();
    let mut pids = HashSet::new();

    unsafe {
        // SAFETY: Win32 API call, HANDLE must be closed after use.
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .context("CreateToolhelp32Snapshot failed")?;

        // RAII guard: CloseHandle on drop, even if we return early via `?`
        struct SnapshotGuard(HANDLE);
        impl Drop for SnapshotGuard {
            fn drop(&mut self) {
                // SAFETY: self.0 is a valid snapshot handle from CreateToolhelp32Snapshot.
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
        let _guard = SnapshotGuard(snapshot);

        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let name = String::from_utf16_lossy(&entry.szExeFile)
                    .trim_end_matches('\0')
                    .to_lowercase();

                if name == target {
                    pids.insert(entry.th32ProcessID);
                }

                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
    }

    Ok(pids)
}

// --- Phase 2: Window matching ---

fn enumerate_windows_by_pids(pids: &HashSet<u32>) -> Vec<HWND> {
    unsafe {
        let mut ctx = EnumCtx {
            pids,
            results: Vec::new(),
        };

        let _ = EnumWindows(Some(enum_window_proc), LPARAM(&mut ctx as *mut _ as isize));

        ctx.results
    }
}

struct EnumCtx<'a> {
    pids: &'a HashSet<u32>,
    results: Vec<HWND>,
}

unsafe extern "system" fn enum_window_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam points to an EnumCtx on the caller's stack in enumerate_windows_by_pids().
    // Same lifetime and single-thread guarantees as enum_monitor_proc.
    let ctx = &mut *(lparam.0 as *mut EnumCtx);

    if !IsWindowVisible(hwnd).as_bool() {
        return BOOL(1);
    }

    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));

    if pid != 0 && ctx.pids.contains(&pid) {
        ctx.results.push(hwnd);
    }

    BOOL(1)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_monitor_index_0() {
        enable_dpi_awareness();
        let hmonitor = find_monitor(0).unwrap();
        assert!(!hmonitor.0.is_null(), "Monitor handle should be valid");
    }

    #[test]
    fn test_find_monitor_out_of_range() {
        let result = find_monitor(999);
        assert!(result.is_err());
    }

    #[test]
    fn test_find_window_not_found() {
        let result = find_window("nonexistent_process_12345.exe", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_find_window_index_out_of_range() {
        // explorer.exe usually exists, but won't have 999 windows
        let result = find_window("explorer.exe", Some(999));
        assert!(result.is_err());
    }
}
