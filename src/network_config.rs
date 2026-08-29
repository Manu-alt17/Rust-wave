//! SD-card Wi-Fi and SNTP provisioning configuration.
//!
//! Credentials are loaded at boot from removable storage. Keep parsing here so
//! firmware wiring never embeds, renders, or logs a password. The file holds a
//! small ordered list of saved networks (numbered `ssid1=`/`password1=`,
//! `ssid2=`/`password2=`, ...) so the boot sequence can fail over between them
//! and the phone provisioning portal can add, update or forget entries
//! without disturbing the others.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    path::Path,
};

use anyhow::{bail, Context, Result};

/// Read-only provisioning file consumed at boot.
pub const WIFI_CONFIG_PATH: &str = "/sdcard/RUSTMIX/WIFI.TXT";
/// Default SNTP pool used when the optional key is omitted.
pub const DEFAULT_NTP_SERVER: &str = "pool.ntp.org";
/// Default timezone profile used when the optional key is omitted.
pub const DEFAULT_TIMEZONE: &str = "America/New_York";
/// Upper bound on saved networks. Keeps the boot-time fail-over sequence
/// bounded and the provisioning portal's list short enough to fit one screen.
pub const WIFI_MAX_SAVED_NETWORKS: usize = 8;

/// One saved Wi-Fi network. Order in [`NetworkConfig::networks`] is the boot
/// fail-over order: the first entry is tried first.
#[derive(Clone, Eq, PartialEq)]
pub struct SavedNetwork {
    pub ssid: String,
    pub password: String,
}

impl core::fmt::Debug for SavedNetwork {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("SavedNetwork")
            .field("ssid", &self.ssid)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Validated boot-time network configuration: an ordered list of saved
/// networks plus the shared (not per-network) timezone and NTP server.
#[derive(Clone, Eq, PartialEq)]
pub struct NetworkConfig {
    pub networks: Vec<SavedNetwork>,
    pub timezone: String,
    pub ntp_server: String,
}

impl core::fmt::Debug for NetworkConfig {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("NetworkConfig")
            .field("networks", &self.networks)
            .field("timezone", &self.timezone)
            .field("ntp_server", &self.ntp_server)
            .finish()
    }
}

impl NetworkConfig {
    /// Load and validate the read-only boot configuration.
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path)
            .with_context(|| format!("unable to read {}", path.display()))?;
        Self::parse(&contents)
            .with_context(|| format!("invalid network configuration in {}", path.display()))
    }

    /// Parse the intentionally small `key=value` provisioning format.
    /// Networks are numbered `ssid1=`/`password1=`, `ssid2=`/`password2=`,
    /// ... up to [`WIFI_MAX_SAVED_NETWORKS`]. The unnumbered `ssid=`/
    /// `password=` pair is accepted as an alias for network 1, so a
    /// hand-written single-network file keeps working unchanged.
    pub fn parse(contents: &str) -> Result<Self> {
        let mut values = BTreeMap::<String, String>::new();
        for (line_number, raw_line) in contents.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("line {} must use key=value", line_number + 1))?;
            let key = key.trim();
            let value = value.trim();
            if !is_known_key(key) {
                bail!("line {} uses unsupported key {key:?}", line_number + 1);
            }
            if values.insert(key.into(), value.into()).is_some() {
                bail!("line {} repeats key {key:?}", line_number + 1);
            }
        }

        let mut networks = Vec::new();
        // Unnumbered ssid=/password= is network 1.
        if let Some(ssid) = values.get("ssid") {
            networks.push(SavedNetwork {
                ssid: ssid.clone(),
                password: values.get("password").cloned().unwrap_or_default(),
            });
        }
        for index in 1..=WIFI_MAX_SAVED_NETWORKS {
            let Some(ssid) = values.get(&format!("ssid{index}")) else {
                continue;
            };
            networks.push(SavedNetwork {
                ssid: ssid.clone(),
                password: values
                    .get(&format!("password{index}"))
                    .cloned()
                    .unwrap_or_default(),
            });
        }

        let timezone = values
            .get("timezone")
            .cloned()
            .unwrap_or_else(|| DEFAULT_TIMEZONE.into());
        let ntp_server = values
            .get("ntp_server")
            .cloned()
            .unwrap_or_else(|| DEFAULT_NTP_SERVER.into());
        Self::validated(networks, timezone, ntp_server)
    }

    /// Build a configuration from already-split fields, applying the same
    /// validation `parse` enforces on every field. Used by the phone
    /// provisioning portal, which submits one SSID/password pair at a time
    /// instead of a hand-written `key=value` file.
    pub fn validated(networks: Vec<SavedNetwork>, timezone: String, ntp_server: String) -> Result<Self> {
        if networks.is_empty() {
            bail!("at least one saved network is required");
        }
        if networks.len() > WIFI_MAX_SAVED_NETWORKS {
            bail!("at most {WIFI_MAX_SAVED_NETWORKS} saved networks are supported");
        }
        for network in &networks {
            validate_ssid(&network.ssid)?;
            validate_password(&network.password)?;
        }
        let mut seen = std::collections::BTreeSet::new();
        for network in &networks {
            if !seen.insert(network.ssid.as_str()) {
                bail!("duplicate saved network {:?}", network.ssid);
            }
        }
        validate_timezone(&timezone)?;
        validate_ntp_server(&ntp_server)?;
        Ok(Self {
            networks,
            timezone,
            ntp_server,
        })
    }

    /// Add a new saved network, or replace the password of an existing one
    /// with the same SSID in place (this is how the provisioning portal
    /// "edits" a saved network's password). Returns an error if this would
    /// exceed [`WIFI_MAX_SAVED_NETWORKS`] for a genuinely new SSID.
    pub fn upsert(&mut self, ssid: String, password: String) -> Result<()> {
        validate_ssid(&ssid)?;
        validate_password(&password)?;
        if let Some(existing) = self.networks.iter_mut().find(|network| network.ssid == ssid) {
            existing.password = password;
            return Ok(());
        }
        if self.networks.len() >= WIFI_MAX_SAVED_NETWORKS {
            bail!("at most {WIFI_MAX_SAVED_NETWORKS} saved networks are supported");
        }
        self.networks.push(SavedNetwork { ssid, password });
        Ok(())
    }

    /// Forget a saved network by SSID. Returns `false` when no such network
    /// was saved (a no-op, not an error, since the caller may be retrying).
    pub fn remove(&mut self, ssid: &str) -> bool {
        let before = self.networks.len();
        self.networks.retain(|network| network.ssid != ssid);
        self.networks.len() != before
    }

    /// Atomically replace the provisioning file at `path` with this
    /// configuration, serialized back to the `key=value` format `parse`
    /// reads. Writes a sibling temporary file first and renames it into
    /// place so a power loss mid-write cannot leave a truncated or corrupt
    /// file.
    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let parent = path
            .parent()
            .context("configuration path has no parent directory")?;
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        let stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .context("configuration path has no file name")?;
        // The SD card's FAT filesystem has no long-filename support here, so
        // names must fit the legacy 8.3 short-name format: at most one dot,
        // at most 8 characters before it. Appending `.tmp` after the real
        // `.TXT` extension (a second dot) fails `File::create` outright with
        // `EINVAL`, so the extension is replaced instead, exactly like the
        // voice-note recorder's own FAT-safe temp file in `voice_notes.rs`.
        let temp_path = parent.join(format!("{stem}.TMP"));
        // ELM FatFs's `f_rename` (what `fs::rename` reaches through ESP-IDF's
        // FAT VFS) refuses to replace an existing destination, unlike POSIX
        // rename: renaming straight onto an already-present `WIFI.TXT` fails
        // with `EEXIST`. The previous file is moved aside to `.BAK` first,
        // matching the same backup-then-rename dance already used for the
        // reader's and voice notes' own state files.
        let backup_path = parent.join(format!("{stem}.BAK"));
        let _ = fs::remove_file(&temp_path);
        let _ = fs::remove_file(&backup_path);
        let mut contents = String::new();
        for (index, network) in self.networks.iter().enumerate() {
            let number = index + 1;
            contents.push_str(&format!("ssid{number}={}\n", network.ssid));
            contents.push_str(&format!("password{number}={}\n", network.password));
        }
        contents.push_str(&format!("timezone={}\n", self.timezone));
        contents.push_str(&format!("ntp_server={}\n", self.ntp_server));
        {
            let mut file = File::create(&temp_path)
                .with_context(|| format!("create {}", temp_path.display()))?;
            file.write_all(contents.as_bytes())
                .with_context(|| format!("write {}", temp_path.display()))?;
            file.sync_all()
                .with_context(|| format!("sync {}", temp_path.display()))?;
        }
        if path.exists() {
            fs::rename(path, &backup_path)
                .with_context(|| format!("back up {}", path.display()))?;
        }
        if let Err(error) = fs::rename(&temp_path, path) {
            if backup_path.exists() {
                let _ = fs::rename(&backup_path, path);
            }
            return Err(error).with_context(|| format!("replace {}", path.display()));
        }
        let _ = fs::remove_file(&backup_path);
        Ok(())
    }
}

fn is_known_key(key: &str) -> bool {
    if matches!(key, "ssid" | "password" | "timezone" | "ntp_server") {
        return true;
    }
    for prefix in ["ssid", "password"] {
        if let Some(suffix) = key.strip_prefix(prefix) {
            if let Ok(index) = suffix.parse::<usize>() {
                return index >= 1 && index <= WIFI_MAX_SAVED_NETWORKS;
            }
        }
    }
    false
}

fn validate_ssid(ssid: &str) -> Result<()> {
    if ssid.is_empty() || ssid.len() > 32 {
        bail!("ssid must contain 1 to 32 UTF-8 bytes");
    }
    Ok(())
}

fn validate_password(password: &str) -> Result<()> {
    if password.len() > 63 {
        bail!("password must contain at most 63 UTF-8 bytes");
    }
    if !password.is_empty() && password.len() < 8 {
        bail!("secured Wi-Fi password must contain at least 8 UTF-8 bytes");
    }
    Ok(())
}

fn validate_timezone(timezone: &str) -> Result<()> {
    if !matches!(timezone, "America/New_York" | "UTC" | "Europe/Rome") {
        bail!("timezone must be America/New_York, UTC or Europe/Rome in this milestone");
    }
    Ok(())
}

fn validate_ntp_server(ntp_server: &str) -> Result<()> {
    if ntp_server.is_empty() || ntp_server.len() > 63 || ntp_server.chars().any(char::is_whitespace)
    {
        bail!("ntp_server must be a non-empty hostname without whitespace");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{NetworkConfig, SavedNetwork, DEFAULT_NTP_SERVER, DEFAULT_TIMEZONE, WIFI_MAX_SAVED_NETWORKS};

    fn network(ssid: &str, password: &str) -> SavedNetwork {
        SavedNetwork {
            ssid: ssid.into(),
            password: password.into(),
        }
    }

    #[test]
    fn parses_minimal_configuration_with_safe_defaults() {
        let config = NetworkConfig::parse("ssid=Lab WiFi\npassword=correct-horse\n").unwrap();
        assert_eq!(config.networks, vec![network("Lab WiFi", "correct-horse")]);
        assert_eq!(config.timezone, DEFAULT_TIMEZONE);
        assert_eq!(config.ntp_server, DEFAULT_NTP_SERVER);
    }

    #[test]
    fn parses_comments_open_network_and_explicit_timezone() {
        let config = NetworkConfig::parse(
            "# removable SD provisioning\nssid=Guest\npassword=\ntimezone=UTC\nntp_server=time.example.org\n",
        )
        .unwrap();
        assert_eq!(config.networks, vec![network("Guest", "")]);
        assert_eq!(config.timezone, "UTC");
        assert_eq!(config.ntp_server, "time.example.org");
    }

    #[test]
    fn accepts_europe_rome_timezone() {
        let config = NetworkConfig::parse("ssid=Casa\npassword=\ntimezone=Europe/Rome\n").unwrap();
        assert_eq!(config.timezone, "Europe/Rome");
    }

    #[test]
    fn debug_output_never_leaks_password() {
        let config = NetworkConfig::parse("ssid=Lab\npassword=secret123\n").unwrap();
        let debug = format!("{config:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("secret123"));
    }

    #[test]
    fn rejects_unknown_duplicate_and_short_password_keys() {
        assert!(NetworkConfig::parse("ssid=Lab\nextra=value\n").is_err());
        assert!(NetworkConfig::parse("ssid=Lab\nssid=Other\n").is_err());
        assert!(NetworkConfig::parse("ssid=Lab\npassword=short\n").is_err());
    }

    #[test]
    fn parses_multiple_numbered_networks_in_order() {
        let config = NetworkConfig::parse(
            "ssid1=Home\npassword1=correct-horse\nssid2=Office\npassword2=battery-staple\ntimezone=UTC\n",
        )
        .unwrap();
        assert_eq!(
            config.networks,
            vec![
                network("Home", "correct-horse"),
                network("Office", "battery-staple"),
            ]
        );
    }

    #[test]
    fn unnumbered_ssid_is_an_alias_for_network_one_and_can_combine_with_numbered_entries() {
        let config = NetworkConfig::parse(
            "ssid=Home\npassword=correct-horse\nssid2=Office\npassword2=battery-staple\n",
        )
        .unwrap();
        assert_eq!(
            config.networks,
            vec![
                network("Home", "correct-horse"),
                network("Office", "battery-staple"),
            ]
        );
    }

    #[test]
    fn rejects_duplicate_ssids_and_networks_beyond_the_cap() {
        assert!(NetworkConfig::parse("ssid1=Home\npassword1=\nssid2=Home\npassword2=\n").is_err());

        let mut contents = String::new();
        for index in 1..=(WIFI_MAX_SAVED_NETWORKS + 1) {
            contents.push_str(&format!("ssid{index}=Net{index}\npassword{index}=\n"));
        }
        assert!(NetworkConfig::parse(&contents).is_err());
    }

    #[test]
    fn rejects_index_zero_and_out_of_range_numbered_keys() {
        assert!(NetworkConfig::parse("ssid0=Home\npassword0=\n").is_err());
        assert!(NetworkConfig::parse(&format!(
            "ssid{}=Home\npassword{0}=\n",
            WIFI_MAX_SAVED_NETWORKS + 1
        ))
        .is_err());
    }

    #[test]
    fn validated_applies_the_same_rules_as_parse() {
        assert!(NetworkConfig::validated(
            vec![network("Lab WiFi", "correct-horse")],
            DEFAULT_TIMEZONE.into(),
            DEFAULT_NTP_SERVER.into(),
        )
        .is_ok());
        assert!(NetworkConfig::validated(Vec::new(), DEFAULT_TIMEZONE.into(), DEFAULT_NTP_SERVER.into())
            .is_err());
        assert!(NetworkConfig::validated(
            vec![network("Lab", "short")],
            DEFAULT_TIMEZONE.into(),
            DEFAULT_NTP_SERVER.into(),
        )
        .is_err());
    }

    #[test]
    fn upsert_adds_a_new_network_and_replaces_an_existing_password() {
        let mut config = NetworkConfig::validated(
            vec![network("Home", "correct-horse")],
            DEFAULT_TIMEZONE.into(),
            DEFAULT_NTP_SERVER.into(),
        )
        .unwrap();

        config.upsert("Office".into(), "battery-staple".into()).unwrap();
        assert_eq!(
            config.networks,
            vec![network("Home", "correct-horse"), network("Office", "battery-staple")]
        );

        config.upsert("Home".into(), "new-password".into()).unwrap();
        assert_eq!(
            config.networks,
            vec![network("Home", "new-password"), network("Office", "battery-staple")]
        );
    }

    #[test]
    fn upsert_rejects_a_new_network_past_the_cap() {
        let mut networks = Vec::new();
        for index in 0..WIFI_MAX_SAVED_NETWORKS {
            networks.push(network(&format!("Net{index}"), "correct-horse"));
        }
        let mut config =
            NetworkConfig::validated(networks, DEFAULT_TIMEZONE.into(), DEFAULT_NTP_SERVER.into()).unwrap();
        assert!(config.upsert("OneMore".into(), "correct-horse".into()).is_err());
    }

    #[test]
    fn remove_forgets_a_saved_network_and_is_a_no_op_when_absent() {
        let mut config = NetworkConfig::validated(
            vec![network("Home", "correct-horse"), network("Office", "battery-staple")],
            DEFAULT_TIMEZONE.into(),
            DEFAULT_NTP_SERVER.into(),
        )
        .unwrap();

        assert!(config.remove("Home"));
        assert_eq!(config.networks, vec![network("Office", "battery-staple")]);
        assert!(!config.remove("Home"));
    }

    #[test]
    fn save_to_path_round_trips_through_parse_with_multiple_networks() {
        let dir = std::env::temp_dir().join(format!(
            "rustmix-wave-network-config-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("WIFI.TXT");

        let config = NetworkConfig::validated(
            vec![network("Lab WiFi", "correct-horse"), network("Travel", "second-pass")],
            "UTC".into(),
            "time.example.org".into(),
        )
        .unwrap();
        config.save_to_path(&path).unwrap();

        let reloaded = NetworkConfig::load_from_path(&path).unwrap();
        assert_eq!(reloaded.networks, config.networks);
        assert_eq!(reloaded.timezone, "UTC");
        assert_eq!(reloaded.ntp_server, "time.example.org");
        assert!(!dir.join("WIFI.TMP").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_to_path_overwrites_an_already_existing_file_and_leaves_no_stray_files() {
        // `f_rename` (reached through `fs::rename` on the SD card's FAT
        // filesystem) refuses to replace an existing destination, unlike a
        // host filesystem's `rename`. Saving twice onto the same path is
        // exactly the on-device situation (join a network, then forget one)
        // that first surfaced this: the second `save_to_path` must still
        // succeed and must not leave a `.TMP`/`.BAK` file behind.
        let dir = std::env::temp_dir().join(format!(
            "rustmix-wave-network-config-overwrite-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("WIFI.TXT");

        let first = NetworkConfig::validated(
            vec![network("Lab WiFi", "correct-horse")],
            DEFAULT_TIMEZONE.into(),
            DEFAULT_NTP_SERVER.into(),
        )
        .unwrap();
        first.save_to_path(&path).unwrap();

        let second = NetworkConfig::validated(
            vec![network("Office", "battery-staple")],
            DEFAULT_TIMEZONE.into(),
            DEFAULT_NTP_SERVER.into(),
        )
        .unwrap();
        second.save_to_path(&path).unwrap();

        let reloaded = NetworkConfig::load_from_path(&path).unwrap();
        assert_eq!(reloaded.networks, second.networks);
        assert!(!dir.join("WIFI.TMP").exists());
        assert!(!dir.join("WIFI.BAK").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_to_path_uses_a_fat_8_3_safe_temp_file_name() {
        // A second dot (e.g. `WIFI.TXT.tmp`) is not a valid legacy 8.3 short
        // name and fails `File::create` with `EINVAL` on the SD card's FAT
        // filesystem (no long-filename support configured), so the
        // temporary file must replace the extension instead of appending to
        // it.
        let dir = std::env::temp_dir().join(format!(
            "rustmix-wave-network-config-fat-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("WIFI.TXT");

        let config = NetworkConfig::validated(
            vec![network("Lab WiFi", "correct-horse")],
            DEFAULT_TIMEZONE.into(),
            DEFAULT_NTP_SERVER.into(),
        )
        .unwrap();
        config.save_to_path(&path).unwrap();

        assert!(dir.join("WIFI.TMP").parent().unwrap().exists());
        assert!(!dir.join("WIFI.TXT.tmp").exists());
        assert!(path.exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
