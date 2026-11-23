use std::collections::HashMap;

use config_traits::{StdConfig, StdConfigLoad2};
use rog_platform::asus_armoury::FirmwareAttribute;
use rog_platform::cpu::{CPUBoost, CPUEPP};
use rog_platform::platform::PlatformProfile;
use serde::{Deserialize, Serialize};

const CONFIG_FILE: &str = "asusd.ron";

/// Default value for base_charge_control_end_threshold when not present in config.
/// Returns 0 so restore_charge_limit() skips restoration for upgraded configs.
fn default_base_charge_limit() -> u8 {
    0
}

#[derive(Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct Tuning {
    pub enabled: bool,
    pub group: HashMap<FirmwareAttribute, i32>,
}
type Tunings = HashMap<PlatformProfile, Tuning>;

#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ProfileCpuBoost {
    balanced: CPUBoost,
    performance: CPUBoost,
    quiet: CPUBoost,
    custom: CPUBoost,
}

impl ProfileCpuBoost {
    pub fn get(self, profile: PlatformProfile) -> CPUBoost {
        match profile {
            PlatformProfile::Balanced => self.balanced,
            PlatformProfile::Performance => self.performance,
            PlatformProfile::Quiet | PlatformProfile::LowPower => self.quiet,
            PlatformProfile::Custom => self.custom,
        }
    }

    pub fn set(&mut self, profile: PlatformProfile, boost: CPUBoost) {
        match profile {
            PlatformProfile::Balanced => self.balanced = boost,
            PlatformProfile::Performance => self.performance = boost,
            PlatformProfile::Quiet | PlatformProfile::LowPower => self.quiet = boost,
            PlatformProfile::Custom => self.custom = boost,
        }
    }

    /// Only return a new policy if the setting for the active profile was
    /// edited.
    pub fn changed_policy_for(self, previous: Self, active: PlatformProfile) -> Option<CPUBoost> {
        let policy = self.get(active);
        (policy != previous.get(active)).then_some(policy)
    }
}

#[derive(Deserialize, Serialize, PartialEq)]
pub struct Config {
    // The current charge limit applied
    pub charge_control_end_threshold: u8,
    /// Save charge limit for restoring after one-shot full charge
    #[serde(default = "default_base_charge_limit")]
    pub base_charge_control_end_threshold: u8,
    pub disable_nvidia_powerd_on_battery: bool,
    /// An optional command/script to run when power is changed to AC
    pub ac_command: String,
    /// An optional command/script to run when power is changed to battery
    pub bat_command: String,
    /// Set true if energy_performance_preference should be set if the
    /// platform profile is changed
    pub platform_profile_linked_epp: bool,
    /// Which platform profile to use on battery power
    pub platform_profile_on_battery: PlatformProfile,
    /// Should the throttle policy be set on bat/ac change?
    pub change_platform_profile_on_battery: bool,
    /// Which platform profile to use on AC power
    pub platform_profile_on_ac: PlatformProfile,
    /// Should the platform profile be set on bat/ac change?
    pub change_platform_profile_on_ac: bool,
    /// The energy_performance_preference for this platform profile
    pub profile_quiet_epp: CPUEPP,
    /// The energy_performance_preference for this platform profile
    pub profile_balanced_epp: CPUEPP,
    /// The energy_performance_preference for this platform profile
    pub profile_custom_epp: CPUEPP,
    /// The energy_performance_preference for this platform profile
    pub profile_performance_epp: CPUEPP,
    /// CPU boost behavior for each platform profile, independent of EPP.
    #[serde(default)]
    pub profile_cpu_boost: ProfileCpuBoost,
    pub ac_profile_tunings: Tunings,
    pub dc_profile_tunings: Tunings,
    pub armoury_settings: HashMap<FirmwareAttribute, i32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub screenpad_gamma: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub screenpad_sync_primary: Option<bool>,
    /// Persisted XG Mobile LED state: None = never set, Some(false) = OFF,
    /// Some(true) = ON. Re-applied when the device appears.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub xgm_led_enabled: Option<bool>,
    /// Temporary state for AC/Batt
    #[serde(skip)]
    pub last_power_plugged: u8,
}

impl Config {
    pub fn profile_epp(&self, profile: PlatformProfile) -> CPUEPP {
        match profile {
            PlatformProfile::Balanced => self.profile_balanced_epp,
            PlatformProfile::Performance => self.profile_performance_epp,
            PlatformProfile::Quiet | PlatformProfile::LowPower => self.profile_quiet_epp,
            PlatformProfile::Custom => self.profile_custom_epp,
        }
    }

    pub fn select_tunings(&mut self, power_plugged: bool, profile: PlatformProfile) -> &mut Tuning {
        let config = if power_plugged {
            &mut self.ac_profile_tunings
        } else {
            &mut self.dc_profile_tunings
        };
        config.entry(profile).or_insert_with(Tuning::default)
    }

    pub fn select_tunings_ref(
        &self,
        power_plugged: bool,
        profile: PlatformProfile,
    ) -> Option<&Tuning> {
        let config = if power_plugged {
            &self.ac_profile_tunings
        } else {
            &self.dc_profile_tunings
        };
        config.get(&profile)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            charge_control_end_threshold: 100,
            // NOTE: This is intentionally 100 (not 0 like the serde default).
            // New installs get 100 (no limit). Upgraded configs missing this
            // field get 0 via serde, which skips restore_charge_limit().
            base_charge_control_end_threshold: 100,
            disable_nvidia_powerd_on_battery: true,
            ac_command: Default::default(),
            bat_command: Default::default(),
            platform_profile_linked_epp: true,
            platform_profile_on_battery: PlatformProfile::Quiet,
            change_platform_profile_on_battery: true,
            platform_profile_on_ac: PlatformProfile::Performance,
            change_platform_profile_on_ac: true,
            profile_quiet_epp: CPUEPP::Power,
            profile_balanced_epp: CPUEPP::BalancePower,
            profile_performance_epp: CPUEPP::Performance,
            profile_custom_epp: CPUEPP::Performance,
            profile_cpu_boost: Default::default(),
            ac_profile_tunings: HashMap::default(),
            dc_profile_tunings: HashMap::default(),
            armoury_settings: HashMap::default(),
            last_power_plugged: Default::default(),
            screenpad_gamma: Default::default(),
            screenpad_sync_primary: Default::default(),
            xgm_led_enabled: Default::default(),
        }
    }
}

impl StdConfig for Config {
    fn new() -> Self {
        Config {
            charge_control_end_threshold: 100,
            disable_nvidia_powerd_on_battery: true,
            platform_profile_on_battery: PlatformProfile::Quiet,
            platform_profile_on_ac: PlatformProfile::Performance,
            ac_command: String::new(),
            bat_command: String::new(),
            ..Default::default()
        }
    }

    fn file_name(&self) -> String {
        CONFIG_FILE.to_owned()
    }

    fn config_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(crate::CONFIG_PATH_BASE)
    }
}

impl StdConfigLoad2<Config611, Config601> for Config {}

#[derive(Deserialize, Serialize)]
pub struct Config611 {
    pub charge_control_end_threshold: u8,
    #[serde(skip)]
    pub base_charge_control_end_threshold: u8,
    pub disable_nvidia_powerd_on_battery: bool,
    pub ac_command: String,
    pub bat_command: String,
    pub platform_profile_linked_epp: bool,
    pub platform_profile_on_battery: PlatformProfile,
    pub change_platform_profile_on_battery: bool,
    pub platform_profile_on_ac: PlatformProfile,
    pub change_platform_profile_on_ac: bool,
    pub profile_quiet_epp: CPUEPP,
    pub profile_balanced_epp: CPUEPP,
    pub profile_custom_epp: CPUEPP,
    pub profile_performance_epp: CPUEPP,
    pub ac_profile_tunings: Tunings,
    pub dc_profile_tunings: Tunings,
    pub armoury_settings: HashMap<FirmwareAttribute, i32>,
    #[serde(skip)]
    pub last_power_plugged: u8,
}

impl From<Config611> for Config {
    fn from(c: Config611) -> Self {
        let mut config = Self {
            // Restore the base charge limit
            charge_control_end_threshold: c.charge_control_end_threshold,
            base_charge_control_end_threshold: c.charge_control_end_threshold,
            disable_nvidia_powerd_on_battery: c.disable_nvidia_powerd_on_battery,
            ac_command: c.ac_command,
            bat_command: c.bat_command,
            platform_profile_linked_epp: c.platform_profile_linked_epp,
            platform_profile_on_battery: c.platform_profile_on_battery,
            change_platform_profile_on_battery: c.change_platform_profile_on_battery,
            platform_profile_on_ac: c.platform_profile_on_ac,
            change_platform_profile_on_ac: c.change_platform_profile_on_ac,
            profile_quiet_epp: c.profile_quiet_epp,
            profile_balanced_epp: c.profile_balanced_epp,
            profile_performance_epp: c.profile_performance_epp,
            profile_custom_epp: c.profile_performance_epp,
            profile_cpu_boost: Default::default(),
            last_power_plugged: c.last_power_plugged,
            ac_profile_tunings: HashMap::default(),
            dc_profile_tunings: HashMap::default(),
            armoury_settings: HashMap::default(),
            screenpad_gamma: None,
            screenpad_sync_primary: Default::default(),
            xgm_led_enabled: Default::default(),
        };

        config.ac_profile_tunings = c.ac_profile_tunings;
        config.dc_profile_tunings = c.dc_profile_tunings;
        config.armoury_settings = c.armoury_settings;
        config
    }
}

#[derive(Deserialize, Serialize)]
pub struct Config601 {
    pub charge_control_end_threshold: u8,
    #[serde(skip)]
    pub base_charge_control_end_threshold: u8,
    pub panel_od: bool,
    pub boot_sound: bool,
    pub mini_led_mode: bool,
    pub disable_nvidia_powerd_on_battery: bool,
    pub ac_command: String,
    pub bat_command: String,
    pub platform_profile_linked_epp: bool,
    pub platform_profile_on_battery: PlatformProfile,
    pub change_platform_profile_on_battery: bool,
    pub platform_profile_on_ac: PlatformProfile,
    pub change_platform_profile_on_ac: bool,
    pub profile_quiet_epp: CPUEPP,
    pub profile_balanced_epp: CPUEPP,
    pub profile_performance_epp: CPUEPP,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ppt_pl1_spl: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ppt_pl2_sppt: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ppt_pl3_fppt: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ppt_fppt: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ppt_apu_sppt: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ppt_platform_sppt: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub nv_dynamic_boost: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub nv_temp_target: Option<u8>,
    #[serde(skip)]
    pub last_power_plugged: u8,
}

impl From<Config601> for Config {
    fn from(c: Config601) -> Self {
        Self {
            // Restore the base charge limit
            charge_control_end_threshold: c.charge_control_end_threshold,
            base_charge_control_end_threshold: c.charge_control_end_threshold,
            disable_nvidia_powerd_on_battery: c.disable_nvidia_powerd_on_battery,
            ac_command: c.ac_command,
            bat_command: c.bat_command,
            platform_profile_linked_epp: c.platform_profile_linked_epp,
            platform_profile_on_battery: c.platform_profile_on_battery,
            change_platform_profile_on_battery: c.change_platform_profile_on_battery,
            platform_profile_on_ac: c.platform_profile_on_ac,
            change_platform_profile_on_ac: c.change_platform_profile_on_ac,
            profile_quiet_epp: c.profile_quiet_epp,
            profile_balanced_epp: c.profile_balanced_epp,
            profile_performance_epp: c.profile_performance_epp,
            profile_custom_epp: c.profile_performance_epp,
            profile_cpu_boost: Default::default(),
            last_power_plugged: c.last_power_plugged,
            ac_profile_tunings: HashMap::default(),
            dc_profile_tunings: HashMap::default(),
            armoury_settings: HashMap::default(),
            screenpad_gamma: None,
            screenpad_sync_primary: Default::default(),
            xgm_led_enabled: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_without_boost_leaves_it_unchanged() {
        let config = Config::default();
        let ron = ron::to_string(&config).unwrap();
        let boost = ron::to_string(&config.profile_cpu_boost).unwrap();
        let field = format!("profile_cpu_boost:{boost},");
        assert!(ron.contains(&field));
        let restored: Config = ron::from_str(&ron.replace(&field, "")).unwrap();
        assert_eq!(restored.profile_cpu_boost, ProfileCpuBoost::default());
        assert_eq!(restored.profile_epp(PlatformProfile::Quiet), CPUEPP::Power);
        assert_eq!(
            restored
                .profile_cpu_boost
                .get(PlatformProfile::Quiet)
                .enabled(),
            None
        );

        let legacy: Config611 = ron::from_str(&ron).unwrap();
        assert_eq!(
            Config::from(legacy).profile_cpu_boost,
            ProfileCpuBoost::default()
        );
    }

    #[test]
    fn only_an_edit_for_the_active_profile_applies_boost() {
        let mut config = Config::default();
        let before = config.profile_cpu_boost;
        config
            .profile_cpu_boost
            .set(PlatformProfile::Quiet, CPUBoost::Disabled);
        config
            .profile_cpu_boost
            .set(PlatformProfile::Custom, CPUBoost::Enabled);
        let saved = ron::to_string(&config).unwrap();
        let restored: Config = ron::from_str(&saved).unwrap();
        let boost = restored.profile_cpu_boost;
        assert_eq!(
            boost.changed_policy_for(before, PlatformProfile::LowPower),
            Some(CPUBoost::Disabled)
        );
        assert_eq!(
            boost.changed_policy_for(before, PlatformProfile::Custom),
            Some(CPUBoost::Enabled)
        );
        assert_eq!(
            boost.changed_policy_for(before, PlatformProfile::Balanced),
            None
        );
    }
}
