//! Sleep-image discovery and strict native-panel BMP decoding, plus a tiny
//! wake marker file (the only write this module does) recording which image
//! was last shown.
//!
//! Sleep assets live below `/sdcard/RUSTMIX/SLEEP`. They are deliberately
//! decoded into the panel's native 800 × 480, 1-bpp framebuffer rather than
//! passing through the logical 480 × 800 portrait UI rotation layer.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Result};
use embedded_graphics::prelude::Point;

use crate::framebuffer::{FrameBuffer, FRAMEBUFFER_SIZE, HEIGHT, ROW_BYTES, WIDTH};

/// Runtime directory containing removable-SD sleep images.
pub const SLEEP_IMAGE_DIRECTORY: &str = "/sdcard/RUSTMIX/SLEEP";
/// Marker file recording the file name of the last sleep image shown. Real
/// hardware deep sleep is a full MCU reboot, so nothing in RAM survives a
/// SELECT-key wake; this tiny file lets the fresh boot reload the exact same
/// frame and draw the wake-in-progress overlay on top of it before the rest
/// of boot has run. Ignored by the BMP-only directory scan above.
pub const WAKE_MARKER_FILE: &str = "LASTWAKE.TXT";
/// Bounded number of files examined on each entry to sleep mode.
pub const MAX_SLEEP_IMAGE_CANDIDATES: usize = 32;
const BMP_FILE_HEADER_BYTES: usize = 14;
const BMP_INFO_HEADER_BYTES: usize = 40;
const BMP_PALETTE_BYTES: usize = 8;
const BMP_MIN_PIXEL_OFFSET: usize =
    BMP_FILE_HEADER_BYTES + BMP_INFO_HEADER_BYTES + BMP_PALETTE_BYTES;

/// Scan diagnostics retained with the selected frame so serial logs can
/// distinguish a missing directory from incomplete FAT VFS type hints.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SleepImageScanStats {
    pub raw_entries: usize,
    pub candidate_entries: usize,
    pub metadata_fallbacks: usize,
    pub ignored_entries: usize,
    pub rejected_entries: usize,
}

/// Random-selection diagnostics retained with the selected frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SleepImageChoice {
    pub random_word: u32,
    pub previous_index: Option<usize>,
    pub selected_index: usize,
    pub anti_repeat: bool,
}

/// One selected sleep image ready for direct native-panel transfer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SleepImageSelection {
    pub file_name: String,
    pub frame: FrameBuffer,
    pub valid_count: usize,
    pub rejected_count: usize,
    pub raw_entries: usize,
    pub candidate_entries: usize,
    pub metadata_fallbacks: usize,
    pub ignored_entries: usize,
    pub scan_error: Option<String>,
    pub choice: Option<SleepImageChoice>,
    pub fallback: bool,
}

/// Read-only SD-backed sleep-image catalog.
#[derive(Clone, Debug)]
pub struct SleepImageCatalog {
    directory: PathBuf,
    last_selected_file_name: Option<String>,
}

impl Default for SleepImageCatalog {
    fn default() -> Self {
        Self::new(SLEEP_IMAGE_DIRECTORY)
    }
}

impl SleepImageCatalog {
    #[must_use]
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            last_selected_file_name: None,
        }
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Scan read-only assets, skip malformed BMPs and select a hardware-seeded
    /// random valid image. When multiple assets exist, the previous image is
    /// removed from the choice space so consecutive entries never repeat it.
    /// Missing or invalid directories fall back to a built-in native-panel
    /// sleep frame.
    pub fn select_random(&mut self, random_word: u32) -> SleepImageSelection {
        match self.scan_valid_images() {
            Ok((valid, mut stats)) if !valid.is_empty() => {
                let previous_index = self
                    .last_selected_file_name
                    .as_deref()
                    .and_then(|previous| {
                        valid
                            .iter()
                            .position(|path| file_name_label(path) == previous)
                    });
                let (index, anti_repeat) =
                    choose_random_index(valid.len(), previous_index, random_word);
                let path = &valid[index];
                let file_name = file_name_label(path);
                self.last_selected_file_name = Some(file_name.clone());
                let choice = Some(SleepImageChoice {
                    random_word,
                    previous_index,
                    selected_index: index,
                    anti_repeat,
                });
                match decode_sleep_bmp_file(path) {
                    Ok(frame) => {
                        self.selection(file_name, frame, valid.len(), stats, None, choice, false)
                    }
                    Err(error) => {
                        stats.rejected_entries = stats.rejected_entries.saturating_add(1);
                        self.fallback(
                            stats,
                            Some(format!("selected BMP decode failed: {error:#}")),
                        )
                    }
                }
            }
            Ok((_valid, stats)) => self.fallback(stats, None),
            Err(error) => self.fallback(
                SleepImageScanStats::default(),
                Some(format!("directory scan failed: {error}")),
            ),
        }
    }

    /// Persist the file name of the image just shown so a real hardware
    /// deep-sleep wake can reload it. Best-effort: the caller logs failures
    /// and keeps going either way, the same as the other sleep-entry
    /// teardown steps around it.
    pub fn record_wake_marker(&self, file_name: &str) -> io::Result<()> {
        fs::write(self.directory.join(WAKE_MARKER_FILE), file_name)
    }

    /// Reload the frame recorded by [`Self::record_wake_marker`] for drawing
    /// behind the wake overlay right after a real hardware deep-sleep
    /// reboot. Falls back to the built-in frame when there is no marker, or
    /// the recorded file is missing, invalid, or no longer decodes cleanly.
    #[must_use]
    pub fn load_wake_marker_frame(&self) -> FrameBuffer {
        self.read_wake_marker()
            .and_then(|file_name| decode_sleep_bmp_file(&self.directory.join(file_name)).ok())
            .unwrap_or_else(built_in_sleep_frame)
    }

    fn read_wake_marker(&self) -> Option<String> {
        let raw = fs::read_to_string(self.directory.join(WAKE_MARKER_FILE)).ok()?;
        let file_name = raw.trim();
        if file_name.is_empty() || file_name.contains(['/', '\\']) {
            return None;
        }
        has_bmp_extension(Path::new(file_name)).then(|| file_name.to_string())
    }

    fn scan_valid_images(&self) -> io::Result<(Vec<PathBuf>, SleepImageScanStats)> {
        let mut candidates = Vec::new();
        let mut stats = SleepImageScanStats::default();
        for entry in fs::read_dir(&self.directory)?.take(MAX_SLEEP_IMAGE_CANDIDATES) {
            let entry = entry?;
            stats.raw_entries = stats.raw_entries.saturating_add(1);

            let hinted_type = entry.file_type().ok();
            if hinted_type
                .as_ref()
                .is_some_and(|file_type| file_type.is_symlink())
            {
                stats.ignored_entries = stats.ignored_entries.saturating_add(1);
                continue;
            }

            // ESP-IDF FAT VFS may expose an incomplete d_type hint. Mirror the
            // verified read-only Files browser: use metadata from the
            // immediately following stat call as the final classification.
            let metadata = entry.metadata()?;
            let hint_is_incomplete = hinted_type.as_ref().map_or(true, |file_type| {
                !file_type.is_file() && !file_type.is_dir()
            });
            if hint_is_incomplete {
                stats.metadata_fallbacks = stats.metadata_fallbacks.saturating_add(1);
            }

            if !metadata.file_type().is_file() || !has_bmp_extension(&entry.path()) {
                stats.ignored_entries = stats.ignored_entries.saturating_add(1);
                continue;
            }
            stats.candidate_entries = stats.candidate_entries.saturating_add(1);
            candidates.push(entry.path());
        }
        candidates.sort_by_key(|path| file_name_label(path).to_ascii_uppercase());

        let mut valid = Vec::new();
        for path in candidates {
            match fs::read(&path)
                .ok()
                .and_then(|bytes| decode_sleep_bmp(&bytes).ok())
            {
                Some(_) => valid.push(path),
                None => stats.rejected_entries = stats.rejected_entries.saturating_add(1),
            }
        }
        Ok((valid, stats))
    }

    fn fallback(
        &self,
        stats: SleepImageScanStats,
        scan_error: Option<String>,
    ) -> SleepImageSelection {
        self.selection(
            "built-in".into(),
            built_in_sleep_frame(),
            0,
            stats,
            scan_error,
            None,
            true,
        )
    }

    fn selection(
        &self,
        file_name: String,
        frame: FrameBuffer,
        valid_count: usize,
        stats: SleepImageScanStats,
        scan_error: Option<String>,
        choice: Option<SleepImageChoice>,
        fallback: bool,
    ) -> SleepImageSelection {
        SleepImageSelection {
            file_name,
            frame,
            valid_count,
            rejected_count: stats.rejected_entries,
            raw_entries: stats.raw_entries,
            candidate_entries: stats.candidate_entries,
            metadata_fallbacks: stats.metadata_fallbacks,
            ignored_entries: stats.ignored_entries,
            scan_error,
            choice,
            fallback,
        }
    }
}

/// Choose a bounded random slot while excluding the previous slot when possible.
fn choose_random_index(
    valid_count: usize,
    previous_index: Option<usize>,
    random_word: u32,
) -> (usize, bool) {
    if valid_count <= 1 {
        return (0, false);
    }

    if let Some(previous_index) = previous_index.filter(|index| *index < valid_count) {
        let slot = random_word as usize % (valid_count - 1);
        let selected_index = if slot >= previous_index {
            slot + 1
        } else {
            slot
        };
        (selected_index, true)
    } else {
        (random_word as usize % valid_count, false)
    }
}

/// Decode one strict uncompressed 1-bpp Windows BMP into native panel bytes.
pub fn decode_sleep_bmp_file(path: &Path) -> Result<FrameBuffer> {
    let bytes = fs::read(path).with_context(|| format!("read sleep BMP {}", path.display()))?;
    decode_sleep_bmp(&bytes)
}

/// Decode bytes using a deliberately narrow hardware-safe BMP contract.
pub fn decode_sleep_bmp(bytes: &[u8]) -> Result<FrameBuffer> {
    if bytes.len() < BMP_MIN_PIXEL_OFFSET {
        bail!("BMP is shorter than the required header and palette");
    }
    if &bytes[0..2] != b"BM" {
        bail!("BMP signature is missing");
    }

    let declared_size = read_u32(bytes, 2)? as usize;
    let pixel_offset = read_u32(bytes, 10)? as usize;
    let dib_header_size = read_u32(bytes, 14)? as usize;
    let width = read_i32(bytes, 18)?;
    let height = read_i32(bytes, 22)?;
    let planes = read_u16(bytes, 26)?;
    let bits_per_pixel = read_u16(bytes, 28)?;
    let compression = read_u32(bytes, 30)?;

    if declared_size != bytes.len() {
        bail!(
            "BMP declared size {declared_size} does not match actual size {}",
            bytes.len()
        );
    }
    if dib_header_size < BMP_INFO_HEADER_BYTES {
        bail!("unsupported BMP DIB header size {dib_header_size}");
    }
    if width != WIDTH as i32 || height != HEIGHT as i32 {
        bail!("sleep BMP must be native {} x {} bottom-up", WIDTH, HEIGHT);
    }
    if planes != 1 || bits_per_pixel != 1 || compression != 0 {
        bail!("sleep BMP must be uncompressed 1-bpp BI_RGB");
    }
    if pixel_offset < BMP_MIN_PIXEL_OFFSET || pixel_offset + FRAMEBUFFER_SIZE > bytes.len() {
        bail!("sleep BMP pixel payload is truncated or has an invalid offset");
    }

    let palette0 = palette_luma(bytes, 54)?;
    let palette1 = palette_luma(bytes, 58)?;
    let invert_bits = match (palette0, palette1) {
        (0, 255) => false,
        (255, 0) => true,
        _ => bail!("sleep BMP palette must contain one black and one white entry"),
    };

    let source = &bytes[pixel_offset..pixel_offset + FRAMEBUFFER_SIZE];
    let mut native = vec![0_u8; FRAMEBUFFER_SIZE];
    for destination_y in 0..HEIGHT as usize {
        let source_y = HEIGHT as usize - 1 - destination_y;
        let source_row = &source[source_y * ROW_BYTES..(source_y + 1) * ROW_BYTES];
        let destination_row =
            &mut native[destination_y * ROW_BYTES..(destination_y + 1) * ROW_BYTES];
        if invert_bits {
            for (destination, source) in destination_row.iter_mut().zip(source_row.iter()) {
                *destination = !*source;
            }
        } else {
            destination_row.copy_from_slice(source_row);
        }
    }

    FrameBuffer::from_native_bytes(native).map_err(|message| anyhow!(message))
}

fn built_in_sleep_frame() -> FrameBuffer {
    let mut frame = FrameBuffer::new_white();
    let width = WIDTH as i32;
    let height = HEIGHT as i32;
    for x in 0..width {
        frame.set_native_black(Point::new(x, 0), true);
        frame.set_native_black(Point::new(x, height - 1), true);
    }
    for y in 0..height {
        frame.set_native_black(Point::new(0, y), true);
        frame.set_native_black(Point::new(width - 1, y), true);
    }
    for x in 240..560 {
        frame.set_native_black(Point::new(x, 220), true);
        frame.set_native_black(Point::new(x, 260), true);
    }
    for y in 220..=260 {
        frame.set_native_black(Point::new(240, y), true);
        frame.set_native_black(Point::new(559, y), true);
    }
    frame
}

fn has_bmp_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("bmp"))
}

fn file_name_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("sleep.bmp")
        .to_string()
}

fn palette_luma(bytes: &[u8], offset: usize) -> Result<u8> {
    let blue = *bytes
        .get(offset)
        .ok_or_else(|| anyhow!("BMP palette is truncated"))?;
    let green = *bytes
        .get(offset + 1)
        .ok_or_else(|| anyhow!("BMP palette is truncated"))?;
    let red = *bytes
        .get(offset + 2)
        .ok_or_else(|| anyhow!("BMP palette is truncated"))?;
    if red == green && green == blue {
        Ok(red)
    } else {
        bail!("sleep BMP palette entries must be grayscale")
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| anyhow!("BMP header is truncated"))?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| anyhow!("BMP header is truncated"))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_i32(bytes: &[u8], offset: usize) -> Result<i32> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| anyhow!("BMP header is truncated"))?;
    Ok(i32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        decode_sleep_bmp, SleepImageCatalog, BMP_INFO_HEADER_BYTES, BMP_MIN_PIXEL_OFFSET,
    };
    use crate::framebuffer::{FRAMEBUFFER_SIZE, HEIGHT, ROW_BYTES, WIDTH};

    fn fixture() -> Vec<u8> {
        include_bytes!("../examples/sd-card/RUSTMIX/SLEEP/SLEEP.BMP").to_vec()
    }

    #[test]
    fn accepts_uploaded_native_sleep_fixture() {
        let frame = decode_sleep_bmp(&fixture()).unwrap();
        assert_eq!(frame.as_bytes().len(), FRAMEBUFFER_SIZE);
    }

    #[test]
    fn rejects_wrong_dimensions_and_color_depth() {
        let mut wrong_width = fixture();
        wrong_width[18..22].copy_from_slice(&799_i32.to_le_bytes());
        assert!(decode_sleep_bmp(&wrong_width).is_err());
        let mut wrong_depth = fixture();
        wrong_depth[28..30].copy_from_slice(&8_u16.to_le_bytes());
        assert!(decode_sleep_bmp(&wrong_depth).is_err());
    }

    #[test]
    fn rejects_compressed_and_truncated_payloads() {
        let mut compressed = fixture();
        compressed[30..34].copy_from_slice(&1_u32.to_le_bytes());
        assert!(decode_sleep_bmp(&compressed).is_err());
        let mut truncated = fixture();
        truncated.truncate(100);
        assert!(decode_sleep_bmp(&truncated).is_err());
    }

    #[test]
    fn reverses_bottom_up_rows_into_native_panel_order() {
        let mut bytes = fixture();
        let pixel_offset = 62;
        bytes[pixel_offset..pixel_offset + FRAMEBUFFER_SIZE].fill(0xFF);
        let bottom_row = pixel_offset + (HEIGHT as usize - 1) * ROW_BYTES;
        bytes[bottom_row] = 0x7F;
        let frame = decode_sleep_bmp(&bytes).unwrap();
        assert_eq!(frame.as_bytes()[0], 0x7F);
        assert_eq!(frame.as_bytes()[ROW_BYTES], 0xFF);
    }

    #[test]
    fn inverts_reversed_black_white_palette() {
        let mut bytes = fixture();
        bytes[54..58].copy_from_slice(&[255, 255, 255, 0]);
        bytes[58..62].copy_from_slice(&[0, 0, 0, 0]);
        let pixel_offset = 62;
        bytes[pixel_offset..pixel_offset + FRAMEBUFFER_SIZE].fill(0x00);
        let frame = decode_sleep_bmp(&bytes).unwrap();
        assert!(frame.as_bytes().iter().all(|byte| *byte == 0xFF));
    }

    #[test]
    fn random_selection_avoids_immediate_repeat_when_multiple_assets_exist() {
        let root = unique_temp_dir("random-anti-repeat");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("SLEEP01.BMP"), fixture()).unwrap();
        fs::write(root.join("SLEEP.BMP"), fixture()).unwrap();
        let mut catalog = SleepImageCatalog::new(&root);
        let first = catalog.select_random(0);
        let second = catalog.select_random(0);
        assert_eq!(first.file_name, "SLEEP.BMP");
        assert_eq!(second.file_name, "SLEEP01.BMP");
        assert_ne!(first.file_name, second.file_name);
        assert!(!first.choice.unwrap().anti_repeat);
        assert!(second.choice.unwrap().anti_repeat);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_reports_candidate_diagnostics_for_valid_assets() {
        let root = unique_temp_dir("diagnostics");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("SLEEP.BMP"), fixture()).unwrap();
        fs::write(root.join("README.TXT"), b"ignored").unwrap();
        let mut catalog = SleepImageCatalog::new(&root);
        let selection = catalog.select_random(0);
        assert!(!selection.fallback);
        assert_eq!(selection.valid_count, 1);
        assert_eq!(selection.raw_entries, 2);
        assert_eq!(selection.candidate_entries, 1);
        assert_eq!(selection.ignored_entries, 1);
        assert!(selection.scan_error.is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_directory_reports_scan_error() {
        let mut catalog = SleepImageCatalog::new(unique_temp_dir("missing-error"));
        let selection = catalog.select_random(0);
        assert!(selection.fallback);
        assert!(selection
            .scan_error
            .as_deref()
            .is_some_and(|error| error.contains("directory scan failed")));
    }

    #[test]
    fn missing_directory_uses_built_in_fallback() {
        let mut catalog = SleepImageCatalog::new(unique_temp_dir("missing"));
        let selection = catalog.select_random(0);
        assert!(selection.fallback);
        assert_eq!(selection.file_name, "built-in");
        assert_eq!(
            selection.frame.as_bytes().len(),
            (WIDTH as usize / 8) * HEIGHT as usize
        );
    }

    #[test]
    fn wake_marker_round_trips_the_exact_recorded_frame() {
        let root = unique_temp_dir("wake-marker-roundtrip");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("SLEEP.BMP"), fixture()).unwrap();
        let catalog = SleepImageCatalog::new(&root);
        catalog.record_wake_marker("SLEEP.BMP").unwrap();
        let frame = catalog.load_wake_marker_frame();
        assert_eq!(frame, decode_sleep_bmp(&fixture()).unwrap());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn wake_marker_falls_back_to_built_in_frame_without_a_marker() {
        let root = unique_temp_dir("wake-marker-missing");
        fs::create_dir_all(&root).unwrap();
        let catalog = SleepImageCatalog::new(&root);
        let frame = catalog.load_wake_marker_frame();
        assert_eq!(
            frame.as_bytes().len(),
            (WIDTH as usize / 8) * HEIGHT as usize
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn wake_marker_rejects_path_traversal_and_missing_extension() {
        let root = unique_temp_dir("wake-marker-hostile");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("SLEEP.BMP"), fixture()).unwrap();
        let catalog = SleepImageCatalog::new(&root);

        fs::write(root.join(super::WAKE_MARKER_FILE), "../SLEEP.BMP").unwrap();
        assert!(catalog.read_wake_marker().is_none());

        fs::write(root.join(super::WAKE_MARKER_FILE), "SLEEP.TXT").unwrap();
        assert!(catalog.read_wake_marker().is_none());

        fs::write(root.join(super::WAKE_MARKER_FILE), "SLEEP.BMP\n").unwrap();
        assert_eq!(catalog.read_wake_marker().as_deref(), Some("SLEEP.BMP"));

        let _ = fs::remove_dir_all(root);
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rustmix-wave-sleep-images-{label}-{nanos}"))
    }

    /// Mirrors the byte-for-byte layout the Wi-Fi portal's browser-side
    /// `buildSleepBmp` encodes (see `wifi_transfer.rs`): 62-byte header,
    /// 2-entry grayscale palette, bottom-up rows, MSB-first bit packing with
    /// `is_white(x, y)` selecting the pixel at display coordinates.
    fn encode_browser_style_bmp(is_white: impl Fn(u32, u32) -> bool) -> Vec<u8> {
        let mut bytes = vec![0_u8; BMP_MIN_PIXEL_OFFSET + FRAMEBUFFER_SIZE];
        bytes[0] = b'B';
        bytes[1] = b'M';
        let total_len = bytes.len() as u32;
        bytes[2..6].copy_from_slice(&total_len.to_le_bytes());
        bytes[10..14].copy_from_slice(&(BMP_MIN_PIXEL_OFFSET as u32).to_le_bytes());
        bytes[14..18].copy_from_slice(&(BMP_INFO_HEADER_BYTES as u32).to_le_bytes());
        bytes[18..22].copy_from_slice(&(WIDTH as i32).to_le_bytes());
        bytes[22..26].copy_from_slice(&(HEIGHT as i32).to_le_bytes());
        bytes[26..28].copy_from_slice(&1_u16.to_le_bytes());
        bytes[28..30].copy_from_slice(&1_u16.to_le_bytes());
        bytes[34..38].copy_from_slice(&(FRAMEBUFFER_SIZE as u32).to_le_bytes());
        bytes[46..50].copy_from_slice(&2_u32.to_le_bytes());
        bytes[54..58].copy_from_slice(&[0, 0, 0, 0]);
        bytes[58..62].copy_from_slice(&[255, 255, 255, 0]);
        let mut offset = BMP_MIN_PIXEL_OFFSET;
        for canvas_row in (0..HEIGHT).rev() {
            for byte_index in 0..ROW_BYTES as u32 {
                let mut byte = 0_u8;
                for bit in 0..8 {
                    let x = byte_index * 8 + bit;
                    if is_white(x, canvas_row) {
                        byte |= 1 << (7 - bit);
                    }
                }
                bytes[offset] = byte;
                offset += 1;
            }
        }
        bytes
    }

    #[test]
    fn browser_encoded_bmp_decodes_and_preserves_pixel_positions() {
        let bytes = encode_browser_style_bmp(|x, y| !(x == 0 && y == 0));
        let frame = decode_sleep_bmp(&bytes).unwrap();
        assert_eq!(frame.as_bytes().len(), FRAMEBUFFER_SIZE);
        assert_eq!(frame.as_bytes()[0], 0x7F);
        assert!(frame.as_bytes()[1..].iter().all(|byte| *byte == 0xFF));
    }
}
