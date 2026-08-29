//! Magic: The Gathering token library.
//!
//! Tokens are never bundled with the firmware. The user curates a personal
//! library from their phone (see the "Magic Tokens" section added to the
//! existing Wi-Fi Transfer portal in `wifi_transfer.rs`): search Scryfall,
//! pick tokens, and the browser dithers and uploads pre-sized 1bpp art
//! through the portal's existing generic upload endpoint. This module only
//! reads what lands on the SD card afterward — see `docs/SD_CARD_SETUP.md`
//! for the on-disk layout.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::buttons::ButtonEvent;

/// Default catalog root. Tests override this via
/// [`MagicUiState::refresh_catalog_from_root`].
pub const MAGIC_TOKENS_DIRECTORY: &str = "/sdcard/RUSTMIX/MAGIC";
const INDEX_FILE: &str = "INDEX.TXT";
const ACTIVE_FILE: &str = "ACTIVE.TXT";
/// At most two tokens can be shown on screen at once: one full-size, or two
/// side by side.
pub const MAGIC_ACTIVE_LIMIT: usize = 2;
/// Token rows shown per screen page before the fixed action row(s).
pub const MAGIC_CATALOG_PAGE_SIZE: usize = 5;

const TOKEN_MAGIC: [u8; 4] = *b"RWMT";
const TOKEN_VERSION: u8 = 1;
const TOKEN_HEADER_BYTES: usize = 4 + 1 + 2 + 2; // magic+version+width+height

/// Full-screen tile: one token shown alone in Portrait, sized to the MTG
/// card's 2.5:3.5 aspect ratio.
pub const FULL_TILE_WIDTH: u16 = 420;
pub const FULL_TILE_HEIGHT: u16 = 588;
/// Pair tile: two tokens shown side by side in Landscape (see
/// [`MagicUiState::load_view_tiles`] and
/// `AppState::sync_orientation_for_active_route`) — the device rotates 90°
/// so each token still reads upright and large. Sized to exactly half the
/// 800x480 Landscape canvas so the pair fills the screen edge to edge with
/// no gap or margin (a full-bleed crop, not the card's own aspect ratio).
pub const HALF_TILE_WIDTH: u16 = 400;
pub const HALF_TILE_HEIGHT: u16 = 480;

/// One catalog row, parsed from `INDEX.TXT`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MagicToken {
    /// Six-hex-character id, also the `.TOK` filename stem prefix
    /// (FAT-8.3-safe: `<ID>_F.TOK` / `<ID>_H.TOK`).
    pub id: String,
    pub name: String,
    pub power_toughness: String,
}

/// One decoded 1bpp tile, packed MSB-first, bit `1` = ink — directly usable
/// as the byte slice backing an `embedded_graphics::image::ImageRaw` or
/// `OrientedFrameBuffer::blit_packed_bitmap_portrait`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MagicTile {
    pub width: u16,
    pub height: u16,
    pub bits: Vec<u8>,
}

/// What a SELECT press on the library list's current row should do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MagicRowAction {
    None,
    ToggledActive,
    OpenView,
    OpenConfigure,
}

/// Token library list cursor, up-to-two active selection, and (once the view
/// screen is entered) the loaded tiles ready to blit. Rendering itself never
/// touches the filesystem — see [`MagicUiState::load_view_tiles`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MagicUiState {
    root: PathBuf,
    pub entries: Vec<MagicToken>,
    pub warning: Option<String>,
    pub selected: usize,
    /// Ids of the tokens currently chosen for on-screen display, oldest
    /// first — toggling a third one evicts index 0.
    pub active: Vec<String>,
    pub view_tiles: Vec<MagicTile>,
}

impl Default for MagicUiState {
    fn default() -> Self {
        Self {
            root: PathBuf::from(MAGIC_TOKENS_DIRECTORY),
            entries: Vec::new(),
            warning: Some("catalog has not been scanned".into()),
            selected: 0,
            active: Vec::new(),
            view_tiles: Vec::new(),
        }
    }
}

impl MagicUiState {
    pub fn refresh_catalog(&mut self, mounted: bool) {
        self.refresh_catalog_from_root(MAGIC_TOKENS_DIRECTORY, mounted);
    }

    pub fn refresh_catalog_from_root(&mut self, root: impl Into<PathBuf>, mounted: bool) {
        self.root = root.into();
        if !mounted {
            self.entries = Vec::new();
            self.warning = Some("SD card is unavailable".into());
            self.active = Vec::new();
            self.selected = 0;
            return;
        }
        match fs::read_to_string(self.root.join(INDEX_FILE)) {
            Ok(text) => {
                self.entries = parse_index(&text);
                self.warning = None;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.entries = Vec::new();
                self.warning = None;
            }
            Err(error) => {
                self.entries = Vec::new();
                self.warning = Some(format!("failed to read INDEX.TXT: {error}"));
            }
        }
        self.active = match fs::read_to_string(self.root.join(ACTIVE_FILE)) {
            Ok(text) => parse_active(&text),
            Err(_) => Vec::new(),
        };
        self.active
            .retain(|id| self.entries.iter().any(|entry| &entry.id == id));
        self.active.truncate(MAGIC_ACTIVE_LIMIT);
        self.selected = self.selected.min(self.row_count() - 1);
    }

    /// Token rows, plus a "Show on screen" row once at least one token is
    /// active, plus the always-present "Configure from phone" row.
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.entries.len() + usize::from(!self.active.is_empty()) + 1
    }

    #[must_use]
    pub fn view_row_index(&self) -> Option<usize> {
        (!self.active.is_empty()).then_some(self.entries.len())
    }

    #[must_use]
    pub fn configure_row_index(&self) -> usize {
        self.row_count() - 1
    }

    pub fn apply_row_button(&mut self, event: ButtonEvent) -> MagicRowAction {
        let count = self.row_count();
        match event {
            ButtonEvent::Up => {
                self.selected = self.selected.checked_sub(1).unwrap_or(count - 1);
                MagicRowAction::None
            }
            ButtonEvent::Down => {
                self.selected = (self.selected + 1) % count;
                MagicRowAction::None
            }
            ButtonEvent::Select => {
                if self.selected < self.entries.len() {
                    let id = self.entries[self.selected].id.clone();
                    self.toggle_active(&id);
                    MagicRowAction::ToggledActive
                } else if Some(self.selected) == self.view_row_index() {
                    MagicRowAction::OpenView
                } else {
                    MagicRowAction::OpenConfigure
                }
            }
        }
    }

    pub fn toggle_active(&mut self, id: &str) {
        if let Some(position) = self.active.iter().position(|active| active == id) {
            self.active.remove(position);
        } else {
            if self.active.len() >= MAGIC_ACTIVE_LIMIT {
                self.active.remove(0);
            }
            self.active.push(id.to_string());
        }
        self.selected = self.selected.min(self.row_count() - 1);
        self.persist_active();
    }

    fn persist_active(&self) {
        let mut text = String::new();
        for id in &self.active {
            text.push_str(id);
            text.push('\n');
        }
        if let Err(error) = atomic_write(&self.root.join(ACTIVE_FILE), text.as_bytes()) {
            log::warn!("rustmix-wave=magic-tokens status=active-write-failed error={error:#}");
        }
    }

    /// Load the tile(s) needed to render the view screen for the current
    /// `active` selection: the full tile when exactly one token is active,
    /// or both halves when two are. Best-effort — a missing or corrupt tile
    /// is silently skipped rather than surfaced as an error, since rendering
    /// itself cannot fail.
    pub fn load_view_tiles(&mut self) {
        let use_half = self.active.len() > 1;
        self.view_tiles = self
            .active
            .iter()
            .filter_map(|id| {
                let path = if use_half {
                    half_tile_path(&self.root, id)
                } else {
                    full_tile_path(&self.root, id)
                };
                load_tile(&path)
            })
            .collect();
    }

    #[must_use]
    pub fn active_tokens(&self) -> Vec<&MagicToken> {
        self.active
            .iter()
            .filter_map(|id| self.entries.iter().find(|entry| &entry.id == id))
            .collect()
    }
}

fn parse_index(text: &str) -> Vec<MagicToken> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let mut parts = line.splitn(3, '|');
            let id = parts.next()?.trim();
            let name = parts.next()?.trim();
            let power_toughness = parts.next().unwrap_or("").trim();
            if id.is_empty() || name.is_empty() {
                return None;
            }
            Some(MagicToken {
                id: id.to_string(),
                name: name.to_string(),
                power_toughness: power_toughness.to_string(),
            })
        })
        .collect()
}

fn parse_active(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[must_use]
pub fn full_tile_path(root: &Path, id: &str) -> PathBuf {
    root.join(format!("{id}_F.TOK"))
}

#[must_use]
pub fn half_tile_path(root: &Path, id: &str) -> PathBuf {
    root.join(format!("{id}_H.TOK"))
}

fn load_tile(path: &Path) -> Option<MagicTile> {
    let bytes = fs::read(path).ok()?;
    parse_token_bytes(&bytes)
}

/// Decode one `.TOK` file: magic `RWMT`, version, width/height (`u16` LE),
/// then MSB-first packed 1bpp bits with bit `1` = ink — mirrors the header
/// shape `cover_cache.rs` uses for its own `.THB` thumbnail cache.
#[must_use]
pub fn parse_token_bytes(bytes: &[u8]) -> Option<MagicTile> {
    if bytes.len() < TOKEN_HEADER_BYTES
        || bytes[0..4] != TOKEN_MAGIC
        || bytes[4] != TOKEN_VERSION
    {
        return None;
    }
    let width = u16::from_le_bytes([bytes[5], bytes[6]]);
    let height = u16::from_le_bytes([bytes[7], bytes[8]]);
    if width == 0 || height == 0 {
        return None;
    }
    let row_bytes = (width as usize).div_ceil(8);
    let payload = &bytes[TOKEN_HEADER_BYTES..];
    if payload.len() != row_bytes * height as usize {
        return None;
    }
    Some(MagicTile {
        width,
        height,
        bits: payload.to_vec(),
    })
}

/// Encode one `.TOK` file. Only used by tests on the device side — the real
/// encoder runs client-side in the phone portal's JavaScript — but keeping
/// it here lets [`parse_token_bytes`] be round-trip tested against the exact
/// header shape it must accept.
#[must_use]
#[cfg(test)]
fn encode_token_bytes(tile: &MagicTile) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(TOKEN_HEADER_BYTES + tile.bits.len());
    bytes.extend_from_slice(&TOKEN_MAGIC);
    bytes.push(TOKEN_VERSION);
    bytes.extend_from_slice(&tile.width.to_le_bytes());
    bytes.extend_from_slice(&tile.height.to_le_bytes());
    bytes.extend_from_slice(&tile.bits);
    bytes
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("TMP");
    fs::write(&tmp_path, bytes)?;
    fs::rename(&tmp_path, path)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        encode_token_bytes, full_tile_path, half_tile_path, parse_token_bytes, MagicRowAction,
        MagicTile, MagicUiState, MAGIC_ACTIVE_LIMIT,
    };
    use crate::buttons::ButtonEvent;

    fn temp_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rustmix-magic-tokens-{name}-{nonce}"))
    }

    #[test]
    fn token_header_round_trips() {
        let tile = MagicTile {
            width: 10,
            height: 2,
            bits: vec![0xFF, 0xC0, 0x0F, 0x00],
        };
        let bytes = encode_token_bytes(&tile);
        assert_eq!(parse_token_bytes(&bytes), Some(tile));
    }

    #[test]
    fn token_header_rejects_wrong_magic_or_length() {
        assert_eq!(parse_token_bytes(b"short"), None);
        let mut bytes = encode_token_bytes(&MagicTile {
            width: 8,
            height: 1,
            bits: vec![0x00],
        });
        bytes[0] = b'X';
        assert_eq!(parse_token_bytes(&bytes), None);
    }

    #[test]
    fn full_and_half_tile_paths_are_fat83_safe() {
        let root = PathBuf::from("/sdcard/RUSTMIX/MAGIC");
        assert_eq!(
            full_tile_path(&root, "A1B2C3"),
            root.join("A1B2C3_F.TOK")
        );
        assert_eq!(
            half_tile_path(&root, "A1B2C3"),
            root.join("A1B2C3_H.TOK")
        );
    }

    #[test]
    fn missing_catalog_is_treated_as_empty_not_an_error() {
        let root = temp_dir("missing-index");
        let mut state = MagicUiState::default();
        state.refresh_catalog_from_root(&root, true);
        assert!(state.entries.is_empty());
        assert!(state.warning.is_none());
        assert_eq!(state.row_count(), 1);
    }

    #[test]
    fn unmounted_card_reports_a_warning_and_clears_active() {
        let mut state = MagicUiState::default();
        state.active = vec!["AAAAAA".into()];
        state.refresh_catalog_from_root("/does/not/matter", false);
        assert!(state.warning.is_some());
        assert!(state.active.is_empty());
    }

    #[test]
    fn catalog_parses_index_and_prunes_stale_active_ids() {
        let root = temp_dir("catalog");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("INDEX.TXT"),
            "A1B2C3|Human|1/1\nD4E5F6|Dragon|2/2\n",
        )
        .unwrap();
        fs::write(root.join("ACTIVE.TXT"), "D4E5F6\nSTALE0\n").unwrap();

        let mut state = MagicUiState::default();
        state.refresh_catalog_from_root(&root, true);

        assert_eq!(state.entries.len(), 2);
        assert_eq!(state.entries[0].name, "Human");
        assert_eq!(state.entries[1].power_toughness, "2/2");
        assert_eq!(state.active, vec!["D4E5F6".to_string()]);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn toggling_a_third_active_token_evicts_the_oldest() {
        let root = temp_dir("evict");
        fs::create_dir_all(&root).unwrap();
        let mut state = MagicUiState::default();
        state.refresh_catalog_from_root(&root, true);
        state.entries = vec![
            token("AAAAAA", "Human"),
            token("BBBBBB", "Zombie"),
            token("CCCCCC", "Dragon"),
        ];

        state.toggle_active("AAAAAA");
        state.toggle_active("BBBBBB");
        assert_eq!(state.active, vec!["AAAAAA", "BBBBBB"]);
        state.toggle_active("CCCCCC");
        assert_eq!(state.active.len(), MAGIC_ACTIVE_LIMIT);
        assert_eq!(state.active, vec!["BBBBBB", "CCCCCC"]);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn select_on_a_token_row_toggles_active_without_navigating() {
        let root = temp_dir("select-toggle");
        let mut state = MagicUiState::default();
        state.refresh_catalog_from_root(&root, true);
        state.entries = vec![token("AAAAAA", "Human")];
        state.selected = 0;

        assert_eq!(
            state.apply_row_button(ButtonEvent::Select),
            MagicRowAction::ToggledActive
        );
        assert_eq!(state.active, vec!["AAAAAA"]);
    }

    #[test]
    fn select_on_the_trailing_rows_opens_view_then_configure() {
        let root = temp_dir("select-actions");
        let mut state = MagicUiState::default();
        state.refresh_catalog_from_root(&root, true);
        state.entries = vec![token("AAAAAA", "Human")];
        state.active = vec!["AAAAAA".into()];
        // row 0 = token, row 1 = "show on screen", row 2 = "configure".
        assert_eq!(state.row_count(), 3);

        state.selected = 1;
        assert_eq!(
            state.apply_row_button(ButtonEvent::Select),
            MagicRowAction::OpenView
        );

        state.selected = 2;
        assert_eq!(
            state.apply_row_button(ButtonEvent::Select),
            MagicRowAction::OpenConfigure
        );
    }

    fn token(id: &str, name: &str) -> super::MagicToken {
        super::MagicToken {
            id: id.to_string(),
            name: name.to_string(),
            power_toughness: "1/1".to_string(),
        }
    }
}
