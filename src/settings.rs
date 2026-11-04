//! 持久化设置
//!
//! 存在 `HKCU\Software\SK-vitals` 下，只需要当前用户权限。
//! 用 REG_DWORD 存 0/1；读不出来时用默认值（第一次运行）。
//! 开机自启沿用 `HKCU\...\Run`，和设置分开。

use std::mem::size_of;
use std::ptr::null;

use windows_sys::Win32::System::Registry::*;

use crate::ui::wide;

const KEY_PATH: &str = r"Software\SK-vitals";
/// 启动时是否显示悬浮窗
const VAL_PANEL_ON_START: &str = "ShowPanelOnStart";
/// 是否显示温度栏
const VAL_SHOW_TEMP: &str = "ShowTemp";

/// HKCU 下的 Run 项，不需要管理员权限
const AUTOSTART_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const AUTOSTART_VALUE: &str = "SK-vitals";

#[derive(Clone, Copy)]
pub struct Settings {
    /// 启动时显示悬浮窗
    pub panel_on_start: bool,
    /// 显示温度栏
    pub show_temp: bool,
}

impl Settings {
    pub fn load() -> Settings {
        Settings {
            panel_on_start: read_bool(VAL_PANEL_ON_START).unwrap_or(true),
            show_temp: read_bool(VAL_SHOW_TEMP).unwrap_or(true),
        }
    }

    pub fn save(&self) {
        write_bool(VAL_PANEL_ON_START, self.panel_on_start);
        write_bool(VAL_SHOW_TEMP, self.show_temp);
    }
}

fn read_bool(name: &str) -> Option<bool> {
    unsafe {
        let mut hkey: HKEY = 0;
        if RegOpenKeyExW(HKEY_CURRENT_USER, wide(KEY_PATH).as_ptr(), 0, KEY_READ, &mut hkey) != 0 {
            return None;
        }
        let mut kind: REG_VALUE_TYPE = 0;
        let mut buf = [0u8; 4];
        let mut size = buf.len() as u32;
        let status = RegQueryValueExW(
            hkey,
            wide(name).as_ptr(),
            null(),
            &mut kind,
            buf.as_mut_ptr(),
            &mut size,
        );
        RegCloseKey(hkey);
        if status != 0 || size == 0 {
            return None;
        }
        match buf[0] {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
}

fn write_bool(name: &str, value: bool) {
    unsafe {
        let mut hkey: HKEY = 0;
        if RegCreateKeyW(HKEY_CURRENT_USER, wide(KEY_PATH).as_ptr(), &mut hkey) != 0 {
            return;
        }
        let data: u32 = if value { 1 } else { 0 };
        RegSetValueExW(
            hkey,
            wide(name).as_ptr(),
            0,
            REG_DWORD,
            &data as *const u32 as *const u8,
            4,
        );
        RegCloseKey(hkey);
    }
}

/// 读 HKCU\...\Run，看本程序的启动项在不在
pub fn is_autostart_enabled() -> bool {
    unsafe {
        let mut hkey: HKEY = 0;
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(AUTOSTART_KEY).as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        ) != 0
        {
            return false;
        }

        let name = wide(AUTOSTART_VALUE);
        let mut kind: REG_VALUE_TYPE = 0;
        let mut buf = [0u16; 512];
        let mut size = (size_of::<u16>() * buf.len()) as u32;
        let status = RegQueryValueExW(
            hkey,
            name.as_ptr(),
            null(),
            &mut kind,
            buf.as_mut_ptr() as *mut u8,
            &mut size,
        );
        RegCloseKey(hkey);
        status == 0
    }
}

/// 写入 / 删除开机启动项，失败返回 false
pub fn set_autostart(enabled: bool, exe: &str) -> bool {
    unsafe {
        let mut hkey: HKEY = 0;
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(AUTOSTART_KEY).as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        ) != 0
        {
            return false;
        }

        let name = wide(AUTOSTART_VALUE);
        let ok = if enabled {
            // --startup 用来区分「开机自启启动」和「用户手动启动」：
            // 前者才受「启动时显示悬浮窗」开关影响，手动双击一定显示小窗。
            // 路径带空格时必须加引号
            let value = wide(&format!("\"{}\" --startup", exe));
            let bytes = (value.len() * size_of::<u16>()) as u32;
            RegSetValueExW(hkey, name.as_ptr(), 0, REG_SZ, value.as_ptr() as *const u8, bytes) == 0
        } else {
            // 2 = ERROR_FILE_NOT_FOUND，本来就没有也算成功
            let status = RegDeleteValueW(hkey, name.as_ptr());
            status == 0 || status == 2
        };

        RegCloseKey(hkey);
        ok
    }
}
