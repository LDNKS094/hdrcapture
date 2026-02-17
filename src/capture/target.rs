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
    EnumWindows, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
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

/// Unified selector input for window resolution.
pub enum WindowSelector {
    Hwnd(HWND),
    Pid(u32),
    Process(String),
}

/// Find window by unified selector + optional ranked index.
///
/// Routing:
/// - `Hwnd`: validate and return directly.
/// - `Pid`/`Process`: enumerate candidate windows, rank heuristically, then pick by index.
///
/// # Examples
/// ```no_run
/// # use hdrcapture::capture::{find_window, WindowSelector};
/// let hwnd = find_window(WindowSelector::Process("notepad.exe".to_string()), None).unwrap();
/// let hwnd = find_window(WindowSelector::Process("notepad.exe".to_string()), Some(1)).unwrap();
/// ```
pub fn find_window(selector: WindowSelector, index: Option<usize>) -> Result<HWND> {
    match selector {
        WindowSelector::Hwnd(hwnd) => validate_window(hwnd),
        WindowSelector::Pid(pid) => {
            let mut pids = HashSet::new();
            pids.insert(pid);
            pick_ranked_window(&pids, index).with_context(|| {
                let idx = index.unwrap_or(0);
                format!("Window index {} out of range for pid {}", idx, pid)
            })
        }
        WindowSelector::Process(process) => {
            let pids = get_pids(&process)?;
            if pids.is_empty() {
                bail!("No running process found for \"{}\"", process);
            }
            pick_ranked_window(&pids, index).with_context(|| {
                let idx = index.unwrap_or(0);
                format!(
                    "Window index {} out of range for process \"{}\"",
                    idx, process
                )
            })
        }
    }
}

/// Validate and normalize an HWND.
pub fn validate_window(hwnd: HWND) -> Result<HWND> {
    let ok = unsafe { IsWindow(Some(hwnd)).as_bool() };
    if !ok {
        bail!("Invalid window handle: {:?}", hwnd.0);
    }
    Ok(hwnd)
}

fn pick_ranked_window(pids: &HashSet<u32>, index: Option<usize>) -> Result<HWND> {
    let windows = enumerate_windows(pids)?;
    if windows.is_empty() {
        bail!("No candidate windows found");
    }
    pick_window(&windows, index)
}

// --- Phase 1: PID collection ---

/// Collect all PIDs whose executable name matches `process`.
///
/// Data source: Toolhelp process snapshot
/// (`CreateToolhelp32Snapshot` + `Process32FirstW/Process32NextW`).
/// Matching is case-insensitive exact match on executable file name.
fn get_pids(process: &str) -> Result<HashSet<u32>> {
    let target = process.to_lowercase();
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

/// Enumerate top-level windows owned by `pids`, then rank by heuristic score.
///
/// This function does not hard-filter candidates by visibility/tool/minimized state.
/// Those signals only affect ranking priority. Returned list is sorted descending
/// by score and ready for index-based selection.
fn enumerate_windows(pids: &HashSet<u32>) -> Result<Vec<HWND>> {
    unsafe {
        let mut ctx = EnumCtx {
            pids,
            candidates: Vec::new(),
        };

        let ok = EnumWindows(Some(enum_window_proc), LPARAM(&mut ctx as *mut _ as isize));
        if ok.is_err() {
            bail!("EnumWindows failed");
        }

        let mut ranked = ctx.candidates;
        ranked.sort_by(compare_candidate);
        Ok(ranked.into_iter().map(|c| c.hwnd).collect())
    }
}

struct EnumCtx<'a> {
    pids: &'a HashSet<u32>,
    candidates: Vec<WindowCandidate>,
}

#[derive(Clone, Copy)]
struct WindowCandidate {
    hwnd: HWND,
    score: i64,
    area: i64,
}

unsafe extern "system" fn enum_window_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam points to an EnumCtx on the caller's stack in enumerate_windows_by_pids().
    // Same lifetime and single-thread guarantees as enum_monitor_proc.
    let ctx = &mut *(lparam.0 as *mut EnumCtx);

    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));

    if pid != 0 && ctx.pids.contains(&pid) {
        let visible = IsWindowVisible(hwnd).as_bool();
        let minimized = IsIconic(hwnd).as_bool();
        let exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let tool = (exstyle & WS_EX_TOOLWINDOW.0) != 0;

        let mut rect = RECT::default();
        let area = if GetWindowRect(hwnd, &mut rect).is_ok() {
            let w = (rect.right - rect.left).max(0) as i64;
            let h = (rect.bottom - rect.top).max(0) as i64;
            w * h
        } else {
            0
        };

        let mut score = 0i64;
        if visible {
            score += 10_000;
        }
        if !tool {
            score += 3_000;
        }
        if !minimized {
            score += 1_000;
        }
        score += (area / 10_000).min(5_000);

        ctx.candidates.push(WindowCandidate { hwnd, score, area });
    }

    BOOL(1)
}

/// Compare two candidates for stable priority ordering.
///
/// Order keys:
/// 1) score (desc)
/// 2) area (desc)
/// 3) hwnd value (asc, deterministic tie-break)
fn compare_candidate(a: &WindowCandidate, b: &WindowCandidate) -> std::cmp::Ordering {
    b.score
        .cmp(&a.score)
        .then_with(|| b.area.cmp(&a.area))
        .then_with(|| a.hwnd.0.cmp(&b.hwnd.0))
}

/// Pick one window from a ranked candidate list.
///
/// `index = None` selects the first (highest-ranked) window.
/// `index = Some(n)` selects the n-th item in the ranked list.
fn pick_window(windows: &[HWND], index: Option<usize>) -> Result<HWND> {
    if windows.is_empty() {
        bail!("No candidate windows found");
    }
    let idx = index.unwrap_or(0);
    windows.get(idx).copied().with_context(|| {
        format!(
            "Window index {} out of range (found {})",
            idx,
            windows.len()
        )
    })
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
        let result = find_window(
            WindowSelector::Process("nonexistent_process_12345.exe".to_string()),
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_find_window_index_out_of_range() {
        // explorer.exe usually exists, but won't have 999 windows
        let result = find_window(
            WindowSelector::Process("explorer.exe".to_string()),
            Some(999),
        );
        assert!(result.is_err());
    }
}
