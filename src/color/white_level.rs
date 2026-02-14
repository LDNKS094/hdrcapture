// SDR white level query via Windows DisplayConfig API.
//
// Returns the SDR content brightness (nits) configured for a given monitor.
// Used to normalize scRGB pixel values before tone-mapping.

use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SDR_WHITE_LEVEL, DISPLAYCONFIG_SOURCE_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFOEXW};

/// Default SDR white level in nits (ITU-R BT.709 reference white).
pub const DEFAULT_SDR_WHITE_NITS: f32 = 80.0;

/// Query the SDR white level (nits) for the given monitor.
///
/// Falls back to `DEFAULT_SDR_WHITE_NITS` (80.0) if the query fails
/// (e.g. older Windows, non-HDR monitor, or API error).
pub fn query_sdr_white_level(monitor: HMONITOR) -> f32 {
    get_sdr_white_nits(monitor).unwrap_or(DEFAULT_SDR_WHITE_NITS)
}

/// Internal: resolve HMONITOR → device name → DisplayConfig path → SDR white level.
fn get_sdr_white_nits(monitor: HMONITOR) -> Option<f32> {
    let device_name = monitor_device_name(monitor)?;
    let path = find_display_config_path(&device_name)?;
    let nits = query_white_level_from_path(&path)?;
    Some(nits)
}

/// Get the GDI device name for a monitor handle.
fn monitor_device_name(monitor: HMONITOR) -> Option<[u16; 32]> {
    // SAFETY: GetMonitorInfoW writes to a caller-provided MONITORINFOEXW.
    // cbSize must be set correctly before the call.
    unsafe {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(monitor, &mut info.monitorInfo).as_bool() {
            return None;
        }
        Some(info.szDevice)
    }
}

/// Find the DISPLAYCONFIG_PATH_INFO matching a GDI device name.
fn find_display_config_path(device_name: &[u16; 32]) -> Option<DISPLAYCONFIG_PATH_INFO> {
    // SAFETY: GetDisplayConfigBufferSizes and QueryDisplayConfig are Win32 APIs
    // that write to caller-provided buffers. We allocate sufficient space based
    // on the returned counts.
    unsafe {
        let mut num_paths = 0u32;
        let mut num_modes = 0u32;
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut num_paths, &mut num_modes)
            != ERROR_SUCCESS
        {
            return None;
        }

        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); num_paths as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); num_modes as usize];

        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut num_paths,
            paths.as_mut_ptr(),
            &mut num_modes,
            modes.as_mut_ptr(),
            None,
        ) != ERROR_SUCCESS
        {
            return None;
        }
        paths.truncate(num_paths as usize);

        // Match path by source device name
        for path in &paths {
            let mut source_name = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                    adapterId: path.sourceInfo.adapterId,
                    id: path.sourceInfo.id,
                },
                ..Default::default()
            };

            if DisplayConfigGetDeviceInfo(&mut source_name.header) != 0 {
                continue;
            }

            if source_name.viewGdiDeviceName == *device_name {
                return Some(*path);
            }
        }
    }

    None
}

/// Query SDR white level from a resolved display config path.
fn query_white_level_from_path(path: &DISPLAYCONFIG_PATH_INFO) -> Option<f32> {
    // SAFETY: DisplayConfigGetDeviceInfo writes to a caller-provided struct.
    // header.size and header.type must be set correctly.
    unsafe {
        let mut level = DISPLAYCONFIG_SDR_WHITE_LEVEL {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                size: std::mem::size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            },
            SDRWhiteLevel: 0,
        };

        if DisplayConfigGetDeviceInfo(&mut level.header) != 0 {
            return None;
        }

        // Windows returns SDRWhiteLevel where 1000 = 80 nits reference white.
        // Formula: nits = (SDRWhiteLevel * 80) / 1000
        let nits = (level.SDRWhiteLevel as f32 * 80.0) / 1000.0;
        Some(nits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::target::{enable_dpi_awareness, find_monitor};

    #[test]
    fn test_query_sdr_white_level() {
        enable_dpi_awareness();
        let monitor = find_monitor(0).expect("No monitor found");
        let nits = query_sdr_white_level(monitor);
        println!("Monitor 0 SDR white level: {} nits", nits);
        // Reasonable range: 80-400 nits (Windows slider range)
        assert!(
            (40.0..=600.0).contains(&nits),
            "SDR white level {} nits is outside expected range",
            nits
        );
    }
}
