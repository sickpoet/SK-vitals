use std::mem::zeroed;

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::GetSystemTimes;

/// 告警等级：托盘图标颜色与 tooltip 文案都由它决定
#[derive(Clone, Copy, PartialEq, Eq)]
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

// 阈值：改这里即可调整告警时机（百分比）
const CPU_WARN: u8 = 80;
const CPU_CRITICAL: u8 = 95;
const MEM_WARN: u8 = 80;
const MEM_CRITICAL: u8 = 92;

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub cpu: u8,
    pub mem: u8,
    pub cpu_level: Level,
    pub mem_level: Level,
}

impl Snapshot {
    pub fn level(&self) -> Level {
        self.cpu_level.worst(self.mem_level)
    }

    pub fn tooltip(&self) -> String {
        let mut tip = format!("SK-vitals\nCPU {}% · 内存 {}%", self.cpu, self.mem);
        match (self.cpu_level, self.mem_level) {
            (Level::Normal, Level::Normal) => {}
            (cpu, mem) => {
                tip.push('\n');
                if cpu != Level::Normal {
                    tip.push_str(if cpu == Level::Critical { "CPU 过高" } else { "CPU 偏高" });
                }
                if mem != Level::Normal {
                    if cpu != Level::Normal {
                        tip.push(' ');
                    }
                    tip.push_str(if mem == Level::Critical { "内存吃紧" } else { "内存偏紧" });
                }
            }
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
            cpu_level: level_of(cpu, CPU_WARN, CPU_CRITICAL),
            mem_level: level_of(mem, MEM_WARN, MEM_CRITICAL),
        }
    }
}
