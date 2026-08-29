//! Wi-Fi scan result type shared between `NetworkRuntime`'s scan machinery
//! and the phone provisioning portal, which serializes scan results to JSON
//! for its "nearby networks" list instead of rendering them on-device.

/// One access point discovered by the most recent scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WifiScanEntry {
    pub ssid: String,
    pub rssi_dbm: i32,
}

/// Sort strongest-first and drop duplicate SSIDs (an AP advertising on
/// multiple channels/bands shows up once per channel in a raw scan).
#[must_use]
pub fn dedupe_sorted_by_strength(mut networks: Vec<WifiScanEntry>) -> Vec<WifiScanEntry> {
    networks.sort_by(|a, b| b.rssi_dbm.cmp(&a.rssi_dbm));
    let mut deduped: Vec<WifiScanEntry> = Vec::with_capacity(networks.len());
    for entry in networks {
        if !deduped.iter().any(|kept: &WifiScanEntry| kept.ssid == entry.ssid) {
            deduped.push(entry);
        }
    }
    deduped
}

#[cfg(test)]
mod tests {
    use super::{dedupe_sorted_by_strength, WifiScanEntry};

    fn entry(ssid: &str, rssi_dbm: i32) -> WifiScanEntry {
        WifiScanEntry {
            ssid: ssid.into(),
            rssi_dbm,
        }
    }

    #[test]
    fn sorts_strongest_first_and_dedupes_by_ssid() {
        let result = dedupe_sorted_by_strength(vec![
            entry("Weak", -80),
            entry("Lab", -40),
            entry("Lab", -55),
            entry("Strong", -30),
        ]);
        assert_eq!(result, vec![entry("Strong", -30), entry("Lab", -40), entry("Weak", -80)]);
    }
}
