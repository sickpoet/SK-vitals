use std::mem::zeroed;

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::GetSystemTimes;

/// 告警等级：托盘图标颜色与 tooltip 文案都由它决定
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Normal,
    Warn,
    Critical,
}

impl Level {
    fn rank(self) -> u8 {
        match self {
            Level::Normal => 0,
            Level::Warn => 1,
            Level::Critical => 2,
        }
    }

    fn worst(self, other: Level) -> Level {
        if self.rank() >= other.rank() {
            self
        } else {
            other
        }
    }
}

// 阈值：改这里即可调整告警时机（百分比 / 摄氏度）
const CPU_WARN: u8 = 80;
const CPU_CRITICAL: u8 = 95;
const MEM_WARN: u8 = 80;
const MEM_CRITICAL: u8 = 92;
const TEMP_WARN: u8 = 80;
const TEMP_CRITICAL: u8 = 95;

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub cpu: u8,
    pub mem: u8,
    /// CPU 温度（摄氏度）；没有可用数据源时为 None
    pub temp: Option<u8>,
    /// 温度栏是否显示（关闭时 tooltip 里也不带温度）
    pub show_temp: bool,
    pub cpu_level: Level,
    pub mem_level: Level,
    pub temp_level: Level,
}

impl Snapshot {
    pub fn level(&self) -> Level {
        self.cpu_level.worst(self.mem_level).worst(self.temp_level)
    }

    /// 填入温度并重算温度等级
    pub fn with_temp(mut self, temp: Option<u8>, show_temp: bool) -> Self {
        self.show_temp = show_temp;
        self.temp_level = match temp {
            Some(t) if show_temp => level_of(t, TEMP_WARN, TEMP_CRITICAL),
            _ => Level::Normal,
        };
        self.temp = if show_temp { temp } else { None };
        self
    }

    pub fn tooltip(&self) -> String {
        let mut tip = format!("SK-vitals\nCPU {}% · 内存 {}%", self.cpu, self.mem);
        if self.show_temp {
            let temp_text = match self.temp {
                Some(t) => format!("{}°C", t),
                None => "--".to_string(),
            };
            tip.push_str(&format!(" · 温度 {}", temp_text));
        }
        let mut notes = String::new();
        if self.cpu_level != Level::Normal {
            notes.push_str(if self.cpu_level == Level::Critical {
                "CPU 过高"
            } else {
                "CPU 偏高"
            });
        }
        if self.mem_level != Level::Normal {
            if !notes.is_empty() {
                notes.push(' ');
            }
            notes.push_str(if self.mem_level == Level::Critical {
                "内存吃紧"
            } else {
                "内存偏紧"
            });
        }
        if self.temp_level != Level::Normal {
            if !notes.is_empty() {
                notes.push(' ');
            }
            notes.push_str(if self.temp_level == Level::Critical {
                "温度过高"
            } else {
                "温度偏高"
            });
        }
        if !notes.is_empty() {
            tip.push('\n');
            tip.push_str(&notes);
        }
        tip
    }
}

fn level_of(value: u8, warn: u8, critical: u8) -> Level {
    if value >= critical {
        Level::Critical
    } else if value >= warn {
        Level::Warn
    } else {
        Level::Normal
    }
}

fn filetime_ticks(ft: FILETIME) -> u64 {
    (ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64
}

/// 每秒采样一次；CPU 用两次 GetSystemTimes 的差值，内存直接读 MEMORYSTATUSEX
pub struct Sampler {
    prev_idle: u64,
    prev_total: u64,
    has_prev: bool,
}

impl Sampler {
    pub fn new() -> Self {
        let mut sampler = Sampler {
            prev_idle: 0,
            prev_total: 0,
            has_prev: false,
        };
        // 先取一次基线，避免第一秒显示的是开机以来的平均值
        sampler.read_cpu();
        sampler
    }

    fn read_cpu(&mut self) -> u8 {
        unsafe {
            let mut idle: FILETIME = zeroed();
            let mut kernel: FILETIME = zeroed();
            let mut user: FILETIME = zeroed();
            if GetSystemTimes(&mut idle, &mut kernel, &mut user) == 0 {
                return 0;
            }
            let idle = filetime_ticks(idle);
            let total = filetime_ticks(kernel) + filetime_ticks(user);

            let mut cpu = 0u8;
            if self.has_prev {
                let idle_delta = idle.saturating_sub(self.prev_idle);
                let total_delta = total.saturating_sub(self.prev_total);
                if total_delta > 0 {
                    let busy = total_delta.saturating_sub(idle_delta);
                    cpu = ((busy as f64 / total_delta as f64) * 100.0).round() as u8;
                }
            }
            self.prev_idle = idle;
            self.prev_total = total;
            self.has_prev = true;
            cpu.min(100)
        }
    }

    pub fn sample(&mut self) -> Snapshot {
        let cpu = self.read_cpu();
        let mem = unsafe {
            let mut status: MEMORYSTATUSEX = zeroed();
            status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
            if GlobalMemoryStatusEx(&mut status) == 0 {
                0
            } else {
                status.dwMemoryLoad.min(100) as u8
            }
        };

        Snapshot {
            cpu,
            mem,
            temp: None,
            show_temp: true,
            cpu_level: level_of(cpu, CPU_WARN, CPU_CRITICAL),
            mem_level: level_of(mem, MEM_WARN, MEM_CRITICAL),
            temp_level: Level::Normal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(cpu: u8, mem: u8) -> Snapshot {
        Snapshot {
            cpu,
            mem,
            temp: None,
            show_temp: true,
            cpu_level: level_of(cpu, CPU_WARN, CPU_CRITICAL),
            mem_level: level_of(mem, MEM_WARN, MEM_CRITICAL),
            temp_level: Level::Normal,
        }
    }

    #[test]
    fn temp_raises_level() {
        let s = snap(10, 10).with_temp(Some(97), true);
        assert_eq!(s.temp_level, Level::Critical);
        assert_eq!(s.level(), Level::Critical);
    }

    #[test]
    fn no_temp_keeps_level() {
        let s = snap(96, 10).with_temp(None, true);
        assert_eq!(s.temp_level, Level::Normal);
        assert_eq!(s.level(), Level::Critical);
    }

    #[test]
    fn hidden_temp_is_ignored() {
        // 关掉温度栏后，温度不再参与告警，也不出现在 tooltip 里
        let s = snap(10, 10).with_temp(Some(99), false);
        assert_eq!(s.temp_level, Level::Normal);
        assert_eq!(s.level(), Level::Normal);
        assert!(s.temp.is_none());
        let tip = s.tooltip();
        assert!(!tip.contains("温度"), "got {}", tip);
    }

    #[test]
    fn tooltip_shows_temperature() {
        let s = snap(10, 20).with_temp(Some(55), true);
        let tip = s.tooltip();
        assert!(tip.contains("温度 55°C"), "got {}", tip);
        assert!(!tip.contains("温度过高"), "got {}", tip);
    }

    #[test]
    fn tooltip_marks_overheat() {
        let s = snap(10, 20).with_temp(Some(99), true);
        assert!(s.tooltip().contains("温度过高"), "got {}", s.tooltip());
    }

    #[test]
    fn tooltip_marks_missing_temperature() {
        let s = snap(10, 20).with_temp(None, true);
        assert!(s.tooltip().contains("温度 --"), "got {}", s.tooltip());
    }
}
