//! Camera diagnosis below libgphoto2: is a camera on USB at all, which driver owns it,
//! and is it behind a hub. Windows only for now; that's where the driver goes wrong
//! (Zadig's WinUSB swap belongs to one USB port, and a new port gets Windows' own driver).

use serde::Deserialize;

/// Camera makers' USB vendor IDs.
const VENDORS: [(&str, &str); 9] = [
    ("04A9", "Canon"),
    ("04B0", "Nikon"),
    ("054C", "Sony"),
    ("04DA", "Panasonic"),
    ("04CB", "Fujifilm"),
    ("07B4", "Olympus"),
    ("0A17", "Pentax"),
    ("25FB", "Ricoh"),
    ("1A98", "Leica"),
];

/// Device classes a camera shows up as. Leaves out printers and scanners from the same makers.
const CAMERA_CLASSES: [&str; 3] = ["WPD", "Camera", "USBDevice"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbCamera {
    pub name: String,
    pub vendor: &'static str,
    pub driver: Driver,
    /// The hub it's plugged into, or `None` when it's on a port of the computer itself.
    pub hub: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Driver {
    /// WinUSB (or another libusb-compatible driver): DragonSlayer can use it.
    Usable(String),
    /// Windows' own camera driver: this port needs Zadig.
    WindowsOwn(String),
}

impl Driver {
    pub fn usable(&self) -> bool {
        matches!(self, Driver::Usable(_))
    }
}

/// USB cameras currently plugged in. `None` where the check isn't available (not Windows).
/// Takes a second or two: run it off the UI thread.
pub fn scan() -> Option<Result<Vec<UsbCamera>, String>> {
    #[cfg(windows)]
    return Some(scan_windows());
    #[cfg(not(windows))]
    None
}

#[cfg(windows)]
fn scan_windows() -> Result<Vec<UsbCamera>, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let vids = VENDORS.iter().map(|(v, _)| *v).collect::<Vec<_>>().join("|");
    let classes = CAMERA_CLASSES.iter().map(|c| format!("'{c}'")).collect::<Vec<_>>().join(",");
    let script = format!(
        r"$ErrorActionPreference = 'SilentlyContinue'
$r = @(Get-PnpDevice -PresentOnly | Where-Object {{
    $_.InstanceId -match '^USB\\VID_({vids})&PID_[0-9A-F]{{4}}\\' -and $_.Class -in @({classes})
}} | ForEach-Object {{
    $id = $_.InstanceId
    $parent = (Get-PnpDeviceProperty -InstanceId $id -KeyName DEVPKEY_Device_Parent).Data
    [pscustomobject]@{{
        id = $id
        name = $_.FriendlyName
        class = $_.Class
        service = (Get-PnpDeviceProperty -InstanceId $id -KeyName DEVPKEY_Device_Service).Data
        parent = (Get-PnpDevice -InstanceId $parent).FriendlyName
    }}
}})
ConvertTo-Json -InputObject $r -Compress"
    );
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("couldn't run PowerShell: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    parse_windows(&String::from_utf8_lossy(&out.stdout))
}

#[derive(Deserialize)]
struct RawDevice {
    id: String,
    name: Option<String>,
    class: Option<String>,
    service: Option<String>,
    parent: Option<String>,
}

/// Parses the PowerShell scan's JSON. Public so it can be tested without Windows.
pub fn parse_windows(json: &str) -> Result<Vec<UsbCamera>, String> {
    let json = json.trim();
    if json.is_empty() {
        return Ok(Vec::new());
    }
    let raw: Vec<RawDevice> = serde_json::from_str(json).map_err(|e| format!("unexpected device list: {e}"))?;
    Ok(raw
        .into_iter()
        .filter(|d| d.class.as_deref().is_some_and(|c| CAMERA_CLASSES.contains(&c)))
        .filter_map(|d| {
            let vid = d.id.to_ascii_uppercase().split("VID_").nth(1)?.get(..4)?.to_owned();
            let vendor = VENDORS.iter().find(|(v, _)| *v == vid)?.1;
            let service = d.service.unwrap_or_default();
            let driver = match service.to_ascii_lowercase().as_str() {
                "winusb" | "libusbk" | "libusb0" => Driver::Usable(service),
                _ => Driver::WindowsOwn(service),
            };
            let hub = d.parent.filter(|p| !p.to_ascii_lowercase().contains("root hub"));
            let name = d.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("{vendor} camera"));
            Some(UsbCamera { name, vendor, driver, hub })
        })
        .collect())
}
