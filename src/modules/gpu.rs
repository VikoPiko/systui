//! Optional NVIDIA GPU adapter via NVML (`nvml-wrapper`). The NVML library is
//! loaded at runtime; if it (or a driver) is missing the panel reports
//! "unavailable" and the rest of the app is unaffected. Every field is optional:
//! a value the driver does not report is `None`, never 0.

use super::ModStatus;
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use nvml_wrapper::error::NvmlError;
use nvml_wrapper::Nvml;
use std::time::Duration;

pub const CADENCE: Duration = Duration::from_secs(2);
/// Retry loading NVML only every N polls when it is missing.
const RETRY_EVERY: u32 = 15;
const MAX_GPUS: u32 = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct GpuInfo {
    pub index: u32,
    pub name: Option<String>,
    pub util_gpu_percent: Option<u32>,
    pub util_mem_percent: Option<u32>,
    pub mem_used: Option<u64>,
    pub mem_total: Option<u64>,
    pub temp_c: Option<u32>,
    pub power_watts: Option<f32>,
    pub fan_percent: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GpuData {
    pub driver: Option<String>,
    pub gpus: Vec<GpuInfo>,
    /// GPUs beyond the display limit.
    pub more: u32,
}

/// An NVML query result: unsupported/unknown becomes `None`.
fn opt<T>(r: Result<T, NvmlError>) -> Option<T> {
    r.ok()
}

fn read(nvml: &Nvml) -> Result<GpuData, ModStatus> {
    let count = nvml
        .device_count()
        .map_err(|e| ModStatus::Error(format!("NVML device_count: {e}")))?;
    let mut gpus = Vec::new();
    for i in 0..count.min(MAX_GPUS) {
        let d = nvml
            .device_by_index(i)
            .map_err(|e| ModStatus::Error(format!("NVML device {i}: {e}")))?;
        let util = opt(d.utilization_rates());
        let mem = opt(d.memory_info());
        gpus.push(GpuInfo {
            index: i,
            name: opt(d.name()).map(|n| crate::util::text::sanitize(&n)),
            util_gpu_percent: util.as_ref().map(|u| u.gpu),
            util_mem_percent: util.as_ref().map(|u| u.memory),
            mem_used: mem.as_ref().map(|m| m.used),
            mem_total: mem.as_ref().map(|m| m.total),
            temp_c: opt(d.temperature(TemperatureSensor::Gpu)),
            power_watts: opt(d.power_usage()).map(|mw| mw as f32 / 1000.0),
            fan_percent: opt(d.fan_speed(0)),
        });
    }
    Ok(GpuData {
        driver: opt(nvml.sys_driver_version()),
        gpus,
        more: count.saturating_sub(MAX_GPUS),
    })
}

/// Availability check (used by `doctor`): driver version and GPU count, or why not.
pub fn probe() -> Result<(Option<String>, u32), ModStatus> {
    match Nvml::init() {
        Ok(n) => Ok((
            opt(n.sys_driver_version()),
            n.device_count()
                .map_err(|e| ModStatus::Error(e.to_string()))?,
        )),
        Err(NvmlError::LibloadingError(_)) | Err(NvmlError::DriverNotLoaded) => Err(
            ModStatus::Unavailable("NVIDIA driver/NVML not found".into()),
        ),
        Err(e) => Err(ModStatus::Error(format!("NVML init failed: {e}"))),
    }
}

pub fn spawn() -> super::ModuleHandle<GpuData> {
    let mut nvml: Option<Nvml> = None;
    let mut skip = 0u32;
    super::ModuleHandle::spawn("gpu", CADENCE, move |_| {
        if nvml.is_none() {
            if skip > 0 {
                skip -= 1;
                return Err(ModStatus::Unavailable(
                    "NVIDIA driver/NVML not found (retrying periodically)".into(),
                ));
            }
            match Nvml::init() {
                Ok(n) => nvml = Some(n),
                Err(NvmlError::LibloadingError(_)) | Err(NvmlError::DriverNotLoaded) => {
                    skip = RETRY_EVERY;
                    return Err(ModStatus::Unavailable(
                        "NVIDIA driver/NVML not found: no NVIDIA GPU support on this machine"
                            .into(),
                    ));
                }
                Err(e) => {
                    skip = RETRY_EVERY;
                    return Err(ModStatus::Error(format!("NVML init failed: {e}")));
                }
            }
        }
        match nvml.as_ref().map(read) {
            Some(r) => r,
            None => Err(ModStatus::Pending),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Works on every machine: with a GPU it returns real data, without one it
    /// must report Unavailable (never panic, never zeros).
    #[test]
    fn real_nvml_is_either_sane_or_cleanly_unavailable() {
        match Nvml::init() {
            Err(_) => {} // no NVIDIA stack here: nothing to check beyond not panicking
            Ok(n) => {
                let d = read(&n).expect("read");
                for g in &d.gpus {
                    if let (Some(u), Some(t)) = (g.mem_used, g.mem_total) {
                        assert!(t > 0 && u <= t, "{g:?}");
                    }
                    if let Some(p) = g.util_gpu_percent {
                        assert!(p <= 100);
                    }
                    if let Some(t) = g.temp_c {
                        assert!(t > 0 && t < 150, "{t}");
                    }
                }
            }
        }
    }
}
