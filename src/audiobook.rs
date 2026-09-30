//! Audiobooks: the library under `/sdcard/RUSTMIX/AUDIO`, the listening
//! position kept for each title, and what the first bytes of an MP3 file
//! say about its length and where to seek.
//!
//! An audiobook is either a loose `.mp3` in that folder, or a subfolder
//! whose `.mp3` files are its tracks, in natural name order ("2" before
//! "10"): the order chapters are numbered in. Everything here is plain
//! data and file parsing, so it runs in host tests; playback itself lives
//! in `audio::engine`.

use std::{
    cmp::Ordering,
    fs,
    path::{Path, PathBuf},
};

/// Folder the library is scanned from.
pub const AUDIOBOOK_ROOT: &str = "/sdcard/RUSTMIX/AUDIO";
/// Listening positions, one line per title (FAT 8.3 name).
pub const AUDIOBOOK_POSITIONS_PATH: &str = "/sdcard/RUSTMIX/AUDIOPOS.TXT";
/// Titles whose position is remembered; the least recently played go first.
pub const AUDIOBOOK_POSITIONS_MAX: usize = 200;
/// Bytes read from the start of a track to find its first frame and VBR
/// header. ID3 tags with embedded cover art can be larger: then the tag
/// size is still known and the stream is probed again right after it.
pub const MP3_PROBE_BYTES: usize = 8 * 1024;

/// One track of an audiobook.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudiobookTrack {
    pub path: PathBuf,
    /// File name without the extension.
    pub title: String,
    pub size_bytes: u64,
}

/// One title in the library.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Audiobook {
    /// Name inside [`AUDIOBOOK_ROOT`] (folder name, or file name for a loose
    /// `.mp3`): the key its listening position is saved under.
    pub key: String,
    pub title: String,
    pub tracks: Vec<AudiobookTrack>,
}

impl Audiobook {
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.tracks.iter().map(|track| track.size_bytes).sum()
    }
}

/// Scan `root` for audiobooks, sorted by title in natural order. A missing
/// folder is an empty library, not an error: it is created on first use.
pub fn scan_audiobooks(root: &Path) -> std::io::Result<Vec<Audiobook>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut books = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            let tracks = scan_tracks(&path)?;
            if !tracks.is_empty() {
                books.push(Audiobook {
                    title: name.clone(),
                    key: name,
                    tracks,
                });
            }
        } else if is_mp3(&path) {
            let size_bytes = entry.metadata().map_or(0, |metadata| metadata.len());
            let title = stem_title(&path);
            books.push(Audiobook {
                title: title.clone(),
                key: name,
                tracks: vec![AudiobookTrack {
                    path,
                    title,
                    size_bytes,
                }],
            });
        }
    }
    books.sort_by(|left, right| natural_cmp(&left.title, &right.title));
    Ok(books)
}

/// The `.mp3` files directly inside one audiobook folder, in natural order.
fn scan_tracks(folder: &Path) -> std::io::Result<Vec<AudiobookTrack>> {
    let mut tracks = Vec::new();
    for entry in fs::read_dir(folder)?.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') || !is_mp3(&path) {
            continue;
        }
        let size_bytes = entry.metadata().map_or(0, |metadata| metadata.len());
        tracks.push(AudiobookTrack {
            title: stem_title(&path),
            path,
            size_bytes,
        });
    }
    tracks.sort_by(|left, right| {
        natural_cmp(
            &left.path.file_name().unwrap_or_default().to_string_lossy(),
            &right.path.file_name().unwrap_or_default().to_string_lossy(),
        )
    });
    Ok(tracks)
}

fn is_mp3(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mp3"))
}

fn stem_title(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Compare names the way people number things: runs of digits by value,
/// the rest ignoring case, so "Capitolo 2" sorts before "Capitolo 10".
#[must_use]
pub fn natural_cmp(left: &str, right: &str) -> Ordering {
    let mut left = left.chars().peekable();
    let mut right = right.chars().peekable();
    // The first difference that does not decide on its own -- letter case,
    // or leading zeros ("01" against "1") -- settles an otherwise equal pair.
    let mut tiebreak = Ordering::Equal;
    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return tiebreak,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(a), Some(b)) if a.is_ascii_digit() && b.is_ascii_digit() => {
                let a = take_digits(&mut left);
                let b = take_digits(&mut right);
                let (a_value, b_value) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
                let ordering = a_value
                    .len()
                    .cmp(&b_value.len())
                    .then_with(|| a_value.cmp(b_value));
                if ordering != Ordering::Equal {
                    return ordering;
                }
                if tiebreak == Ordering::Equal {
                    tiebreak = a.len().cmp(&b.len());
                }
            }
            (Some(a), Some(b)) => {
                let ordering = a.to_lowercase().cmp(b.to_lowercase());
                if ordering != Ordering::Equal {
                    return ordering;
                }
                if tiebreak == Ordering::Equal {
                    tiebreak = a.cmp(&b);
                }
                left.next();
                right.next();
            }
        }
    }
}

fn take_digits(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut digits = String::new();
    while let Some(digit) = chars.peek().copied().filter(char::is_ascii_digit) {
        digits.push(digit);
        chars.next();
    }
    digits
}

/// Where listening stopped in one audiobook.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ListeningPosition {
    /// Index into [`Audiobook::tracks`].
    pub track: usize,
    /// Byte offset in the track file to resume decoding from.
    pub byte_offset: u64,
    /// The same point as a time, for display.
    pub position_ms: u64,
}

/// What the audiobook player is doing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PlayerState {
    #[default]
    Stopped,
    Loading,
    Playing,
    Paused,
    /// The last track played to its end.
    Finished,
    Error,
}

/// The player's state as the UI shows it and the position store saves it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NowPlaying {
    /// [`Audiobook::key`] of the loaded title; empty when none is.
    pub key: String,
    pub title: String,
    pub track: usize,
    pub track_count: usize,
    pub track_title: String,
    pub position: ListeningPosition,
    /// Length of the current track.
    pub duration_ms: u64,
    pub state: PlayerState,
    pub error: Option<String>,
}

impl NowPlaying {
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        !self.key.is_empty()
    }

    /// Playing or about to be: the device must not go to standby.
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self.state, PlayerState::Loading | PlayerState::Playing)
    }
}

/// What the player screen asks the audio engine for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlayerRequest {
    /// Start (or resume, from its saved position) the title with this key.
    Open {
        key: String,
    },
    TogglePause,
    SeekBy {
        seconds: i32,
    },
    SkipTrack {
        forward: bool,
    },
    /// Stop and unload, keeping the position.
    Stop,
    Volume {
        up: bool,
    },
}

/// Every title's [`ListeningPosition`], most recently played last.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AudiobookPositions {
    entries: Vec<(String, ListeningPosition)>,
}

impl AudiobookPositions {
    #[must_use]
    pub fn get(&self, key: &str) -> Option<ListeningPosition> {
        self.entries
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, position)| *position)
    }

    /// Record `position` for `key`, which becomes the most recently played.
    pub fn set(&mut self, key: &str, position: ListeningPosition) {
        self.entries.retain(|(candidate, _)| candidate != key);
        self.entries.push((key.to_owned(), position));
        let excess = self.entries.len().saturating_sub(AUDIOBOOK_POSITIONS_MAX);
        self.entries.drain(..excess);
    }

    pub fn remove(&mut self, key: &str) {
        self.entries.retain(|(candidate, _)| candidate != key);
    }

    /// Key of the title played last, if any.
    #[must_use]
    pub fn most_recent(&self) -> Option<&str> {
        self.entries.last().map(|(key, _)| key.as_str())
    }

    /// Lenient: lines that do not parse are skipped, so one bad line never
    /// costs every other title its position.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut positions = Self::default();
        for line in text.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut fields = line.split('\t');
            let (Some(key), Some(track), Some(byte_offset), Some(position_ms), None) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) else {
                continue;
            };
            let (Ok(track), Ok(byte_offset), Ok(position_ms)) =
                (track.parse(), byte_offset.parse(), position_ms.parse())
            else {
                continue;
            };
            if key.is_empty() {
                continue;
            }
            positions.set(
                key,
                ListeningPosition {
                    track,
                    byte_offset,
                    position_ms,
                },
            );
        }
        positions
    }

    #[must_use]
    pub fn serialized(&self) -> String {
        let mut text = String::from("# RustMix Wave audiobook positions: title, track, byte, ms\n");
        for (key, position) in &self.entries {
            // Tabs and line breaks would split the record; such a name
            // simply is not remembered.
            if key.contains(['\t', '\n', '\r']) {
                continue;
            }
            text.push_str(&format!(
                "{key}\t{}\t{}\t{}\n",
                position.track, position.byte_offset, position.position_ms
            ));
        }
        text
    }

    #[must_use]
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    /// Written to a temporary file first, then renamed over the old one,
    /// so a power cut mid-write leaves the previous positions intact.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let temporary = path.with_extension("TMP");
        fs::write(&temporary, self.serialized())?;
        // FatFs refuses to rename onto an existing file.
        let _ = fs::remove_file(path);
        fs::rename(&temporary, path)
    }
}

/// What the start of an MP3 file says about its audio stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mp3StreamInfo {
    /// Byte offset of the first audio frame (after any ID3v2 tag).
    pub audio_start: u64,
    pub sample_rate: u32,
    pub channels: u8,
    /// Samples per channel in each frame: 1152 (MPEG-1) or 576.
    pub samples_per_frame: u32,
    /// Bitrate of the first frame, in kbit/s.
    pub bitrate_kbps: u32,
    /// Frame count from a Xing/Info or VBRI header, when present.
    pub frame_count: Option<u32>,
    /// Audio byte count from the same header, when present.
    pub stream_bytes: Option<u32>,
    /// Xing seek table: at i% of the duration, the stream is at
    /// `toc[i] / 256` of its bytes.
    pub toc: Option<[u8; 100]>,
}

/// Outcome of probing the first bytes of a file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mp3Probe {
    Found(Mp3StreamInfo),
    /// An ID3v2 tag larger than the probed bytes: probe again from here.
    SkipTo(u64),
    NotMp3,
}

/// Probe `head`, the first bytes of a file.
#[must_use]
pub fn probe_mp3(head: &[u8]) -> Mp3Probe {
    probe_mp3_at(head, 0)
}

/// Probe `bytes`, read from `offset` in the file.
#[must_use]
pub fn probe_mp3_at(bytes: &[u8], offset: u64) -> Mp3Probe {
    let mut start = 0_usize;
    if bytes.len() >= 10 && &bytes[..3] == b"ID3" {
        let size = syncsafe(&bytes[6..10]) as usize;
        let footer = if bytes[5] & 0x10 != 0 { 10 } else { 0 };
        start = 10 + size + footer;
        if start >= bytes.len() {
            return Mp3Probe::SkipTo(offset + start as u64);
        }
    }
    // The first frame whose successor also parses, so a stray 0xFF in
    // leftover tag padding is not mistaken for audio.
    let mut position = start;
    while position + 4 <= bytes.len() {
        if let Some(header) = FrameHeader::parse(&bytes[position..]) {
            let next = position + header.frame_bytes as usize;
            let confirmed = next + 4 > bytes.len()
                || FrameHeader::parse(&bytes[next..])
                    .is_some_and(|following| following.sample_rate == header.sample_rate);
            if confirmed {
                return Mp3Probe::Found(stream_info(
                    &bytes[position..],
                    offset + position as u64,
                    header,
                ));
            }
        }
        position += 1;
    }
    Mp3Probe::NotMp3
}

fn stream_info(frame: &[u8], audio_start: u64, header: FrameHeader) -> Mp3StreamInfo {
    let mut info = Mp3StreamInfo {
        audio_start,
        sample_rate: header.sample_rate,
        channels: header.channels,
        samples_per_frame: header.samples_per_frame,
        bitrate_kbps: header.bitrate_kbps,
        frame_count: None,
        stream_bytes: None,
        toc: None,
    };
    let side_info = match (header.mpeg1, header.channels) {
        (true, 1) => 17,
        (true, _) => 32,
        (false, 1) => 9,
        (false, _) => 17,
    };
    let xing = 4 + side_info;
    if frame.len() >= xing + 8 && matches!(&frame[xing..xing + 4], b"Xing" | b"Info") {
        let flags = be32(&frame[xing + 4..]);
        let mut cursor = xing + 8;
        if flags & 1 != 0 && frame.len() >= cursor + 4 {
            info.frame_count = Some(be32(&frame[cursor..]));
            cursor += 4;
        }
        if flags & 2 != 0 && frame.len() >= cursor + 4 {
            info.stream_bytes = Some(be32(&frame[cursor..]));
            cursor += 4;
        }
        if flags & 4 != 0 && frame.len() >= cursor + 100 {
            let mut toc = [0_u8; 100];
            toc.copy_from_slice(&frame[cursor..cursor + 100]);
            info.toc = Some(toc);
        }
        return info;
    }
    // Fraunhofer's VBRI header sits right after the 32 bytes that follow
    // the frame header, whatever the channel mode.
    if frame.len() >= 36 + 18 && &frame[36..40] == b"VBRI" {
        info.stream_bytes = Some(be32(&frame[36 + 10..]));
        info.frame_count = Some(be32(&frame[36 + 14..]));
    }
    info
}

impl Mp3StreamInfo {
    /// Track length in milliseconds: exact from a VBR header's frame count,
    /// otherwise estimated from the first frame's bitrate.
    #[must_use]
    pub fn duration_ms(&self, file_bytes: u64) -> u64 {
        if let Some(frames) = self.frame_count.filter(|frames| *frames > 0) {
            return u64::from(frames) * u64::from(self.samples_per_frame) * 1000
                / u64::from(self.sample_rate.max(1));
        }
        self.audio_bytes(file_bytes) * 8 / u64::from(self.bitrate_kbps.max(1))
    }

    fn audio_bytes(&self, file_bytes: u64) -> u64 {
        self.stream_bytes
            .map(u64::from)
            .filter(|bytes| *bytes > 0)
            .unwrap_or_else(|| file_bytes.saturating_sub(self.audio_start))
    }

    /// Byte offset to start decoding from to land near `ms`.
    #[must_use]
    pub fn byte_for_ms(&self, ms: u64, file_bytes: u64) -> u64 {
        let duration = self.duration_ms(file_bytes).max(1);
        let ms = ms.min(duration);
        let audio_bytes = self.audio_bytes(file_bytes);
        let within = match self.toc {
            Some(toc) => {
                // Position in hundredths of a percent, interpolated between
                // two table entries.
                let scaled = ms * 10_000 / duration;
                let index = (scaled / 100).min(99) as usize;
                let low = u64::from(toc[index]);
                let high = if index == 99 {
                    256
                } else {
                    u64::from(toc[index + 1])
                };
                let fraction = scaled - index as u64 * 100;
                let at = low * 100 + (high.saturating_sub(low)) * fraction;
                at * audio_bytes / (256 * 100)
            }
            None => ms * audio_bytes / duration,
        };
        (self.audio_start + within).min(file_bytes)
    }

    /// Time at byte offset `byte`, the inverse of [`Self::byte_for_ms`]
    /// (linear, which is exact for constant-bitrate files).
    #[must_use]
    pub fn ms_for_byte(&self, byte: u64, file_bytes: u64) -> u64 {
        let audio_bytes = self.audio_bytes(file_bytes).max(1);
        let within = byte.saturating_sub(self.audio_start).min(audio_bytes);
        within * self.duration_ms(file_bytes) / audio_bytes
    }
}

/// The fields of one MPEG audio Layer III frame header that matter here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FrameHeader {
    mpeg1: bool,
    sample_rate: u32,
    channels: u8,
    samples_per_frame: u32,
    bitrate_kbps: u32,
    frame_bytes: u32,
}

impl FrameHeader {
    fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] & 0xE0 != 0xE0 {
            return None;
        }
        let version = (bytes[1] >> 3) & 0x03; // 3 = MPEG-1, 2 = MPEG-2, 0 = MPEG-2.5
        let layer = (bytes[1] >> 1) & 0x03; // 1 = Layer III
        if version == 1 || layer != 1 {
            return None;
        }
        let bitrate_index = usize::from(bytes[2] >> 4);
        let rate_index = usize::from((bytes[2] >> 2) & 0x03);
        if bitrate_index == 0 || bitrate_index == 15 || rate_index == 3 {
            return None;
        }
        const MPEG1_KBPS: [u32; 15] = [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ];
        const MPEG2_KBPS: [u32; 15] =
            [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
        let mpeg1 = version == 3;
        let bitrate_kbps = if mpeg1 { MPEG1_KBPS } else { MPEG2_KBPS }[bitrate_index];
        let base_rate = [44_100, 48_000, 32_000][rate_index];
        let sample_rate = match version {
            3 => base_rate,
            2 => base_rate / 2,
            _ => base_rate / 4,
        };
        let padding = u32::from((bytes[2] >> 1) & 0x01);
        let channels = if bytes[3] >> 6 == 3 { 1 } else { 2 };
        let samples_per_frame = if mpeg1 { 1152 } else { 576 };
        let frame_bytes = samples_per_frame / 8 * bitrate_kbps * 1000 / sample_rate + padding;
        Some(Self {
            mpeg1,
            sample_rate,
            channels,
            samples_per_frame,
            bitrate_kbps,
            frame_bytes,
        })
    }
}

fn syncsafe(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .take(4)
        .fold(0, |value, byte| (value << 7) | u32::from(byte & 0x7F))
}

fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// "4:05", or "1:02:07" past an hour.
#[must_use]
pub fn format_clock_ms(ms: u64) -> String {
    let seconds = ms / 1000;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        format_clock_ms, natural_cmp, probe_mp3, probe_mp3_at, scan_audiobooks, AudiobookPositions,
        ListeningPosition, Mp3Probe, AUDIOBOOK_POSITIONS_MAX,
    };

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rustmix-audiobook-{name}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// One MPEG-1 Layer III frame header: 128 kbit/s, 44.1 kHz, stereo.
    const FRAME_128K: [u8; 4] = [0xFF, 0xFB, 0x90, 0x00];
    /// 144 * 128000 / 44100, rounded down.
    const FRAME_128K_BYTES: usize = 417;

    fn cbr_stream(frames: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        for _ in 0..frames {
            let mut frame = vec![0_u8; FRAME_128K_BYTES];
            frame[..4].copy_from_slice(&FRAME_128K);
            bytes.extend_from_slice(&frame);
        }
        bytes
    }

    #[test]
    fn natural_order_follows_chapter_numbers() {
        let mut names = vec![
            "Capitolo 10",
            "capitolo 2",
            "Capitolo 1",
            "Epilogo",
            "Capitolo 02b",
        ];
        names.sort_by(|left, right| natural_cmp(left, right));
        assert_eq!(
            names,
            [
                "Capitolo 1",
                "capitolo 2",
                "Capitolo 02b",
                "Capitolo 10",
                "Epilogo"
            ]
        );
        assert_eq!(
            natural_cmp("track 01", "track 1"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn scans_loose_files_and_folders_in_natural_order() {
        let root = temp_dir("scan");
        fs::write(root.join("Racconto breve.mp3"), b"x").unwrap();
        fs::write(root.join("note.txt"), b"x").unwrap();
        fs::write(root.join("._Racconto breve.mp3"), b"x").unwrap();
        let book = root.join("Il nome del vento");
        fs::create_dir_all(&book).unwrap();
        for name in [
            "10 - Fine.mp3",
            "2 - Due.MP3",
            "1 - Inizio.mp3",
            "cover.jpg",
        ] {
            fs::write(book.join(name), b"xx").unwrap();
        }
        fs::create_dir_all(root.join("Vuota")).unwrap();

        let books = scan_audiobooks(&root).unwrap();
        let titles: Vec<&str> = books.iter().map(|book| book.title.as_str()).collect();
        assert_eq!(titles, ["Il nome del vento", "Racconto breve"]);
        let tracks: Vec<&str> = books[0]
            .tracks
            .iter()
            .map(|track| track.title.as_str())
            .collect();
        assert_eq!(tracks, ["1 - Inizio", "2 - Due", "10 - Fine"]);
        assert_eq!(books[0].total_bytes(), 6);
        assert_eq!(books[1].key, "Racconto breve.mp3");
        assert!(scan_audiobooks(&root.join("missing")).unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn positions_round_trip_and_track_the_most_recent_title() {
        let mut positions = AudiobookPositions::default();
        let first = ListeningPosition {
            track: 2,
            byte_offset: 123_456,
            position_ms: 61_000,
        };
        positions.set("Il nome del vento", first);
        positions.set("Racconto.mp3", ListeningPosition::default());
        positions.set("Il nome del vento", ListeningPosition { track: 3, ..first });
        assert_eq!(positions.most_recent(), Some("Il nome del vento"));
        let parsed = AudiobookPositions::parse(&positions.serialized());
        assert_eq!(parsed, positions);
        assert_eq!(parsed.get("Il nome del vento").unwrap().track, 3);
        // Bad lines are skipped, good ones kept.
        let lenient = AudiobookPositions::parse("junk\nA\t1\t2\t3\nB\tx\t2\t3\n");
        assert_eq!(
            lenient.get("A"),
            Some(ListeningPosition {
                track: 1,
                byte_offset: 2,
                position_ms: 3
            })
        );
        assert_eq!(lenient.get("B"), None);
    }

    #[test]
    fn positions_keep_only_the_most_recent_titles() {
        let mut positions = AudiobookPositions::default();
        for index in 0..AUDIOBOOK_POSITIONS_MAX + 5 {
            positions.set(&format!("book {index}"), ListeningPosition::default());
        }
        assert!(positions.get("book 0").is_none());
        assert!(positions
            .get(&format!("book {}", AUDIOBOOK_POSITIONS_MAX + 4))
            .is_some());
    }

    #[test]
    fn positions_save_and_load() {
        let dir = temp_dir("positions");
        let path = dir.join("AUDIOPOS.TXT");
        let mut positions = AudiobookPositions::default();
        positions.set(
            "Libro",
            ListeningPosition {
                track: 1,
                byte_offset: 9,
                position_ms: 10,
            },
        );
        positions.save(&path).unwrap();
        positions.save(&path).unwrap();
        assert_eq!(AudiobookPositions::load(&path), positions);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn probes_a_constant_bitrate_stream_after_an_id3_tag() {
        let mut file = b"ID3\x04\x00\x00\x00\x00\x00\x20".to_vec();
        file.extend_from_slice(&[0_u8; 32]);
        file.extend_from_slice(&cbr_stream(4));
        let Mp3Probe::Found(info) = probe_mp3(&file) else {
            panic!("no stream found");
        };
        assert_eq!(info.audio_start, 42);
        assert_eq!(
            (info.sample_rate, info.channels, info.bitrate_kbps),
            (44_100, 2, 128)
        );
        // 128 kbit/s: 16 000 bytes per second of audio.
        let file_bytes = 42 + 16_000 * 60;
        assert_eq!(info.duration_ms(file_bytes), 60_000);
        assert_eq!(info.byte_for_ms(30_000, file_bytes), 42 + 16_000 * 30);
        assert_eq!(info.ms_for_byte(42 + 16_000 * 30, file_bytes), 30_000);
    }

    #[test]
    fn a_tag_larger_than_the_probe_asks_to_skip_past_it() {
        let head = b"ID3\x03\x00\x00\x00\x01\x00\x00".to_vec(); // 16 KiB tag
        assert_eq!(probe_mp3(&head), Mp3Probe::SkipTo(10 + 16_384));
        let stream = cbr_stream(2);
        assert!(
            matches!(probe_mp3_at(&stream, 16_394), Mp3Probe::Found(info) if info.audio_start == 16_394)
        );
        assert_eq!(probe_mp3(b"not an mp3 at all, just text"), Mp3Probe::NotMp3);
    }

    #[test]
    fn a_xing_header_gives_the_exact_length_and_a_seek_table() {
        let mut frame = cbr_stream(2);
        // Stereo MPEG-1: the Xing tag follows 4 + 32 bytes.
        frame[36..40].copy_from_slice(b"Xing");
        frame[40..44].copy_from_slice(&7_u32.to_be_bytes()); // frames, bytes, TOC
        frame[44..48].copy_from_slice(&(2_297_u32).to_be_bytes()); // ~60 s at 1152/44100
        frame[48..52].copy_from_slice(&(1_000_000_u32).to_be_bytes());
        for (index, value) in frame[52..152].iter_mut().enumerate() {
            *value = (index * 256 / 100) as u8;
        }
        let Mp3Probe::Found(info) = probe_mp3(&frame) else {
            panic!("no stream found");
        };
        assert_eq!(info.frame_count, Some(2_297));
        assert_eq!(info.duration_ms(5_000_000), 60_003);
        // Halfway through the time is halfway through the stream's bytes.
        let middle = info.byte_for_ms(30_001, 5_000_000);
        assert!((499_000..=501_000).contains(&middle), "{middle}");
    }

    #[test]
    fn clock_labels() {
        assert_eq!(format_clock_ms(0), "0:00");
        assert_eq!(format_clock_ms(245_900), "4:05");
        assert_eq!(format_clock_ms(3_727_000), "1:02:07");
    }
}
