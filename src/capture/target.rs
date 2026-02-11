// 捕获目标解析：监视器索引 → HMONITOR，窗口类名/标题 → HWND

use anyhow::{bail, Context, Result};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::FindWindowW;

// ---------------------------------------------------------------------------
// DPI
// ---------------------------------------------------------------------------

/// 启用 Per-Monitor DPI 感知
///
/// 确保捕获的是物理分辨率而非缩放后的逻辑分辨率。
/// 重复调用安全（已设置过则静默忽略）。
pub fn enable_dpi_awareness() {
    unsafe {
        // SAFETY: best-effort 调用，失败说明已被设置过
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

// ---------------------------------------------------------------------------
// 监视器查找
// ---------------------------------------------------------------------------

/// 按索引查找监视器
///
/// 索引按系统枚举顺序排列，不保证 `0` 为主显示器。
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

// --- 内部枚举 ---

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
    let monitors = &mut *(lparam.0 as *mut Vec<HMONITOR>);
    monitors.push(hmonitor);
    BOOL(1)
}

// ---------------------------------------------------------------------------
// 窗口查找
// ---------------------------------------------------------------------------

/// 按类名和/或标题查找窗口
///
/// 至少需要提供 `class` 或 `title` 之一。
/// 返回第一个匹配的窗口句柄。
pub fn find_window(class: Option<&str>, title: Option<&str>) -> Result<HWND> {
    if class.is_none() && title.is_none() {
        bail!("Must specify at least one of class or title");
    }

    unsafe {
        let class_wide: Vec<u16>;
        let class_ptr = match class {
            Some(s) => {
                class_wide = s.encode_utf16().chain(std::iter::once(0)).collect();
                windows::core::PCWSTR(class_wide.as_ptr())
            }
            None => windows::core::PCWSTR::null(),
        };

        let title_wide: Vec<u16>;
        let title_ptr = match title {
            Some(s) => {
                title_wide = s.encode_utf16().chain(std::iter::once(0)).collect();
                windows::core::PCWSTR(title_wide.as_ptr())
            }
            None => windows::core::PCWSTR::null(),
        };

        FindWindowW(class_ptr, title_ptr).context(format!(
            "Window not found (class={:?}, title={:?})",
            class, title
        ))
    }
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
        let result = find_window(Some("NonExistentClass_12345"), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_find_window_requires_param() {
        let result = find_window(None, None);
        assert!(result.is_err());
    }
}
