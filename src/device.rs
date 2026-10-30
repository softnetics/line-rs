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
            Device::DesktopWin | Device::DesktopMac | Device::Ios | Device::Android | Device::AndroidSecondary
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
        }
    }
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
            _ => return Err(Error::internal("UnsupportedDevice", format!("Unsupported device: {s}."))),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDetails {
    pub device: Device,
    pub app_version: String,
    pub system_name: String,
    pub system_version: String,
}

impl DeviceDetails {
    /// `x-line-application` header value.
    pub fn application(&self) -> String {
        format!("{}\t{}\t{}\t{}", self.device, self.app_version, self.system_name, self.system_version)
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
        assert_eq!(d.application(), "DESKTOPWIN\t9.7.0.3556\tWINDOWS\t10.0.0-NT-x64");
        assert_eq!("ios".parse::<Device>().unwrap(), Device::Ios);
        assert_eq!(Device::DesktopMac.details(None).application(), "DESKTOPMAC\t26.5.0\tMAC\t13.0.0");
    }
}
