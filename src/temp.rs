//! CPU 温度读取
//!
//! Windows 没有官方的“读 CPU 温度”API，可用的数据源都是 WMI 类。这里按可靠性依次尝试：
//!
//! 1. `root\OpenHardwareMonitor` / `root\LibreHardwareMonitor` 的 `Sensor`（需要用户自己在跑 LibreHardwareMonitor）
//! 2. `root\hardware` 的 `NumericSensor`（Win11 24H2 才有）
//! 3. `root\wmi` 的 `MSAcpi_ThermalZoneTemperature`（笔记本的 ACPI 热区，单位是 0.1K）
//! 4. `root\cimv2` 的 `Win32_TemperatureProbe` / `CIM_TemperatureSensor`（单位 0.1°C）
//!
//! 全部取不到就返回 `None`，界面显示 `--`。查询放在后台线程里按低频轮询，
//! 主线程只读缓存值，不占用 UI 线程。

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use wmi::{IWbemClassWrapper, Variant, WMIConnection};

/// 缓存里表示“还没有温度”的哨兵值
const NO_VALUE: i32 = -999;

/// 轮询间隔：温度变化很慢，15 秒一次足够
const INTERVAL: Duration = Duration::from_secs(15);
/// 某个来源连续失败后休眠的轮数（15 秒 * 20 ≈ 5 分钟后再试）
const DORMANT_CYCLES: u64 = 20;

/// 后台温度读取器。主线程只调用 [`TempReader::read`] 读缓存。
pub struct TempReader {
    value: Arc<AtomicI32>,
}

impl TempReader {
    /// 启动后台轮询线程
    pub fn start() -> Self {
        let value = Arc::new(AtomicI32::new(NO_VALUE));
        let slot = value.clone();
        let _ = thread::Builder::new()
            .name("sk-vitals-temp".into())
            .stack_size(256 * 1024)
            .spawn(move || poll_loop(slot));
        TempReader { value }
    }

    /// 最近一次读到的温度（摄氏度），没有数据源时为 `None`
    pub fn read(&self) -> Option<u8> {
        match self.value.load(Ordering::Relaxed) {
            NO_VALUE => None,
            v => Some(v.clamp(0, 255) as u8),
        }
    }
}

/// 一个 WMI 数据源
struct Source {
    namespace: &'static str,
    queries: &'static [&'static str],
    /// 从查询结果里挑出 CPU 温度
    extract: fn(&[Row]) -> Option<f64>,
    conn: Option<WMIConnection>,
    fails: u32,
    /// 在这个轮次之前不再尝试
    dormant_until: u64,
}

impl Source {
    fn poll(&mut self) -> Option<f64> {
        if self.conn.is_none() {
            match WMIConnection::with_namespace_path(self.namespace) {
                Ok(conn) => self.conn = Some(conn),
                Err(_) => return None,
            }
        }
        let conn = self.conn.as_ref()?;

        let mut rows = Vec::new();
        for query in self.queries {
            let iter = match conn.exec_query(*query) {
                Ok(iter) => iter,
                Err(_) => return None,
            };
            for item in iter {
                if let Ok(obj) = item {
                    rows.push(Row::from_object(&obj));
                }
            }
        }
        (self.extract)(&rows)
    }
}

/// 一条传感器记录，屏蔽 WMI 细节，便于单独测试换算逻辑
struct Row {
    sensor_type: String,
    name: String,
    id: String,
    /// `Value`（OpenHardwareMonitor 系）
    value: Option<f64>,
    /// `CurrentReading`
    reading: Option<f64>,
    /// `BaseUnits`
    base_units: String,
    /// `UnitModifier`
    unit_modifier: Option<f64>,
}

impl Row {
    fn from_object(obj: &IWbemClassWrapper) -> Row {
        let num = |name: &str| obj.get_property(name).ok().and_then(|v| prop_num(&v));
        Row {
            sensor_type: prop_str(obj, "SensorType"),
            name: prop_str(obj, "Name"),
            id: prop_str(obj, "Identifier"),
            value: num("Value"),
            reading: num("CurrentReading"),
            base_units: prop_str(obj, "BaseUnits"),
            unit_modifier: num("UnitModifier"),
        }
    }
}

fn build_sources() -> Vec<Source> {
    vec![
        Source {
            namespace: "ROOT\\OpenHardwareMonitor",
            queries: &["SELECT * FROM Sensor"],
            extract: extract_hardware_monitor,
            conn: None,
            fails: 0,
            dormant_until: 0,
        },
        Source {
            namespace: "ROOT\\LibreHardwareMonitor",
            queries: &["SELECT * FROM Sensor"],
            extract: extract_hardware_monitor,
            conn: None,
            fails: 0,
            dormant_until: 0,
        },
        Source {
            namespace: "ROOT\\hardware",
            queries: &["SELECT * FROM NumericSensor"],
            extract: extract_numeric_sensor,
            conn: None,
            fails: 0,
            dormant_until: 0,
        },
        Source {
            namespace: "ROOT\\WMI",
            queries: &["SELECT * FROM MSAcpi_ThermalZoneTemperature"],
            extract: extract_acpi_thermal_zone,
            conn: None,
            fails: 0,
            dormant_until: 0,
        },
        Source {
            namespace: "ROOT\\CIMV2",
            queries: &[
                "SELECT * FROM Win32_TemperatureProbe",
                "SELECT * FROM CIM_TemperatureSensor",
            ],
            extract: extract_probe,
            conn: None,
            fails: 0,
            dormant_until: 0,
        },
    ]
}

fn poll_loop(cache: Arc<AtomicI32>) {
    let mut sources = build_sources();
    let mut cycle: u64 = 0;
    let mut last: Option<f64> = None;

    loop {
        for src in sources.iter_mut() {
            if cycle < src.dormant_until {
                continue;
            }
            match src.poll() {
                Some(v) => {
                    src.fails = 0;
                    last = Some(v);
                    break;
                }
                None => {
                    src.fails += 1;
                    if src.fails >= 3 {
                        // 连续失败基本说明这台机器没有这个数据源，休眠一段时间再试
                        src.dormant_until = cycle + DORMANT_CYCLES;
                        src.conn = None;
                    }
                }
            }
        }

        // 查询失败时保留上一次的值，避免温度在界面上闪烁成 `--`
        let stored = match last {
            Some(v) => v.round() as i32,
            None => NO_VALUE,
        };
        cache.store(stored, Ordering::Relaxed);

        cycle += 1;
        thread::sleep(INTERVAL);
    }
}

/// 把 WMI 变体里的数值取出来
fn prop_num(variant: &Variant) -> Option<f64> {
    match variant {
        Variant::I1(n) => Some(*n as f64),
        Variant::I2(n) => Some(*n as f64),
        Variant::I4(n) => Some(*n as f64),
        Variant::I8(n) => Some(*n as f64),
        Variant::R4(n) => Some(*n as f64),
        Variant::R8(n) => Some(*n as f64),
        Variant::UI1(n) => Some(*n as f64),
        Variant::UI2(n) => Some(*n as f64),
        Variant::UI4(n) => Some(*n as f64),
        Variant::UI8(n) => Some(*n as f64),
        Variant::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn prop_str(obj: &IWbemClassWrapper, name: &str) -> String {
    obj.get_property(name)
        .ok()
        .and_then(|v| match v {
            Variant::String(s) => Some(s),
            _ => None,
        })
        .unwrap_or_default()
}

/// 合理的 CPU 温度区间，用来过滤掉明显读错单位的数据
fn plausible(celsius: f64) -> bool {
    (-20.0..=150.0).contains(&celsius)
}

/// 给传感器打分：分数越高越像“CPU 封装温度”
fn score_sensor(name: &str, id: &str) -> i32 {
    let n = name.to_lowercase();
    let i = id.to_lowercase();
    if n.contains("package") || n.contains("tctl") || n.contains("tdie") {
        return 100;
    }
    if i.contains("/intelcpu/") || i.contains("/amdcpu/") || i.contains("/cpu/") {
        return 80;
    }
    if n.contains("core") {
        return 60;
    }
    if n.contains("cpu") {
        return 50;
    }
    0
}

/// 在候选里挑分数最高、同分取温度最高的
fn pick_best(best: Option<(i32, f64)>, score: i32, value: f64) -> Option<(i32, f64)> {
    Some(match best {
        Some((s, v)) if s > score || (s == score && v >= value) => (s, v),
        _ => (score, value),
    })
}

/// OpenHardwareMonitor / LibreHardwareMonitor 的 `Sensor` 类
fn extract_hardware_monitor(rows: &[Row]) -> Option<f64> {
    let mut best: Option<(i32, f64)> = None;
    for row in rows {
        if !row.sensor_type.eq_ignore_ascii_case("Temperature") {
            continue;
        }
        let value = match row.value {
            Some(v) if plausible(v) => v,
            _ => continue,
        };
        best = pick_best(best, score_sensor(&row.name, &row.id), value);
    }
    best.map(|(_, v)| v)
}

/// Win11 24H2 的 `root\hardware` 命名空间
fn extract_numeric_sensor(rows: &[Row]) -> Option<f64> {
    let mut best: Option<(i32, f64)> = None;
    for row in rows {
        if !row.sensor_type.eq_ignore_ascii_case("Temperature") {
            continue;
        }
        let raw = match row.reading {
            Some(v) => v,
            None => continue,
        };
        let value = match numeric_celsius(raw, &row.base_units, row.unit_modifier) {
            Some(v) => v,
            None => continue,
        };
        best = pick_best(best, score_sensor(&row.name, &row.id), value);
    }
    best.map(|(_, v)| v)
}

/// 按 CIM 规范换算温度：读数单位是 `BaseUnits * 10^UnitModifier`
fn numeric_celsius(raw: f64, base_units: &str, modifier: Option<f64>) -> Option<f64> {
    let scale = match modifier {
        Some(m) if m.abs() <= 6.0 => 10f64.powf(m),
        _ => 1.0,
    };
    let base = raw * scale;
    let units = base_units.to_lowercase();

    if units.contains("kel") {
        let celsius = base - 273.15;
        return if plausible(celsius) { Some(celsius) } else { None };
    }
    if units.contains("cel") {
        return if plausible(base) { Some(base) } else { None };
    }
    // 没有单位信息：只有在结果落在常见 CPU 温度区间时才采信
    if (5.0..=125.0).contains(&base) {
        return Some(base);
    }
    None
}

/// `root\wmi` 的 ACPI 热区，单位是 0.1K
fn extract_acpi_thermal_zone(rows: &[Row]) -> Option<f64> {
    let mut best: Option<(i32, f64)> = None;
    for row in rows {
        let raw = match row.reading {
            Some(v) => v,
            None => continue,
        };
        let celsius = raw / 10.0 - 273.15;
        if !plausible(celsius) {
            continue;
        }
        let lower = row.name.to_lowercase();
        let score = if lower.contains("cpu")
            || lower.contains("processor")
            || lower.contains("package")
        {
            100
        } else {
            0
        };
        best = pick_best(best, score, celsius);
    }
    best.map(|(_, v)| v)
}

/// `root\cimv2` 的温度探针，单位是 0.1°C
fn extract_probe(rows: &[Row]) -> Option<f64> {
    let mut best: Option<(i32, f64)> = None;
    for row in rows {
        let raw = match row.reading {
            Some(v) => v,
            None => continue,
        };
        let celsius = raw / 10.0;
        if !plausible(celsius) {
            continue;
        }
        best = pick_best(best, score_sensor(&row.name, &row.id), celsius);
    }
    best.map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, id: &str) -> Row {
        Row {
            sensor_type: "Temperature".into(),
            name: name.into(),
            id: id.into(),
            value: None,
            reading: None,
            base_units: String::new(),
            unit_modifier: None,
        }
    }

    #[test]
    fn acpi_thermal_zone_converts_tenths_of_kelvin() {
        // 3231 = 323.1K = 49.95°C
        let rows = vec![Row {
            reading: Some(3231.0),
            ..row("ACPI Thermal Zone", "")
        }];
        let v = extract_acpi_thermal_zone(&rows).unwrap();
        assert!((v - 49.95).abs() < 0.01, "got {}", v);
    }

    #[test]
    fn acpi_prefers_cpu_named_zone() {
        let rows = vec![
            Row {
                reading: Some(3300.0),
                ..row("GPU Thermal Zone", "")
            },
            Row {
                reading: Some(3200.0),
                ..row("CPU Thermal Zone", "")
            },
        ];
        let v = extract_acpi_thermal_zone(&rows).unwrap();
        assert!((v - 46.85).abs() < 0.01, "got {}", v);
    }

    #[test]
    fn probe_reads_tenths_of_celsius() {
        let rows = vec![Row {
            reading: Some(555.0),
            ..row("CPU", "")
        }];
        let v = extract_probe(&rows).unwrap();
        assert!((v - 55.5).abs() < 0.01, "got {}", v);
    }

    #[test]
    fn hardware_monitor_picks_cpu_package() {
        let rows = vec![
            Row {
                value: Some(70.0),
                ..row("GPU Core", "/gpu-nvidia/0/temperature/0")
            },
            Row {
                value: Some(48.0),
                ..row("CPU Package", "/intelcpu/0/temperature/0")
            },
        ];
        let v = extract_hardware_monitor(&rows).unwrap();
        assert!((v - 48.0).abs() < 0.01, "got {}", v);
    }

    #[test]
    fn numeric_sensor_uses_celsius_base_units() {
        let rows = vec![Row {
            reading: Some(6200.0),
            base_units: "Cel".into(),
            unit_modifier: Some(-2.0),
            ..row("CPU Package", "")
        }];
        let v = extract_numeric_sensor(&rows).unwrap();
        assert!((v - 62.0).abs() < 0.01, "got {}", v);
    }

    #[test]
    fn numeric_sensor_converts_kelvin_base_units() {
        let rows = vec![Row {
            reading: Some(3300.0),
            base_units: "Kel".into(),
            unit_modifier: Some(-1.0),
            ..row("CPU Package", "")
        }];
        let v = extract_numeric_sensor(&rows).unwrap();
        assert!((v - 56.85).abs() < 0.01, "got {}", v);
    }

    #[test]
    fn numeric_sensor_rejects_implausible_values() {
        let rows = vec![Row {
            reading: Some(999999.0),
            base_units: "Cel".into(),
            unit_modifier: None,
            ..row("CPU Package", "")
        }];
        assert!(extract_numeric_sensor(&rows).is_none());
    }

    #[test]
    fn no_rows_means_no_temperature() {
        assert!(extract_hardware_monitor(&[]).is_none());
        assert!(extract_numeric_sensor(&[]).is_none());
        assert!(extract_acpi_thermal_zone(&[]).is_none());
        assert!(extract_probe(&[]).is_none());
    }
}
