// 显示器枚举与管理

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, HMONITOR, MONITOR_DEFAULTTONEAREST};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

/// 获取窗口所在的显示器
///
/// # Arguments
/// * `hwnd` - 窗口句柄
///
/// # Returns
/// * `HMONITOR` - 窗口所在的显示器句柄
pub fn get_window_monitor(hwnd: HWND) -> HMONITOR {
    unsafe {
        // SAFETY: MonitorFromWindow 总是返回有效的 HMONITOR
        // MONITOR_DEFAULTTONEAREST 确保即使窗口不在任何显示器上也返回最近的显示器
        MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)
    }
}

/// 启用 DPI 感知（仅用于测试或需要强制开启的场景）
///
/// 确保捕获的是显示器的物理分辨率，而不是被缩放后的逻辑分辨率
pub fn enable_dpi_awareness() {
    unsafe {
        // SAFETY: 这是一个 best-effort 调用。
        // 如果进程已经设置了 DPI 感知模式（例如被 GUI 框架设置过），
        // 此调用会返回 FALSE (E_ACCESSDENIED)。我们显式忽略此错误，
        // 因为我们的目标只是确保它被开启，而不是必须由我们开启。
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, MONITORINFO, MONITORINFOEXW,
    };

    /// 单元测试：验证显示器枚举功能
    /// 这是一个调试辅助测试，用于验证系统能正确检测显示器
    #[test]
    fn test_monitor_enumeration() {
        enable_dpi_awareness();

        let monitors = enumerate_monitors();

        // 验证至少有一个显示器
        assert!(!monitors.is_empty(), "应该至少检测到一个显示器");

        // 验证有且仅有一个主显示器
        let primary_count = monitors.iter().filter(|m| m.is_primary).count();
        assert_eq!(primary_count, 1, "应该有且仅有一个主显示器");

        // 打印显示器信息（用于调试）
        println!("\n🖥️  检测到 {} 个显示器:", monitors.len());
        for (i, monitor) in monitors.iter().enumerate() {
            println!(
                "  [{}] {} {}x{} {}",
                i,
                monitor.name,
                monitor.width,
                monitor.height,
                if monitor.is_primary {
                    "⭐ 主显示器"
                } else {
                    ""
                }
            );

            // 验证分辨率合理
            assert!(monitor.width > 0, "显示器宽度必须大于 0");
            assert!(monitor.height > 0, "显示器高度必须大于 0");
        }
    }

    // --- 测试辅助结构和函数 ---

    /// 显示器信息
    #[derive(Debug)]
    struct MonitorInfo {
        handle: HMONITOR,
        name: String,
        is_primary: bool,
        width: i32,
        height: i32,
    }

    /// 枚举所有显示器
    fn enumerate_monitors() -> Vec<MonitorInfo> {
        unsafe {
            let mut monitors = Vec::new();

            let _ = EnumDisplayMonitors(
                Some(HDC::default()),
                None,
                Some(enum_monitors_proc),
                LPARAM(&mut monitors as *mut _ as isize),
            );

            monitors
        }
    }

    unsafe extern "system" fn enum_monitors_proc(
        hmonitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let monitors = &mut *(lparam.0 as *mut Vec<MonitorInfo>);

        // 获取显示器信息
        let mut monitor_info = MONITORINFOEXW {
            monitorInfo: MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
                ..Default::default()
            },
            ..Default::default()
        };

        if GetMonitorInfoW(hmonitor, &mut monitor_info.monitorInfo as *mut _ as *mut _).as_bool() {
            let name = String::from_utf16_lossy(&monitor_info.szDevice)
                .trim_end_matches('\0')
                .to_string();

            let is_primary = (monitor_info.monitorInfo.dwFlags & 1) != 0; // MONITORINFOF_PRIMARY = 1

            let width =
                monitor_info.monitorInfo.rcMonitor.right - monitor_info.monitorInfo.rcMonitor.left;
            let height =
                monitor_info.monitorInfo.rcMonitor.bottom - monitor_info.monitorInfo.rcMonitor.top;

            monitors.push(MonitorInfo {
                handle: hmonitor,
                name,
                is_primary,
                width,
                height,
            });
        }

        BOOL(1) // 继续枚举
    }
}
