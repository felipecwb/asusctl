use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use zbus::zvariant::{OwnedValue, Type, Value};

use crate::error::{PlatformError, Result};
use crate::platform::PlatformProfile;
use crate::{read_attr_string, to_device};

const ATTR_AVAILABLE_GOVERNORS: &str = "cpufreq/scaling_available_governors";
const ATTR_GOVERNOR: &str = "cpufreq/scaling_governor";
const ATTR_AVAILABLE_EPP: &str = "cpufreq/energy_performance_available_preferences";
const ATTR_EPP: &str = "cpufreq/energy_performance_preference";
const CPU_SYSFS: &str = "/sys/devices/system/cpu";

/// The boost policy to apply when a platform profile is active.
#[repr(u32)]
#[derive(
    Deserialize, Serialize, Type, Value, OwnedValue, Default, Debug, PartialEq, Eq, Clone, Copy,
)]
#[zvariant(signature = "u")]
pub enum CPUBoost {
    #[default]
    LeaveUnchanged = 0,
    Enabled = 1,
    Disabled = 2,
}

impl CPUBoost {
    pub fn enabled(self) -> Option<bool> {
        match self {
            Self::LeaveUnchanged => None,
            Self::Enabled => Some(true),
            Self::Disabled => Some(false),
        }
    }
}

impl From<i32> for CPUBoost {
    fn from(value: i32) -> Self {
        match value {
            1 => Self::Enabled,
            2 => Self::Disabled,
            _ => Self::LeaveUnchanged,
        }
    }
}

impl From<CPUBoost> for i32 {
    fn from(value: CPUBoost) -> Self {
        value as i32
    }
}

/// Probe boost independently of CPU governor and EPP support.
pub fn has_cpu_boost() -> bool {
    BoostControl::detect(Path::new(CPU_SYSFS)).is_some()
}

pub fn set_cpu_boost(enabled: bool) -> Result<()> {
    BoostControl::detect(Path::new(CPU_SYSFS))
        .ok_or(PlatformError::NotSupported)?
        .set(enabled)
}

enum BoostControl {
    CpuFreq {
        global: Option<PathBuf>,
        policies: Vec<PathBuf>,
    },
    IntelNoTurbo(PathBuf),
}

impl BoostControl {
    fn detect(root: &Path) -> Option<Self> {
        let cpufreq = root.join("cpufreq");
        let global = cpufreq.join("boost");
        let global = global.is_file().then_some(global);
        let no_turbo = root.join("intel_pstate/no_turbo");
        if global.is_none() && no_turbo.is_file() {
            return Some(Self::IntelNoTurbo(no_turbo));
        }

        // cpuN/cpufreq is a symlink to policyN. Visit each policy only once.
        let policies: Vec<_> = fs::read_dir(cpufreq)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let name = entry.file_name();
                let index = name.to_str()?.strip_prefix("policy")?;
                index.parse::<u32>().ok()?;
                let path = entry.path().join("boost");
                path.is_file().then_some(path)
            })
            .collect();
        if global.is_none() && policies.is_empty() {
            return None;
        }
        Some(Self::CpuFreq { global, policies })
    }

    fn set(&self, enabled: bool) -> Result<()> {
        match self {
            Self::IntelNoTurbo(path) => Self::write(path, !enabled),
            Self::CpuFreq { global, policies } => {
                if let Some(path) = global {
                    Self::write(path, enabled)?;
                    // With global boost disabled, Linux disallows per-policy writes.
                    if !enabled {
                        return Ok(());
                    }
                }

                // Some kernels skip updating individual policies when the global
                // setting already matches. Apply any remaining overrides directly.
                let mut first_error = None;
                for path in policies {
                    match Self::set_policy(path, enabled) {
                        Ok(()) => {}
                        Err(PlatformError::Read(_, ref err) | PlatformError::Write(_, ref err))
                            if matches!(
                                err.kind(),
                                ErrorKind::NotFound | ErrorKind::ResourceBusy
                            ) =>
                        {
                            // A CPU policy can disappear or go offline after
                            // discovery.
                        }
                        Err(err) => {
                            first_error.get_or_insert(err);
                        }
                    }
                }
                first_error.map_or(Ok(()), Err)
            }
        }
    }

    fn set_policy(path: &Path, enabled: bool) -> Result<()> {
        let value = fs::read_to_string(path)
            .map_err(|err| PlatformError::Read(path.display().to_string(), err))?;
        if value.trim() == if enabled { "1" } else { "0" } {
            return Ok(());
        }
        Self::write(path, enabled)
    }

    fn write(path: &Path, value: bool) -> Result<()> {
        OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(path)
            .and_then(|mut file| file.write_all(if value { b"1" } else { b"0" }))
            .map_err(|err| PlatformError::Write(path.display().to_string(), err))
    }
}

#[cfg(test)]
mod boost_tests {
    use super::*;

    struct FakeSysfs(PathBuf);

    impl FakeSysfs {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("asusctl-boost-{}-{name}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn file(&self, name: &str, value: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, value).unwrap();
            path
        }
    }

    impl Drop for FakeSysfs {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn uses_global_cpufreq_and_enables_policy_overrides() {
        let root = FakeSysfs::new("global");
        let global = root.file("cpufreq/boost", "1\n");
        let policy = root.file("cpufreq/policy0/boost", "0\n");
        let control = BoostControl::detect(&root.0).unwrap();
        control.set(true).unwrap();
        assert_eq!(fs::read_to_string(&policy).unwrap(), "1");
        control.set(false).unwrap();
        assert_eq!(fs::read_to_string(global).unwrap(), "0");
        // The global kernel control, not a separate write, disables the policies.
        assert_eq!(fs::read_to_string(policy).unwrap(), "1");
    }

    #[test]
    fn applies_policy_only_boost_to_all_policies() {
        let root = FakeSysfs::new("policies");
        let first = root.file("cpufreq/policy0/boost", "1\n");
        let second = root.file("cpufreq/policy4/boost", "1\n");
        let ignored = root.file("cpufreq/policy-metadata/boost", "1\n");
        BoostControl::detect(&root.0).unwrap().set(false).unwrap();
        assert_eq!(fs::read_to_string(first).unwrap(), "0");
        assert_eq!(fs::read_to_string(second).unwrap(), "0");
        assert_eq!(fs::read_to_string(ignored).unwrap(), "1\n");
    }

    #[test]
    fn inverts_intel_no_turbo() {
        let root = FakeSysfs::new("intel");
        let path = root.file("intel_pstate/no_turbo", "0\n");
        let control = BoostControl::detect(&root.0).unwrap();
        control.set(false).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "1");
        control.set(true).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "0");
    }

    #[test]
    fn handles_missing_boost_and_reports_write_failure() {
        let root = FakeSysfs::new("missing");
        root.file("cpufreq/policy0/energy_performance_preference", "power\n");
        assert!(BoostControl::detect(&root.0).is_none());

        let path = root.file("cpufreq/boost", "1\n");
        let control = BoostControl::detect(&root.0).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(matches!(
            control.set(false),
            Err(PlatformError::Write(_, _))
        ));
        assert!(
            !path.exists(),
            "failed write must not create the missing file"
        );
    }
}

/// Both modern AMD and Intel have cpufreq control if using `powersave`
/// governor. What interests us the most here is `energy_performance_preference`
/// which can drastically alter CPU performance.
#[derive(Debug, PartialEq, Eq, PartialOrd, Clone)]
pub struct CPUControl {
    paths: Vec<PathBuf>,
}

impl CPUControl {
    pub fn new() -> Result<Self> {
        let mut enumerator = udev::Enumerator::new().map_err(|err| {
            warn!("{}", err);
            PlatformError::Udev("enumerator failed".into(), err)
        })?;
        enumerator.match_subsystem("cpu").map_err(|err| {
            warn!("{}", err);
            PlatformError::Udev("match_subsystem failed".into(), err)
        })?;

        let mut supported = false;
        let mut cpu = CPUControl { paths: Vec::new() };
        for device in enumerator.scan_devices().map_err(|err| {
            warn!("{}", err);
            PlatformError::Udev("CPU: scan_devices failed".into(), err)
        })? {
            if !supported {
                info!(
                    "Found CPU support at {:?}, checking supported items",
                    device.sysname()
                );

                match device.attribute_value(ATTR_AVAILABLE_GOVERNORS) {
                    Some(g) => info!("{ATTR_AVAILABLE_GOVERNORS}: {g:?}"),
                    None => {
                        return Err(PlatformError::CPU(format!(
                            "{ATTR_AVAILABLE_GOVERNORS} not found"
                        )));
                    }
                }
                match device.attribute_value(ATTR_GOVERNOR) {
                    Some(g) => info!("{ATTR_GOVERNOR}: {g:?}"),
                    None => return Err(PlatformError::CPU(format!("{ATTR_GOVERNOR} not found"))),
                }
                // EPP attributes (energy_performance_available_preferences and energy_performance_preference)
                // are optional depending on the cpufreq driver and kernel version. Missing EPP attributes
                // are logged as warnings rather than hard errors to support broader hardware configurations.
                match device.attribute_value(ATTR_AVAILABLE_EPP) {
                    Some(g) => info!("{ATTR_AVAILABLE_EPP}: {g:?}"),
                    None => warn!("{ATTR_AVAILABLE_EPP} not found (EPP controls unsupported)"),
                }
                match device.attribute_value(ATTR_EPP) {
                    Some(g) => info!("{ATTR_EPP}: {g:?}"),
                    None => warn!("{ATTR_EPP} not found (EPP preference setting unsupported)"),
                }
                supported = true;
            }
            if supported {
                info!("Adding: {:?}", device.syspath());
                cpu.paths.push(device.syspath().to_owned());
            }
        }
        if cpu.paths.is_empty() {
            return Err(PlatformError::MissingFunction(
                "asus-nb-wmi not found".into(),
            ));
        }
        Ok(cpu)
    }

    pub fn get_governor(&self) -> Result<CPUGovernor> {
        if let Some(path) = self.paths.first() {
            let s = read_attr_string(&to_device(path)?, ATTR_GOVERNOR)?;
            Ok(s.as_str().into())
        } else {
            Err(PlatformError::CPU("No CPUs found".to_string()))
        }
    }

    pub fn get_available_governors(&self) -> Result<Vec<CPUGovernor>> {
        if let Some(path) = self.paths.first() {
            read_attr_string(&to_device(path)?, ATTR_AVAILABLE_GOVERNORS)
                .map(|s| s.split_whitespace().map(|s| s.into()).collect())
        } else {
            Err(PlatformError::CPU("No CPUs found".to_string()))
        }
    }

    pub fn set_governor(&self, gov: CPUGovernor) -> Result<()> {
        if !self.get_available_governors()?.contains(&gov) {
            return Err(PlatformError::CPU(format!("{gov:?} is not available")));
        }
        for path in &self.paths {
            let mut dev = to_device(path)?;
            dev.set_attribute_value(ATTR_GOVERNOR, String::from(gov))?;
        }
        Ok(())
    }

    pub fn get_epp(&self) -> Result<CPUEPP> {
        if let Some(path) = self.paths.first() {
            let s = read_attr_string(&to_device(path)?, ATTR_EPP)?;
            Ok(s.as_str().into())
        } else {
            Err(PlatformError::CPU("No CPUs found".to_string()))
        }
    }

    /// Returns available EPP (Energy Performance Preference) options.
    /// EPP sysfs attributes are optional as not all CPU scaling drivers expose them.
    pub fn get_available_epp(&self) -> Result<Vec<CPUEPP>> {
        if let Some(path) = self.paths.first() {
            match read_attr_string(&to_device(path)?, ATTR_AVAILABLE_EPP) {
                Ok(s) => Ok(s.split_whitespace().map(|s| s.into()).collect()),
                Err(err) => {
                    debug!("Reading {ATTR_AVAILABLE_EPP} failed or attribute not present: {err}");
                    Err(err)
                }
            }
        } else {
            Err(PlatformError::CPU("No CPUs found".to_string()))
        }
    }

    /// Sets EPP (Energy Performance Preference) for all CPUs.
    /// Checks available preferences first; if unsupported or invalid, returns an error early.
    pub fn set_epp(&self, epp: CPUEPP) -> Result<()> {
        let available = match self.get_available_epp() {
            Ok(avail) => avail,
            Err(err) => {
                debug!("EPP preference setting unsupported on this system: {err}");
                return Err(err);
            }
        };
        if !available.contains(&epp) {
            return Err(PlatformError::CPU(format!("{epp:?} is not available")));
        }
        for path in &self.paths {
            let mut dev = to_device(path)?;
            dev.set_attribute_value(ATTR_EPP, String::from(epp))?;
        }
        Ok(())
    }
}

#[repr(u8)]
#[derive(
    Deserialize, Serialize, Type, Value, OwnedValue, Debug, PartialEq, PartialOrd, Clone, Copy,
)]
#[zvariant(signature = "s")]
pub enum CPUGovernor {
    Performance = 0,
    Powersave = 1,
    BadValue = 2,
}

impl From<&str> for CPUGovernor {
    fn from(s: &str) -> Self {
        match s {
            "performance" => Self::Performance,
            "powersave" => Self::Powersave,
            _ => Self::BadValue,
        }
    }
}

impl From<CPUGovernor> for String {
    fn from(g: CPUGovernor) -> Self {
        match g {
            CPUGovernor::Performance => "performance".to_string(),
            CPUGovernor::Powersave => "powersave".to_string(),
            CPUGovernor::BadValue => "bad_value".to_string(),
        }
    }
}

#[repr(u32)]
#[derive(
    Deserialize,
    Serialize,
    Type,
    Value,
    OwnedValue,
    Default,
    Debug,
    PartialEq,
    PartialOrd,
    Clone,
    Copy,
)]
#[zvariant(signature = "u")]
pub enum CPUEPP {
    #[default]
    Default = 0,
    Performance = 1,
    BalancePerformance = 2,
    BalancePower = 3,
    Power = 4,
}

impl From<PlatformProfile> for CPUEPP {
    fn from(value: PlatformProfile) -> Self {
        match value {
            PlatformProfile::Balanced => CPUEPP::BalancePerformance,
            PlatformProfile::Performance => CPUEPP::Performance,
            PlatformProfile::Quiet => CPUEPP::Power,
            PlatformProfile::LowPower => CPUEPP::Power,
            PlatformProfile::Custom => CPUEPP::BalancePower,
        }
    }
}

impl From<&str> for CPUEPP {
    fn from(s: &str) -> Self {
        match s {
            "default" => Self::Default,
            "performance" => Self::Performance,
            "balance_performance" => Self::BalancePerformance,
            "balance_power" => Self::BalancePower,
            "power" => Self::Power,
            _ => Self::Default,
        }
    }
}

impl From<CPUEPP> for String {
    fn from(g: CPUEPP) -> Self {
        match g {
            CPUEPP::Default => "default".to_string(),
            CPUEPP::Performance => "performance".to_string(),
            CPUEPP::BalancePerformance => "balance_performance".to_string(),
            CPUEPP::BalancePower => "balance_power".to_string(),
            CPUEPP::Power => "power".to_string(),
        }
    }
}

impl From<i32> for CPUEPP {
    fn from(value: i32) -> Self {
        match value {
            0 => Self::Default,
            1 => Self::Performance,
            2 => Self::BalancePerformance,
            3 => Self::BalancePower,
            4 => Self::Power,
            _ => Self::Default,
        }
    }
}

impl From<CPUEPP> for i32 {
    fn from(value: CPUEPP) -> Self {
        value as i32
    }
}

pub fn get_cpu_model() -> String {
    if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
        for line in cpuinfo.lines() {
            if line.starts_with("model name")
                && let Some(pos) = line.find(':')
            {
                let model = line[pos + 1..].trim().to_string();
                if !model.is_empty() {
                    return model;
                }
            }
        }
    }
    "CPU".to_string()
}

#[derive(Debug, Clone, Copy)]
pub struct CpuTicks {
    pub idle: u64,
    pub total: u64,
}

pub fn read_cpu_ticks() -> Option<CpuTicks> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let first_line = stat.lines().next()?;
    if first_line.starts_with("cpu ") {
        let parts: Vec<&str> = first_line.split_whitespace().collect();
        let mut total = 0u64;
        let mut idle = 0u64;
        for (i, part) in parts.iter().skip(1).enumerate() {
            if let Ok(ticks) = part.parse::<u64>() {
                total += ticks;
                if i == 3 || i == 4 {
                    idle += ticks;
                }
            }
        }
        return Some(CpuTicks { idle, total });
    }
    None
}

pub fn get_cpu_temp() -> f32 {
    if let Ok(entries) = std::fs::read_dir("/sys/class/hwmon") {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(name) = std::fs::read_to_string(path.join("name")) {
                let name = name.trim();
                if (name == "k10temp" || name == "coretemp" || name == "zenpower")
                    && let Ok(temp_str) = std::fs::read_to_string(path.join("temp1_input"))
                    && let Ok(temp_val) = temp_str.trim().parse::<f32>()
                {
                    return temp_val / 1000.0;
                }
            }
        }
    }
    if let Ok(temp_str) = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp")
        && let Ok(temp_val) = temp_str.trim().parse::<f32>()
    {
        return temp_val / 1000.0;
    }
    0.0
}

pub fn get_cpu_frequency_mhz() -> f32 {
    let mut total_freq = 0.0;
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir("/sys/devices/system/cpu") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("cpu") && name[3..].chars().all(|c| c.is_ascii_digit()) {
                let freq_path = entry.path().join("cpufreq/scaling_cur_freq");
                if let Ok(freq_str) = std::fs::read_to_string(freq_path)
                    && let Ok(freq_khz) = freq_str.trim().parse::<f32>()
                {
                    total_freq += freq_khz / 1000.0;
                    count += 1;
                }
            }
        }
    }
    if count == 0
        && let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo")
    {
        for line in cpuinfo.lines() {
            if line.starts_with("cpu MHz")
                && let Some(pos) = line.find(':')
                && let Ok(val) = line[pos + 1..].trim().parse::<f32>()
            {
                total_freq += val;
                count += 1;
            }
        }
    }
    if count > 0 {
        total_freq / count as f32
    } else {
        0.0
    }
}

pub fn get_ram_usage_pct() -> f32 {
    if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
        let mut total = 0.0;
        let mut available = 0.0;
        for line in meminfo.lines() {
            if line.starts_with("MemTotal:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Some(val) = parts.get(1) {
                    total = val.parse::<f32>().unwrap_or(0.0);
                }
            } else if line.starts_with("MemAvailable:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Some(val) = parts.get(1) {
                    available = val.parse::<f32>().unwrap_or(0.0);
                }
            }
        }
        if total > 0.0 {
            return ((total - available) / total) * 100.0;
        }
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::CPUControl;
    use crate::cpu::{CPUEPP, CPUGovernor};

    #[test]
    #[ignore = "Can't run this in a docker image"]
    fn check_cpu() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let cpu = CPUControl::new()?;
        assert_eq!(cpu.get_governor()?, CPUGovernor::Powersave);
        assert_eq!(
            cpu.get_available_governors()?,
            vec![
                CPUGovernor::Performance,
                CPUGovernor::Powersave
            ]
        );

        assert_eq!(cpu.get_epp()?, CPUEPP::BalancePower);
        assert_eq!(
            cpu.get_available_epp()?,
            vec![
                CPUEPP::Default,
                CPUEPP::Performance,
                CPUEPP::BalancePerformance,
                CPUEPP::BalancePower,
                CPUEPP::Power,
            ]
        );
        Ok(())
    }
}
