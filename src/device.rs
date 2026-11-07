//! Emulated LINE client devices.

use std::fmt;
use std::str::FromStr;

use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Device {
    DesktopWin,
    DesktopMac,
    Android,
    AndroidSecondary,
    Ios,
    IosIpad,
    WatchOs,
    WearOs,
}

impl Device {
    /// The name LINE expects (e.g. `DESKTOPWIN`).
    pub fn as_str(self) -> &'static str {
        match self {
            Device::DesktopWin => "DESKTOPWIN",
            Device::DesktopMac => "DESKTOPMAC",
            Device::Android => "ANDROID",
            Device::AndroidSecondary => "ANDROIDSECONDARY",
            Device::Ios => "IOS",
            Device::IosIpad => "IOSIPAD",
            Device::WatchOs => "WATCHOS",
            Device::WearOs => "WEAROS",
        }
    }

    /// Devices that support the v3 (refresh-token) login flows.
    pub fn is_v3_supported(self) -> bool {
        matches!(
            self,
            Device::DesktopWin
                | Device::DesktopMac
                | Device::Ios
                | Device::Android
                | Device::AndroidSecondary
        )
    }

    /// Default app version, OS name and OS version for this device.
    pub fn details(self, version: Option<&str>) -> DeviceDetails {
        let (app, sys, sys_ver) = match self {
            Device::DesktopWin => ("9.7.0.3556", "WINDOWS", "10.0.0-NT-x64"),
            // App version derived from static RE of LINE macOS 26.5.0
            // (Sentry release `line-desktop-mac@26.5.0.3975`); unverified
            // against live server.
            Device::DesktopMac => ("26.5.0", "MAC", "13.0.0"),
            Device::Android | Device::AndroidSecondary => ("26.6.2", "Android OS", "16"),
            Device::Ios | Device::IosIpad => ("26.7.2", "iOS", "18.0"),
            Device::WatchOs => ("26.7.2", "Watch OS", "11.0"),
            Device::WearOs => ("13.4.1", "Wear OS", "3.0"),
        };
        DeviceDetails {
            device: self,
            app_version: version.unwrap_or(app).to_string(),
            system_name: sys.to_string(),
            system_version: sys_ver.to_string(),
            device_name: default_device_name().to_string(),
        }
    }
}

/// The name this computer gives LINE at login when the caller sets none
/// (see [`DeviceDetails::device_name`]).
///
/// The hardware model when the OS reports one (`hw.model` on macOS, e.g.
/// `Mac14,2`; the DMI product name on Linux), otherwise `Mac`,
/// `Windows PC`, `Linux PC` or `Computer`. Read once and cached.
pub fn default_device_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        host_model()
            .filter(|m| is_real_model(m))
            .unwrap_or_else(|| fallback_device_name().to_string())
    })
}

fn fallback_device_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "Mac"
    } else if cfg!(target_os = "windows") {
        "Windows PC"
    } else if cfg!(target_os = "linux") {
        "Linux PC"
    } else {
        "Computer"
    }
}

/// Firmware placeholders that are not a model name (`"System Product
/// Name"` is the ASUS default that linejs sent for every device).
fn is_real_model(model: &str) -> bool {
    const PLACEHOLDERS: [&str; 8] = [
        "system product name",
        "to be filled by o.e.m.",
        "default string",
        "not applicable",
        "not specified",
        "none",
        "unknown",
        "o.e.m.",
    ];
    let m = model.trim();
    !m.is_empty() && m.len() <= 64 && !PLACEHOLDERS.contains(&m.to_ascii_lowercase().as_str())
}

#[cfg(target_os = "macos")]
fn host_model() -> Option<String> {
    let name = c"hw.model";
    let mut buf = [0u8; 128];
    let mut len = buf.len();
    // SAFETY: `name` is NUL-terminated; `buf` and `len` describe a valid
    // writable buffer, and sysctl writes at most `len` bytes.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    let bytes = &buf[..len.min(buf.len())];
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    Some(String::from_utf8_lossy(&bytes[..end]).trim().to_string())
}

#[cfg(target_os = "linux")]
fn host_model() -> Option<String> {
    std::fs::read_to_string("/sys/devices/virtual/dmi/id/product_name")
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn host_model() -> Option<String> {
    None
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Device {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        Ok(match s.to_ascii_uppercase().as_str() {
            "DESKTOPWIN" => Device::DesktopWin,
            "DESKTOPMAC" => Device::DesktopMac,
            "ANDROID" => Device::Android,
            "ANDROIDSECONDARY" => Device::AndroidSecondary,
            "IOS" => Device::Ios,
            "IOSIPAD" => Device::IosIpad,
            "WATCHOS" => Device::WatchOs,
            "WEAROS" => Device::WearOs,
            _ => {
                return Err(Error::internal(
                    "UnsupportedDevice",
                    format!("Unsupported device: {s}."),
                ));
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDetails {
    pub device: Device,
    pub app_version: String,
    pub system_name: String,
    pub system_version: String,
    /// The device's name sent at login as `modelName` and `systemName`
    /// (the same value in both). LINE shows it in the
    /// phone's list of logged-in devices. Defaults to
    /// [`default_device_name`]; set it with `InitOptions::device_name` or
    /// `ClientBuilder::device_name`.
    pub device_name: String,
}

impl DeviceDetails {
    /// `x-line-application` header value.
    pub fn application(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}",
            self.device, self.app_version, self.system_name, self.system_version
        )
    }

    pub fn user_agent(&self) -> String {
        format!("Line/{}", self.app_version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_header() {
        let d = Device::DesktopWin.details(None);
        assert_eq!(
            d.application(),
            "DESKTOPWIN\t9.7.0.3556\tWINDOWS\t10.0.0-NT-x64"
        );
        assert_eq!("ios".parse::<Device>().unwrap(), Device::Ios);
        assert_eq!(
            Device::DesktopMac.details(None).application(),
            "DESKTOPMAC\t26.5.0\tMAC\t13.0.0"
        );
    }

    #[test]
    fn device_name_is_real_or_neutral() {
        let name = default_device_name();
        assert!(is_real_model(name), "{name}");
        for fake in ["linejs", "evex", "System Product Name"] {
            assert!(!name.contains(fake), "{name}");
        }
        assert_eq!(Device::Android.details(None).device_name, name);
        assert!(!is_real_model("To be filled by O.E.M."));
        assert!(!is_real_model("  "));
        assert!(is_real_model("MacBookPro18,3"));
    }
}
