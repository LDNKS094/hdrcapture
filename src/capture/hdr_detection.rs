// HDR 状态检测 — 基于 DisplayConfig API
//
// 使用 QueryDisplayConfig + DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO
// 一次性获取所有活跃显示路径的 HDR 状态，并通过 GDI 设备名匹配到 HMONITOR。
//
// ## 设计原则
// - 无全局缓存，每次查询都获取最新状态
// - 设备状态变化时，Python 端重新创建 capture 对象即可
// - 性能：每次查询 2-3ms（创建频率低，可接受）

use anyhow::{Context, Result};
use std::collections::HashMap;
use windows::core::BOOL;
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::WIN32_ERROR;
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};

use super::types::MonitorInfo;

// ---------------------------------------------------------------------------
// Win32 辅助
// ---------------------------------------------------------------------------

/// 将 WIN32_ERROR 转换为 Result
fn check_win32(result: WIN32_ERROR, api_name: &str) -> Result<()> {
    if result.0 == 0 {
        // ERROR_SUCCESS
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "{} failed with error code: {}",
            api_name,
            result.0
        ))
    }
}

// ---------------------------------------------------------------------------
// DisplayConfig 查询
// ---------------------------------------------------------------------------

/// 通过 DisplayConfig API 查询某条路径的 HDR（Advanced Color）状态
///
/// # 位域布局 (DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO.Anonymous.value)
/// - bit 0: advancedColorSupported
/// - bit 1: advancedColorEnabled      ← 我们关心的
/// - bit 2: wideColorEnforced
/// - bit 3: advancedColorForceDisabled
unsafe fn query_advanced_color_enabled(path: &DISPLAYCONFIG_PATH_INFO) -> Result<bool> {
    // 手动构造 DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO
    // 该结构体在 windows-rs 中的内存布局：
    //   DISPLAYCONFIG_DEVICE_INFO_HEADER  header;   // 20 bytes
    //   union { struct { u32 bits }; u32 value; };   // 4 bytes
    //   DISPLAYCONFIG_COLOR_ENCODING colorEncoding;  // 4 bytes
    //   u32 bitsPerColorChannel;                     // 4 bytes
    // 总计 32 bytes
    //
    // 由于 windows-rs 对该结构体的绑定可能因版本而异，
    // 我们使用原始字节缓冲区 + 手动填充 header 的方式来保证兼容性。

    #[repr(C)]
    struct AdvancedColorInfo {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
        value: u32,          // union { bits; value; }
        color_encoding: u32, // DISPLAYCONFIG_COLOR_ENCODING
        bits_per_color_channel: u32,
    }

    let mut info = AdvancedColorInfo {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
            size: std::mem::size_of::<AdvancedColorInfo>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        },
        value: 0,
        color_encoding: 0,
        bits_per_color_channel: 0,
    };

    let ret = DisplayConfigGetDeviceInfo(
        &mut info.header as *mut _ as *mut DISPLAYCONFIG_DEVICE_INFO_HEADER,
    );

    if ret != 0 {
        return Err(anyhow::anyhow!(
            "DisplayConfigGetDeviceInfo(ADVANCED_COLOR_INFO) failed: {}",
            ret
        ));
    }

    // bit 1 = advancedColorEnabled
    let advanced_color_enabled = (info.value >> 1) & 1;
    Ok(advanced_color_enabled == 1)
}

/// 通过 DisplayConfig API 查询某条路径的 GDI 源设备名（如 `\\.\DISPLAY1`）
unsafe fn query_source_device_name(path: &DISPLAYCONFIG_PATH_INFO) -> Result<String> {
    let mut source_name = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
    source_name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
    source_name.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
    source_name.header.adapterId = path.sourceInfo.adapterId;
    source_name.header.id = path.sourceInfo.id;

    let ret = DisplayConfigGetDeviceInfo(&mut source_name.header);
    if ret != 0 {
        return Err(anyhow::anyhow!(
            "DisplayConfigGetDeviceInfo(SOURCE_NAME) failed: {}",
            ret
        ));
    }

    let name = String::from_utf16_lossy(&source_name.viewGdiDeviceName)
        .trim_end_matches('\0')
        .to_string();
    Ok(name)
}

// ---------------------------------------------------------------------------
// 显示器枚举（GDI 侧）
// ---------------------------------------------------------------------------

/// 通过 EnumDisplayMonitors 收集的原始显示器信息
struct RawMonitorInfo {
    handle: HMONITOR,
    device_name: String, // 如 \\.\DISPLAY1
    is_primary: bool,
    width: u32,
    height: u32,
}

/// 使用 GDI EnumDisplayMonitors 枚举所有显示器
fn enum_gdi_monitors() -> Vec<RawMonitorInfo> {
    let mut monitors: Vec<RawMonitorInfo> = Vec::new();

    unsafe {
        // SAFETY: EnumDisplayMonitors 回调在当前线程同步执行，
        // monitors 的生命周期覆盖整个调用过程。
        let _ = EnumDisplayMonitors(
            Some(HDC::default()),
            None,
            Some(enum_proc),
            LPARAM(&mut monitors as *mut Vec<RawMonitorInfo> as isize),
        );
    }

    monitors
}

/// EnumDisplayMonitors 回调
unsafe extern "system" fn enum_proc(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let monitors = &mut *(lparam.0 as *mut Vec<RawMonitorInfo>);

    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };

    if GetMonitorInfoW(hmonitor, &mut info.monitorInfo as *mut _ as *mut _).as_bool() {
        let device_name = String::from_utf16_lossy(&info.szDevice)
            .trim_end_matches('\0')
            .to_string();

        let is_primary = (info.monitorInfo.dwFlags & 1) != 0; // MONITORINFOF_PRIMARY

        let width = (info.monitorInfo.rcMonitor.right - info.monitorInfo.rcMonitor.left) as u32;
        let height = (info.monitorInfo.rcMonitor.bottom - info.monitorInfo.rcMonitor.top) as u32;

        monitors.push(RawMonitorInfo {
            handle: hmonitor,
            device_name,
            is_primary,
            width,
            height,
        });
    }

    BOOL(1) // 继续枚举
}

// ---------------------------------------------------------------------------
// 公开 API
// ---------------------------------------------------------------------------

/// 枚举所有显示器并获取 HDR 状态
///
/// 实现流程：
/// 1. 通过 GDI `EnumDisplayMonitors` 获取所有 HMONITOR + 设备名
/// 2. 通过 `QueryDisplayConfig` 获取所有活跃显示路径
/// 3. 对每条路径查询 GDI 源设备名，与步骤 1 匹配
/// 4. 对匹配的路径查询 `DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO`
/// 5. 更新缓存并返回 `Vec<MonitorInfo>`
pub fn enumerate_monitors() -> Result<Vec<MonitorInfo>> {
    // --- Step 1: GDI 枚举 ---
    let gdi_monitors = enum_gdi_monitors();
    if gdi_monitors.is_empty() {
        return Err(anyhow::anyhow!(
            "No monitors detected via EnumDisplayMonitors"
        ));
    }

    // --- Step 2: QueryDisplayConfig ---
    let mut path_count: u32 = 0;
    let mut mode_count: u32 = 0;

    unsafe {
        let ret =
            GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count);
        check_win32(ret, "GetDisplayConfigBufferSizes")?;
    }

    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];

    unsafe {
        let ret = QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            None,
        );
        check_win32(ret, "QueryDisplayConfig")?;
    }

    // 截断到实际返回的数量
    paths.truncate(path_count as usize);

    // --- Step 3 & 4: 建立 GDI 设备名 → HDR 状态 的映射 ---
    let mut device_hdr_map: HashMap<String, bool> = HashMap::new();

    for path in &paths {
        unsafe {
            // 获取该路径的 GDI 源设备名
            if let Ok(source_name) = query_source_device_name(path) {
                // 查询 HDR 状态
                let is_hdr = query_advanced_color_enabled(path).unwrap_or(false);
                device_hdr_map.insert(source_name, is_hdr);
            }
        }
    }

    // --- Step 5: 合并结果 ---
    let mut result = Vec::with_capacity(gdi_monitors.len());

    for mon in &gdi_monitors {
        let is_hdr = device_hdr_map
            .get(&mon.device_name)
            .copied()
            .unwrap_or(false);

        result.push(MonitorInfo::new(
            mon.handle,
            mon.device_name.clone(),
            mon.is_primary,
            mon.width,
            mon.height,
            is_hdr,
        ));
    }

    Ok(result)
}

/// 检测显示器是否处于 HDR 模式
///
/// 每次调用都会查询最新的显示器状态，无缓存。
/// 性能：2-3ms（创建 capture 对象时调用，频率低）
///
/// # Arguments
/// * `monitor` - 显示器句柄 (HMONITOR)
///
/// # Returns
/// * `Ok(true)` - 显示器处于 HDR 模式
/// * `Ok(false)` - 显示器处于 SDR 模式
///
/// # Design
/// 设备状态变化时，Python 端应重新创建 capture 对象，
/// 无需手动刷新缓存（因为没有缓存）。
pub fn is_monitor_hdr(monitor: HMONITOR) -> Result<bool> {
    let monitors =
        enumerate_monitors().context("Failed to enumerate monitors for HDR detection")?;

    monitors
        .iter()
        .find(|m| m.handle() == monitor)
        .map(|m| m.is_hdr)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Monitor 0x{:X} not found in enumeration",
                monitor.0 as isize
            )
        })
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enumerate_monitors_returns_results() {
        let monitors = enumerate_monitors().expect("enumerate_monitors should succeed");
        assert!(!monitors.is_empty(), "Should detect at least one monitor");

        println!("\n--- HDR Detection Results ---");
        for (i, mon) in monitors.iter().enumerate() {
            println!(
                "  [{}] {} {}x{} | HDR: {} {}",
                i,
                mon.name,
                mon.width,
                mon.height,
                mon.is_hdr,
                if mon.is_primary { "(Primary)" } else { "" },
            );
        }
    }

    #[test]
    fn test_is_monitor_hdr_consistency() {
        // 枚举所有显示器
        let monitors = enumerate_monitors().expect("enumerate_monitors should succeed");
        assert!(!monitors.is_empty());

        // 验证 is_monitor_hdr() 返回一致的结果
        for mon in &monitors {
            let result = is_monitor_hdr(mon.handle()).expect("is_monitor_hdr should succeed");
            assert_eq!(
                result, mon.is_hdr,
                "is_monitor_hdr() should return consistent result with enumerate_monitors()"
            );
        }
    }
}
