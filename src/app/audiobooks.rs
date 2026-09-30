//! State behind the Audiobooks screens: the library list, the player and
//! its menu. Hardware-free: the main loop scans the card, runs the audio
//! engine and feeds the results back in, and takes the player's requests
//! out with [`AudiobookUiState::take_request`].

use crate::{
    audiobook::{Audiobook, AudiobookPositions, NowPlaying, PlayerRequest, PlayerState},
    buttons::ButtonEvent,
    regional::Locale,
};

/// While playing, the player screen redraws for a position change only
/// this often: every e-paper refresh costs power and some ghosting.
pub const PLAYER_POSITION_REDRAW_MS: u64 = 15_000;
/// How far the player menu's skip entries jump.
pub const PLAYER_SKIP_SECONDS: i32 = 30;

/// Entries of the menu a long SELECT opens on the player.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlayerMenuItem {
    Back30,
    Forward30,
    PreviousTrack,
    NextTrack,
    Stop,
}

impl PlayerMenuItem {
    pub const ALL: [Self; 5] = [
        Self::Back30,
        Self::Forward30,
        Self::PreviousTrack,
        Self::NextTrack,
        Self::Stop,
    ];

    #[must_use]
    pub const fn label_i18n(self, locale: Locale) -> &'static str {
        match (self, locale) {
            (Self::Back30, Locale::English) => "Back 30 s",
            (Self::Back30, Locale::Italian) => "Indietro 30 s",
            (Self::Forward30, Locale::English) => "Forward 30 s",
            (Self::Forward30, Locale::Italian) => "Avanti 30 s",
            (Self::PreviousTrack, Locale::English) => "Previous track",
            (Self::PreviousTrack, Locale::Italian) => "Traccia precedente",
            (Self::NextTrack, Locale::English) => "Next track",
            (Self::NextTrack, Locale::Italian) => "Traccia successiva",
            (Self::Stop, Locale::English) => "Stop",
            (Self::Stop, Locale::Italian) => "Ferma",
        }
    }

    const fn request(self) -> PlayerRequest {
        match self {
            Self::Back30 => PlayerRequest::SeekBy {
                seconds: -PLAYER_SKIP_SECONDS,
            },
            Self::Forward30 => PlayerRequest::SeekBy {
                seconds: PLAYER_SKIP_SECONDS,
            },
            Self::PreviousTrack => PlayerRequest::SkipTrack { forward: false },
            Self::NextTrack => PlayerRequest::SkipTrack { forward: true },
            Self::Stop => PlayerRequest::Stop,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AudiobookUiState {
    pub books: Vec<Audiobook>,
    pub scan_error: Option<String>,
    pub selected: usize,
    pub positions: AudiobookPositions,
    pub now_playing: NowPlaying,
    /// Selected row while the player menu is open.
    pub menu: Option<usize>,
    /// Position the player screen was last drawn with.
    drawn_position_ms: u64,
    request: Option<PlayerRequest>,
}

impl AudiobookUiState {
    /// Take a fresh library scan. The selection lands on the title played
    /// last, so reopening the list puts it under the cursor.
    pub fn set_library(&mut self, scan: Result<Vec<Audiobook>, String>) {
        match scan {
            Ok(books) => {
                self.books = books;
                self.scan_error = None;
            }
            Err(error) => {
                self.books.clear();
                self.scan_error = Some(error);
            }
        }
        let recent = self
            .positions
            .most_recent()
            .and_then(|key| self.books.iter().position(|book| book.key == key));
        self.selected = recent.unwrap_or(0).min(self.books.len().saturating_sub(1));
    }

    #[must_use]
    pub fn selected_book(&self) -> Option<&Audiobook> {
        self.books.get(self.selected)
    }

    #[must_use]
    pub fn book(&self, key: &str) -> Option<&Audiobook> {
        self.books.iter().find(|book| book.key == key)
    }

    pub fn take_request(&mut self) -> Option<PlayerRequest> {
        self.request.take()
    }

    /// Library list: Up/Down move, SELECT plays the title (resuming it).
    /// Returns `true` when the player should open.
    pub fn apply_library(&mut self, event: ButtonEvent) -> bool {
        let count = self.books.len();
        match event {
            ButtonEvent::Up if count > 0 => {
                self.selected = self.selected.checked_sub(1).unwrap_or(count - 1);
                false
            }
            ButtonEvent::Down if count > 0 => {
                self.selected = (self.selected + 1) % count;
                false
            }
            ButtonEvent::Select => {
                let Some((key, title)) = self
                    .selected_book()
                    .map(|book| (book.key.clone(), book.title.clone()))
                else {
                    return false;
                };
                // Already the loaded title: just show the player.
                if self.now_playing.key != key || !self.now_playing.is_active() {
                    self.now_playing.key.clone_from(&key);
                    self.now_playing.title = title;
                    self.now_playing.state = PlayerState::Loading;
                    self.now_playing.error = None;
                    self.request = Some(PlayerRequest::Open { key });
                }
                self.menu = None;
                true
            }
            _ => false,
        }
    }

    /// Player: SELECT plays or pauses, Up/Down set the volume. With the
    /// menu open, Up/Down move through it and SELECT runs the entry.
    pub fn apply_player(&mut self, event: ButtonEvent) {
        if let Some(selected) = self.menu {
            let count = PlayerMenuItem::ALL.len();
            match event {
                ButtonEvent::Up => self.menu = Some(selected.checked_sub(1).unwrap_or(count - 1)),
                ButtonEvent::Down => self.menu = Some((selected + 1) % count),
                ButtonEvent::Select => {
                    let item = PlayerMenuItem::ALL[selected];
                    self.request = Some(item.request());
                    if item == PlayerMenuItem::Stop {
                        self.menu = None;
                    }
                }
            }
            return;
        }
        self.request = Some(match event {
            ButtonEvent::Select => match self.now_playing.state {
                // Nothing loaded (stopped, finished, failed): play again.
                PlayerState::Stopped | PlayerState::Finished | PlayerState::Error
                    if self.now_playing.is_loaded() =>
                {
                    self.now_playing.state = PlayerState::Loading;
                    PlayerRequest::Open {
                        key: self.now_playing.key.clone(),
                    }
                }
                _ => PlayerRequest::TogglePause,
            },
            ButtonEvent::Up => PlayerRequest::Volume { up: true },
            ButtonEvent::Down => PlayerRequest::Volume { up: false },
        });
    }

    /// Long SELECT on the player opens its menu.
    pub fn open_menu(&mut self) {
        self.menu = Some(0);
    }

    /// BOOT with the menu open closes it; `false` when it was not open.
    pub fn close_menu(&mut self) -> bool {
        self.menu.take().is_some()
    }

    /// Take the engine's latest state; remembers the position. Returns
    /// whether the player screen should be redrawn for it: always for a
    /// change of title, track, state or error, and for the position alone
    /// every [`PLAYER_POSITION_REDRAW_MS`].
    pub fn update_now_playing(&mut self, now: NowPlaying) -> bool {
        if now.is_loaded() {
            self.positions.set(&now.key, now.position);
        }
        let previous = &self.now_playing;
        let changed = previous.key != now.key
            || previous.track != now.track
            || previous.state != now.state
            || previous.error != now.error
            || previous.duration_ms != now.duration_ms;
        let moved =
            now.position.position_ms.abs_diff(self.drawn_position_ms) >= PLAYER_POSITION_REDRAW_MS;
        self.now_playing = now;
        changed || moved
    }

    /// The player screen is being drawn with the current position.
    pub fn note_player_drawn(&mut self) {
        self.drawn_position_ms = self.now_playing.position.position_ms;
    }

    /// Progress through a title, 0-100, from its saved position: tracks
    /// done plus the fraction of the current one by bytes.
    #[must_use]
    pub fn progress_percent(&self, book: &Audiobook) -> Option<u8> {
        let position = self.positions.get(&book.key)?;
        let total = book.total_bytes().max(1);
        let before: u64 = book
            .tracks
            .iter()
            .take(position.track)
            .map(|track| track.size_bytes)
            .sum();
        let current = book
            .tracks
            .get(position.track)
            .map_or(0, |track| position.byte_offset.min(track.size_bytes));
        Some(((before + current) * 100 / total).min(100) as u8)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{AudiobookUiState, PlayerMenuItem, PLAYER_POSITION_REDRAW_MS};
    use crate::{
        audiobook::{
            Audiobook, AudiobookTrack, ListeningPosition, NowPlaying, PlayerRequest, PlayerState,
        },
        buttons::ButtonEvent,
    };

    fn book(key: &str, tracks: usize) -> Audiobook {
        Audiobook {
            key: key.into(),
            title: key.into(),
            tracks: (0..tracks)
                .map(|index| AudiobookTrack {
                    path: PathBuf::from(format!("/a/{key}/{index}.mp3")),
                    title: format!("{index}"),
                    size_bytes: 1_000,
                })
                .collect(),
        }
    }

    #[test]
    fn selection_lands_on_the_title_played_last() {
        let mut state = AudiobookUiState::default();
        state.positions.set(
            "B",
            ListeningPosition {
                track: 1,
                byte_offset: 500,
                position_ms: 9,
            },
        );
        state.set_library(Ok(vec![book("A", 1), book("B", 2), book("C", 1)]));
        assert_eq!(state.selected, 1);
        // Half of the second of two tracks: 75%.
        assert_eq!(state.progress_percent(&state.books[1]), Some(75));
        assert_eq!(state.progress_percent(&state.books[0]), None);
        state.set_library(Err("no card".into()));
        assert_eq!(state.selected, 0);
        assert!(state.scan_error.is_some());
    }

    #[test]
    fn select_in_the_library_opens_the_title() {
        let mut state = AudiobookUiState::default();
        state.set_library(Ok(vec![book("A", 1), book("B", 1)]));
        assert!(!state.apply_library(ButtonEvent::Down));
        assert!(state.apply_library(ButtonEvent::Select));
        assert_eq!(
            state.take_request(),
            Some(PlayerRequest::Open { key: "B".into() })
        );
        assert_eq!(state.now_playing.state, PlayerState::Loading);
        // Selecting the title already playing just shows the player.
        state.now_playing.state = PlayerState::Playing;
        assert!(state.apply_library(ButtonEvent::Select));
        assert_eq!(state.take_request(), None);
    }

    #[test]
    fn player_buttons_and_menu_map_to_requests() {
        let mut state = AudiobookUiState::default();
        state.now_playing = NowPlaying {
            key: "A".into(),
            state: PlayerState::Playing,
            ..NowPlaying::default()
        };
        state.apply_player(ButtonEvent::Select);
        assert_eq!(state.take_request(), Some(PlayerRequest::TogglePause));
        state.apply_player(ButtonEvent::Up);
        assert_eq!(
            state.take_request(),
            Some(PlayerRequest::Volume { up: true })
        );

        state.open_menu();
        state.apply_player(ButtonEvent::Down);
        state.apply_player(ButtonEvent::Select);
        assert_eq!(
            state.take_request(),
            Some(PlayerRequest::SeekBy { seconds: 30 })
        );
        assert_eq!(state.menu, Some(1));
        state.apply_player(ButtonEvent::Up);
        state.apply_player(ButtonEvent::Up);
        assert_eq!(state.menu, Some(PlayerMenuItem::ALL.len() - 1));
        state.apply_player(ButtonEvent::Select);
        assert_eq!(state.take_request(), Some(PlayerRequest::Stop));
        assert_eq!(state.menu, None);
        assert!(!state.close_menu());

        // A stopped title plays again from its position.
        state.now_playing.state = PlayerState::Stopped;
        state.apply_player(ButtonEvent::Select);
        assert_eq!(
            state.take_request(),
            Some(PlayerRequest::Open { key: "A".into() })
        );
    }

    #[test]
    fn position_updates_redraw_only_now_and_then() {
        let mut state = AudiobookUiState::default();
        let mut now = NowPlaying {
            key: "A".into(),
            state: PlayerState::Playing,
            ..NowPlaying::default()
        };
        assert!(state.update_now_playing(now.clone()));
        state.note_player_drawn();
        now.position.position_ms = 1_000;
        assert!(!state.update_now_playing(now.clone()));
        assert_eq!(state.positions.get("A").unwrap().position_ms, 1_000);
        now.position.position_ms = PLAYER_POSITION_REDRAW_MS;
        assert!(state.update_now_playing(now.clone()));
        now.state = PlayerState::Paused;
        assert!(state.update_now_playing(now));
    }
}
