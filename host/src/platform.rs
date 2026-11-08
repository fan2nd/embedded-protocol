//! Host naming conventions; collection and USB protocol stay platform independent.
use sysinfo::NetworkData;

pub fn default_interface(os: &str, name: &str, loopback: bool) -> bool {
    let lower = name.to_ascii_lowercase();
    if loopback || matches!(lower.as_str(), "lo" | "lo0") || lower.contains("loopback") {
        return false;
    }
    match os {
        "macos" => name.starts_with("en"),
        "linux" => ["en", "eth", "wl", "ww", "usb"]
            .iter()
            .any(|p| name.starts_with(p)),
        // Windows adapter names may be localized or user-renamed. No English-only whitelist.
        "windows" => true,
        _ => true,
    }
}

pub fn selected_interface(name: &str, data: &NetworkData, selected: Option<&str>) -> bool {
    if let Some(selected) = selected {
        return name == selected;
    }
    let ips = data.ip_networks();
    let loopback = !ips.is_empty() && ips.iter().all(|ip| ip.addr.is_loopback());
    default_interface(std::env::consts::OS, name, loopback)
}

pub fn serial_permission_hint() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Check the device permissions. Install packaging/linux/70-pc-monitor.rules, reload udev, then reconnect the board; see README.md.",
        "windows" => "Close other serial monitors using this COM port. Use the native USB CDC port, not CH343.",
        _ => "Close other serial monitors and run from a normal Terminal with access to the USB device.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn naming_and_localized_windows_adapters() {
        for (os, name) in [
            ("macos", "en0"),
            ("linux", "eth0"),
            ("linux", "enp3s0"),
            ("linux", "wlan0"),
            ("linux", "wlp2s0"),
            ("linux", "usb0"),
            ("windows", "Ethernet"),
            ("windows", "Wi-Fi"),
            ("windows", "以太网"),
        ] {
            assert!(default_interface(os, name, false), "{os}/{name}");
        }
        for (os, name) in [
            ("macos", "lo0"),
            ("macos", "utun0"),
            ("linux", "lo"),
            ("linux", "docker0"),
            ("linux", "veth123"),
            ("windows", "Loopback Pseudo-Interface 1"),
        ] {
            assert!(!default_interface(os, name, false), "{os}/{name}");
        }
        assert!(!default_interface("windows", "本地回环", true));
    }
}
