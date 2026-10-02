//! Updating the second-stage bootloader over the air.
//!
//! The ESP32-S3's ROM starts the bootloader only from flash address 0 and
//! has no backup copy to fall back on (chips with a "recovery bootloader"
//! have that in ROM), so this can never be made fully safe: if the power is
//! cut while the new bootloader is copied over the old one, the device does
//! not start again until it is reflashed over USB. The ROM's own download
//! mode cannot be damaged, so it can always be recovered that way. What
//! this module does is keep that window short and rare:
//!
//! - offered only when the release carries a bootloader that differs from
//!   the installed one, and only once the firmware itself is up to date;
//! - downloaded whole into memory (a bootloader is under 32 KB) and checked
//!   against the SHA-256 GitHub publishes for the asset, the hash appended
//!   to the image, its magic byte, chip id and size, before anything is
//!   written;
//! - written only with the battery at least half full, or on external
//!   power;
//! - staged in unused flash (never in a firmware slot: with rollback on,
//!   the free slot can hold the version rollback would return to) and
//!   verified there by ESP-IDF, then copied over the old one -- only the
//!   sectors the image needs -- and read back; a copy that does not read
//!   back identical is made again while the power is still on.
//!
//! The release asset is named `*-bootloader.img`, not `.bin`: firmware that
//! predates this module takes the first `.bin` asset as the app image.

use crate::sha256::sha256;

/// Name suffix of the bootloader asset on a GitHub release.
pub const BOOTLOADER_ASSET_SUFFIX: &str = "-bootloader.img";
/// Flash between the bootloader (address 0) and the partition table.
pub const BOOTLOADER_REGION_BYTES: usize = 0x8000;
/// Below this charge, without external power, the write is refused.
pub const MIN_BATTERY_PERCENT: u8 = 50;
const SMALLEST_IMAGE_BYTES: usize = 1024;
const IMAGE_MAGIC: u8 = 0xE9;
const MAX_SEGMENTS: u8 = 16;
const ESP32S3_CHIP_ID: u16 = 9;
/// `esp_bootloader_desc_t` follows the 24-byte image header and the first
/// 8-byte segment header.
const DESCRIPTION_OFFSET: usize = 32;
const DESCRIPTION_MAGIC: u8 = 0x50;

/// A bootloader attached to a release, as GitHub lists it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootloaderAsset {
    pub download_url: String,
    pub sha256: [u8; 32],
    pub size: usize,
}

/// What a bootloader says about itself (`esp_bootloader_desc_t`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootloaderDescription {
    pub idf_version: String,
    /// Build date, `YYYY-MM-DD`.
    pub built: String,
}

impl BootloaderDescription {
    /// `v5.5.1, 2026-10-02`.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}, {}", self.idf_version, self.built)
    }
}

/// The description embedded in a bootloader image, if it has one: ESP-IDF
/// bootloaders carry it since v5.1.
#[must_use]
pub fn describe(image: &[u8]) -> Option<BootloaderDescription> {
    let description = image.get(DESCRIPTION_OFFSET..DESCRIPTION_OFFSET + 64)?;
    if description[0] != DESCRIPTION_MAGIC {
        return None;
    }
    let text = |bytes: &[u8]| {
        let end = bytes
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    };
    let idf_version = text(&description[8..40]);
    // `__DATE__ __TIME__`: "Oct  2 2026 15:14:09".
    let date_time = text(&description[40..64]);
    let mut parts = date_time.split_whitespace();
    let built = match (parts.next(), parts.next(), parts.next()) {
        (Some(month), Some(day), Some(year)) => {
            match month_number(month).zip(day.parse::<u8>().ok()) {
                Some((month, day)) => format!("{year}-{month:02}-{day:02}"),
                None => date_time.clone(),
            }
        }
        _ => date_time.clone(),
    };
    Some(BootloaderDescription { idf_version, built })
}

fn month_number(name: &str) -> Option<u8> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    MONTHS
        .iter()
        .position(|month| *month == name)
        .map(|index| index as u8 + 1)
}

/// Every check a downloaded bootloader must pass before anything is
/// written: the digest GitHub published, then the image's own structure.
pub fn check_image(image: &[u8], expected_sha256: &[u8; 32]) -> Result<(), String> {
    if !(SMALLEST_IMAGE_BYTES..=BOOTLOADER_REGION_BYTES).contains(&image.len()) {
        return Err(format!(
            "bootloader of {} bytes, outside {SMALLEST_IMAGE_BYTES}..={BOOTLOADER_REGION_BYTES}",
            image.len()
        ));
    }
    if sha256(image) != *expected_sha256 {
        return Err("bootloader download does not match its published SHA-256".into());
    }
    if image[0] != IMAGE_MAGIC {
        return Err(format!("not an ESP image (first byte 0x{:02x})", image[0]));
    }
    if image[1] == 0 || image[1] > MAX_SEGMENTS {
        return Err(format!("bootloader with {} segments", image[1]));
    }
    let chip_id = u16::from_le_bytes([image[12], image[13]]);
    if chip_id != ESP32S3_CHIP_ID {
        return Err(format!("bootloader for chip id {chip_id}, not an ESP32-S3"));
    }
    if image[23] != 1 {
        return Err("bootloader without an appended SHA-256".into());
    }
    let (body, appended) = image.split_at(image.len() - 32);
    if sha256(body)[..] != *appended {
        return Err("bootloader's appended SHA-256 does not match its contents".into());
    }
    Ok(())
}

/// Whether the power can carry the write: external power, or the battery
/// at least [`MIN_BATTERY_PERCENT`] full. The refusal carries the battery
/// level, for the screen to explain in the user's language.
pub fn power_allows_write(
    battery_percent: Option<u8>,
    external_power: bool,
) -> Result<(), Option<u8>> {
    match battery_percent {
        _ if external_power => Ok(()),
        Some(percent) if percent >= MIN_BATTERY_PERCENT => Ok(()),
        level => Err(level),
    }
}

/// How a write went wrong: the difference is whether the old bootloader is
/// still there.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstallError {
    /// Refused or failed before the copy: the old bootloader is intact.
    NotWritten(String),
    /// The copy did not read back intact, even after the retries: the
    /// device may not start again, so it must stay on.
    Damaged(String),
}

impl core::fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotWritten(reason) => write!(formatter, "not written: {reason}"),
            Self::Damaged(reason) => write!(formatter, "damaged: {reason}"),
        }
    }
}

/// Whether the bootloader region in flash already starts with `asset`.
#[must_use]
pub fn is_installed(region: &[u8], asset: &BootloaderAsset) -> bool {
    asset.size <= region.len() && sha256(&region[..asset.size]) == asset.sha256
}

#[cfg(target_os = "espidf")]
pub mod espidf {
    use core::ptr;
    use std::{ffi::CStr, time::Duration};

    use embedded_svc::{
        http::{client::Client as HttpClient, Method},
        utils::io,
    };
    use esp_idf_svc::{
        http::client::{Configuration as HttpConfiguration, EspHttpConnection},
        sys,
    };
    use log::{info, warn};

    use super::{InstallError, BOOTLOADER_REGION_BYTES};

    const SECTOR_BYTES: usize = 4096;
    /// End of the partition table, where the search for unused flash starts.
    const PARTITION_TABLE_END: u32 = 0x9000;
    const COPY_ATTEMPTS: usize = 3;
    const DOWNLOAD_TIMEOUT_SECONDS: u64 = 60;
    const USER_AGENT: &str = "rustmix-wave-ota";
    const PRIMARY_LABEL: &CStr = c"PrimaryBTLDR";
    const STAGING_LABEL: &CStr = c"OtaBTLDR";

    fn esp_result(code: sys::esp_err_t, action: &str) -> Result<(), String> {
        if code == sys::ESP_OK {
            Ok(())
        } else {
            Err(format!("could not {action} (error 0x{code:x})"))
        }
    }

    /// The bootloader region as it is in flash now.
    pub fn read_installed() -> Result<Vec<u8>, String> {
        let mut region = vec![0_u8; BOOTLOADER_REGION_BYTES];
        esp_result(
            unsafe {
                sys::esp_flash_read(
                    ptr::null_mut(),
                    region.as_mut_ptr().cast(),
                    0,
                    region.len() as u32,
                )
            },
            "read the bootloader",
        )?;
        Ok(region)
    }

    /// A release asset, whole, into memory: at most `limit` bytes.
    pub fn download(url: &str, limit: usize) -> Result<Vec<u8>, String> {
        let http_config = HttpConfiguration {
            crt_bundle_attach: Some(sys::esp_crt_bundle_attach),
            timeout: Some(Duration::from_secs(DOWNLOAD_TIMEOUT_SECONDS)),
            // Release assets redirect to a signed storage URL about 1 KB
            // long; see `ota::espidf::install_update`.
            buffer_size: Some(4096),
            buffer_size_tx: Some(4096),
            ..Default::default()
        };
        let connection = EspHttpConnection::new(&http_config)
            .map_err(|error| format!("HTTP connection init failed: {error}"))?;
        let mut client = HttpClient::wrap(connection);
        let request = client
            .request(Method::Get, url, &[("user-agent", USER_AGENT)])
            .map_err(|error| format!("download request setup failed: {error}"))?;
        let mut response = request
            .submit()
            .map_err(|error| format!("download request failed: {error}"))?;
        let status = response.status();
        if status != 200 {
            return Err(format!("download HTTP status {status}"));
        }
        let mut body = vec![0_u8; limit + 1];
        let read = io::try_read_full(&mut response, &mut body)
            .map_err(|error| format!("download read failed: {}", error.0))?;
        if read > limit {
            return Err(format!("bootloader asset larger than {limit} bytes"));
        }
        body.truncate(read);
        Ok(body)
    }

    /// Write a checked image over the bootloader, then read it back. See
    /// the module documentation for the order of the steps.
    pub fn install(image: &[u8]) -> Result<(), InstallError> {
        let (primary, primary_registered) = bootloader_partition(
            0,
            BOOTLOADER_REGION_BYTES,
            PRIMARY_LABEL,
            sys::esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_BOOTLOADER_PRIMARY,
        )
        .map_err(InstallError::NotWritten)?;
        let result = (|| {
            let staging_offset =
                find_unused_flash(BOOTLOADER_REGION_BYTES).map_err(InstallError::NotWritten)?;
            let (staging, staging_registered) = bootloader_partition(
                staging_offset,
                BOOTLOADER_REGION_BYTES,
                STAGING_LABEL,
                sys::esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_BOOTLOADER_OTA,
            )
            .map_err(InstallError::NotWritten)?;
            let result = stage_and_copy(primary, staging, image);
            if staging_registered {
                unsafe { sys::esp_partition_deregister_external(staging) };
            }
            result
        })();
        if primary_registered {
            unsafe { sys::esp_partition_deregister_external(primary) };
        }
        result
    }

    /// The partition table does not list the bootloader: register it (and
    /// the staging area) for the length of the update, as ESP-IDF's
    /// `partitions_ota` example does. Returns whether it was registered
    /// here, and so must be deregistered.
    fn bootloader_partition(
        offset: u32,
        size: usize,
        label: &CStr,
        subtype: sys::esp_partition_subtype_t,
    ) -> Result<(*const sys::esp_partition_t, bool), String> {
        let bootloader = sys::esp_partition_type_t_ESP_PARTITION_TYPE_BOOTLOADER;
        let existing = unsafe { sys::esp_partition_find_first(bootloader, subtype, ptr::null()) };
        if !existing.is_null() {
            return Ok((existing, false));
        }
        let mut partition: *const sys::esp_partition_t = ptr::null();
        esp_result(
            unsafe {
                sys::esp_partition_register_external(
                    ptr::null_mut(),
                    offset as usize,
                    size,
                    label.as_ptr(),
                    bootloader,
                    subtype,
                    &mut partition,
                )
            },
            "register the bootloader area",
        )?;
        Ok((partition, true))
    }

    /// The first stretch of flash no partition uses, after the partition
    /// table, at least `size` bytes long. On this board's table: 56 KB
    /// between `phy_init` and `ota_0`, at 0x12000.
    fn find_unused_flash(size: usize) -> Result<u32, String> {
        let mut used: Vec<(u32, u32)> = Vec::new();
        let mut iterator = unsafe {
            sys::esp_partition_find(
                sys::esp_partition_type_t_ESP_PARTITION_TYPE_ANY,
                sys::esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_ANY,
                ptr::null(),
            )
        };
        while !iterator.is_null() {
            let partition = unsafe { &*sys::esp_partition_get(iterator) };
            used.push((partition.address, partition.address + partition.size));
            iterator = unsafe { sys::esp_partition_next(iterator) };
        }
        unsafe { sys::esp_partition_iterator_release(iterator) };
        used.sort_unstable();
        let mut start = PARTITION_TABLE_END;
        for (begin, end) in used {
            if begin >= start && (begin - start) as usize >= size {
                return Ok(start);
            }
            start = start.max(end);
        }
        Err("no unused flash to stage the bootloader in".into())
    }

    fn stage_and_copy(
        primary: *const sys::esp_partition_t,
        staging: *const sys::esp_partition_t,
        image: &[u8],
    ) -> Result<(), InstallError> {
        stage(staging, primary, image).map_err(InstallError::NotWritten)?;
        warn!(
            "rustmix-wave=bootloader-update status=copying bytes={}",
            image.len()
        );
        let copy_bytes = image.len().div_ceil(SECTOR_BYTES) * SECTOR_BYTES;
        for attempt in 1..=COPY_ATTEMPTS {
            let copied = unsafe { sys::esp_partition_copy(primary, 0, staging, 0, copy_bytes) };
            let mut written = vec![0_u8; image.len()];
            let read = unsafe {
                sys::esp_partition_read(primary, 0, written.as_mut_ptr().cast(), written.len())
            };
            if copied == sys::ESP_OK && read == sys::ESP_OK && written == image {
                info!("rustmix-wave=bootloader-update status=written attempt={attempt}");
                return Ok(());
            }
            warn!(
                "rustmix-wave=bootloader-update status=readback-mismatch attempt={attempt} copy=0x{copied:x} read=0x{read:x}"
            );
        }
        Err(InstallError::Damaged(format!(
            "the bootloader did not read back intact after {COPY_ATTEMPTS} copies"
        )))
    }

    /// Download into the staging area and have ESP-IDF verify it there; the
    /// bootloader region is not touched.
    fn stage(
        staging: *const sys::esp_partition_t,
        primary: *const sys::esp_partition_t,
        image: &[u8],
    ) -> Result<(), String> {
        let mut handle: sys::esp_ota_handle_t = 0;
        esp_result(
            unsafe { sys::esp_ota_begin(staging, image.len(), &mut handle) },
            "open the staging area",
        )?;
        let staged = (|| {
            // Also lifts the flash driver's write protection on the
            // bootloader region, for the copy below.
            esp_result(
                unsafe { sys::esp_ota_set_final_partition(handle, primary, false) },
                "target the bootloader",
            )?;
            esp_result(
                unsafe { sys::esp_ota_write(handle, image.as_ptr().cast(), image.len()) },
                "stage the bootloader",
            )
        })();
        if let Err(error) = staged {
            unsafe { sys::esp_ota_abort(handle) };
            return Err(error);
        }
        // Verifies the staged image as a bootloader; the old one is still
        // untouched if this fails.
        esp_result(
            unsafe { sys::esp_ota_end(handle) },
            "verify the staged bootloader",
        )?;
        let mut staged_copy = vec![0_u8; image.len()];
        esp_result(
            unsafe {
                sys::esp_partition_read(
                    staging,
                    0,
                    staged_copy.as_mut_ptr().cast(),
                    staged_copy.len(),
                )
            },
            "read the staged bootloader back",
        )?;
        if staged_copy != image {
            return Err("the staged bootloader does not match the download".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        check_image, describe, is_installed, power_allows_write, BootloaderAsset,
        BootloaderDescription, BOOTLOADER_REGION_BYTES,
    };
    use crate::sha256::sha256;

    /// A minimal image shaped like an ESP32-S3 bootloader: header, the
    /// description where ESP-IDF puts it, a body, the appended SHA-256.
    fn image(chip_id: u16, length: usize) -> Vec<u8> {
        let mut image = vec![0_u8; length - 32];
        image[0] = 0xE9;
        image[1] = 3;
        image[12..14].copy_from_slice(&chip_id.to_le_bytes());
        image[23] = 1;
        image[32] = 0x50;
        image[40..46].copy_from_slice(b"v5.5.1");
        image[72..92].copy_from_slice(b"Oct  2 2026 15:14:09");
        for (index, byte) in image.iter_mut().enumerate().skip(160) {
            *byte = (index % 251) as u8;
        }
        let hash = sha256(&image);
        image.extend_from_slice(&hash);
        image
    }

    #[test]
    fn a_well_formed_bootloader_passes_every_check() {
        let image = image(9, 19_008);
        assert_eq!(check_image(&image, &sha256(&image)), Ok(()));
        assert_eq!(
            describe(&image),
            Some(BootloaderDescription {
                idf_version: "v5.5.1".into(),
                built: "2026-10-02".into(),
            })
        );
        assert_eq!(describe(&image).unwrap().label(), "v5.5.1, 2026-10-02");
    }

    #[test]
    fn a_bootloader_is_refused_on_any_mismatch() {
        let good = image(9, 19_008);
        let digest = sha256(&good);
        // Not the file GitHub published.
        assert!(check_image(&good, &sha256(b"other")).is_err());
        // Another chip's bootloader, even with its own correct digest.
        let other_chip = image(5, 19_008);
        assert!(check_image(&other_chip, &sha256(&other_chip))
            .unwrap_err()
            .contains("chip id 5"));
        // Damaged after hashing: the appended SHA-256 no longer matches.
        let mut damaged = good.clone();
        damaged[1000] ^= 1;
        assert!(check_image(&damaged, &sha256(&damaged))
            .unwrap_err()
            .contains("appended"));
        // Larger than the space before the partition table.
        let huge = image(9, BOOTLOADER_REGION_BYTES + 16);
        assert!(check_image(&huge, &sha256(&huge)).is_err());
        let mut not_esp = good.clone();
        not_esp[0] = 0;
        assert!(check_image(&not_esp, &sha256(&not_esp)).is_err());
        assert_eq!(check_image(&good, &digest), Ok(()));
    }

    #[test]
    fn the_write_needs_half_a_battery_or_external_power() {
        assert_eq!(power_allows_write(Some(50), false), Ok(()));
        assert_eq!(power_allows_write(Some(49), false), Err(Some(49)));
        assert_eq!(power_allows_write(Some(5), true), Ok(()));
        assert_eq!(power_allows_write(None, false), Err(None));
        assert_eq!(power_allows_write(None, true), Ok(()));
    }

    #[test]
    fn the_installed_bootloader_is_recognised_by_its_digest() {
        let bootloader = image(9, 19_008);
        let mut region = bootloader.clone();
        region.resize(BOOTLOADER_REGION_BYTES, 0xFF);
        let asset = BootloaderAsset {
            download_url: "https://example.com/b-bootloader.img".into(),
            sha256: sha256(&bootloader),
            size: bootloader.len(),
        };
        assert!(is_installed(&region, &asset));
        region[100] ^= 1;
        assert!(!is_installed(&region, &asset));
        let too_long = BootloaderAsset {
            size: BOOTLOADER_REGION_BYTES + 1,
            ..asset
        };
        assert!(!is_installed(&region, &too_long));
    }

    #[test]
    fn an_image_without_a_description_has_none() {
        let mut image = image(9, 19_008);
        image[32] = 0;
        assert_eq!(describe(&image), None);
        assert_eq!(describe(&[0xE9; 40]), None);
    }
}
