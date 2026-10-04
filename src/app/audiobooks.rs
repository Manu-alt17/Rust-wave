//! State behind the Audiobooks screens: the library list, the player and
//! its menu. Hardware-free: the main loop scans the card, runs the audio
//! engine and feeds the results back in, and takes the player's requests
//! out with [`AudiobookUiState::take_request`].

use std::time::{Duration, Instant};

use crate::{
    audiobook::{
        Audiobook, AudiobookPositions, AudiobookTrack, ListeningPosition, NowPlaying,
        PlayerRequest, PlayerState,
    },
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
    /// Open the list of tracks, to start one directly.
    Tracks,
    /// Cycle the timer that stops playback by itself.
    SleepTimer,
    Stop,
}

/// Minutes the sleep timer offers, in the order SELECT cycles them; 0 is
/// off.
pub const SLEEP_TIMER_MINUTES: [u16; 5] = [0, 15, 30, 45, 60];

impl PlayerMenuItem {
    pub const ALL: [Self; 7] = [
        Self::Back30,
        Self::Forward30,
        Self::PreviousTrack,
        Self::NextTrack,
        Self::Tracks,
        Self::SleepTimer,
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
            (Self::Tracks, Locale::English) => "Tracks",
            (Self::Tracks, Locale::Italian) => "Tracce",
            (Self::SleepTimer, Locale::English) => "Sleep timer",
            (Self::SleepTimer, Locale::Italian) => "Timer di spegnimento",
            (Self::Stop, Locale::English) => "Stop",
            (Self::Stop, Locale::Italian) => "Ferma",
        }
    }

    /// What the item asks of the audio engine; `None` for the ones the
    /// screen handles itself.
    const fn request(self) -> Option<PlayerRequest> {
        match self {
            Self::Back30 => Some(PlayerRequest::SeekBy {
                seconds: -PLAYER_SKIP_SECONDS,
            }),
            Self::Forward30 => Some(PlayerRequest::SeekBy {
                seconds: PLAYER_SKIP_SECONDS,
            }),
            Self::PreviousTrack => Some(PlayerRequest::SkipTrack { forward: false }),
            Self::NextTrack => Some(PlayerRequest::SkipTrack { forward: true }),
            Self::Stop => Some(PlayerRequest::Stop),
            Self::Tracks | Self::SleepTimer => None,
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
    /// Selected row of the track list while it is open from the menu.
    pub track_list: Option<usize>,
    /// Minutes the sleep timer was set to; 0 while it is off.
    pub sleep_timer_minutes: u16,
    sleep_timer_deadline: Option<Instant>,
    /// Whole minutes the sleep timer has left, rounded up, as of the last
    /// [`Self::tick_sleep_timer`].
    pub sleep_timer_remaining: Option<u16>,
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
    /// menu open, Up/Down move through it and SELECT runs the entry; with
    /// the track list open, SELECT starts the track under the cursor.
    pub fn apply_player(&mut self, event: ButtonEvent) {
        if let Some(selected) = self.track_list {
            let count = self.playing_tracks().len();
            match event {
                _ if count == 0 => self.track_list = None,
                ButtonEvent::Up => {
                    self.track_list = Some(selected.checked_sub(1).unwrap_or(count - 1));
                }
                ButtonEvent::Down => self.track_list = Some((selected + 1) % count),
                ButtonEvent::Select => {
                    self.play_track(selected.min(count - 1));
                    self.track_list = None;
                    self.menu = None;
                }
            }
            return;
        }
        if let Some(selected) = self.menu {
            let count = PlayerMenuItem::ALL.len();
            match event {
                ButtonEvent::Up => self.menu = Some(selected.checked_sub(1).unwrap_or(count - 1)),
                ButtonEvent::Down => self.menu = Some((selected + 1) % count),
                ButtonEvent::Select => match PlayerMenuItem::ALL[selected] {
                    PlayerMenuItem::Tracks => {
                        if !self.playing_tracks().is_empty() {
                            self.track_list = Some(self.now_playing.track);
                        }
                    }
                    PlayerMenuItem::SleepTimer => self.cycle_sleep_timer(Instant::now()),
                    item => {
                        self.request = item.request();
                        if item == PlayerMenuItem::Stop {
                            self.menu = None;
                            self.clear_sleep_timer();
                        }
                    }
                },
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
        self.track_list = None;
    }

    /// BOOT steps back one level: from the track list to the menu, from the
    /// menu to the player. `false` when neither was open.
    pub fn close_menu(&mut self) -> bool {
        if self.track_list.take().is_some() {
            return true;
        }
        self.menu.take().is_some()
    }

    /// Tracks of the loaded title, for the track list. Empty when no title
    /// is loaded or the library no longer has it.
    #[must_use]
    pub fn playing_tracks(&self) -> &[AudiobookTrack] {
        self.book(&self.now_playing.key)
            .map_or(&[], |book| book.tracks.as_slice())
    }

    /// Start track `index` of the loaded title from its beginning: the
    /// title is opened again at that position.
    fn play_track(&mut self, index: usize) {
        if !self.now_playing.is_loaded() {
            return;
        }
        let key = self.now_playing.key.clone();
        self.positions.set(
            &key,
            ListeningPosition {
                track: index,
                byte_offset: 0,
                position_ms: 0,
            },
        );
        self.now_playing.state = PlayerState::Loading;
        self.now_playing.error = None;
        self.request = Some(PlayerRequest::Open { key });
    }

    /// Move the sleep timer to its next setting, counted from `now`.
    pub fn cycle_sleep_timer(&mut self, now: Instant) {
        let current = SLEEP_TIMER_MINUTES
            .iter()
            .position(|minutes| *minutes == self.sleep_timer_minutes)
            .unwrap_or(0);
        let minutes = SLEEP_TIMER_MINUTES[(current + 1) % SLEEP_TIMER_MINUTES.len()];
        if minutes == 0 {
            self.clear_sleep_timer();
        } else {
            self.sleep_timer_minutes = minutes;
            self.sleep_timer_deadline = Some(now + Duration::from_secs(u64::from(minutes) * 60));
            self.sleep_timer_remaining = Some(minutes);
        }
    }

    fn clear_sleep_timer(&mut self) {
        self.sleep_timer_minutes = 0;
        self.sleep_timer_deadline = None;
        self.sleep_timer_remaining = None;
    }

    /// Advance the sleep timer to `now`. When it is up, playback is asked
    /// to stop (take the request with [`Self::take_request`]) and the timer
    /// turns off. Returns whether what the player screen shows changed: the
    /// minutes left, or the timer ending.
    pub fn tick_sleep_timer(&mut self, now: Instant) -> bool {
        let Some(deadline) = self.sleep_timer_deadline else {
            return false;
        };
        if now >= deadline {
            self.clear_sleep_timer();
            if matches!(
                self.now_playing.state,
                PlayerState::Loading | PlayerState::Playing | PlayerState::Paused
            ) {
                self.request = Some(PlayerRequest::Stop);
            }
            return true;
        }
        let seconds = deadline.duration_since(now).as_secs();
        let remaining = Some(seconds.div_ceil(60).min(u64::from(u16::MAX)) as u16);
        if remaining == self.sleep_timer_remaining {
            return false;
        }
        self.sleep_timer_remaining = remaining;
        true
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
    fn the_track_list_starts_the_chosen_track_from_its_beginning() {
        let mut state = AudiobookUiState::default();
        state.set_library(Ok(vec![book("A", 3)]));
        state.now_playing = NowPlaying {
            key: "A".into(),
            track: 1,
            track_count: 3,
            state: PlayerState::Playing,
            ..NowPlaying::default()
        };
        state.open_menu();
        state.menu = PlayerMenuItem::ALL
            .iter()
            .position(|item| *item == PlayerMenuItem::Tracks);
        state.apply_player(ButtonEvent::Select);
        // Opens on the track being played; nothing is asked of the engine.
        assert_eq!(state.track_list, Some(1));
        assert_eq!(state.take_request(), None);
        state.apply_player(ButtonEvent::Down);
        state.apply_player(ButtonEvent::Down);
        assert_eq!(state.track_list, Some(0));
        state.apply_player(ButtonEvent::Up);
        assert_eq!(state.track_list, Some(2));
        // BOOT steps back to the menu, then to the player.
        assert!(state.close_menu());
        assert_eq!(state.track_list, None);
        assert!(state.menu.is_some());
        state.apply_player(ButtonEvent::Select);
        state.apply_player(ButtonEvent::Down);
        assert_eq!(state.track_list, Some(2));
        state.apply_player(ButtonEvent::Select);
        assert_eq!(
            state.take_request(),
            Some(PlayerRequest::Open { key: "A".into() })
        );
        assert_eq!(
            state.positions.get("A"),
            Some(ListeningPosition {
                track: 2,
                byte_offset: 0,
                position_ms: 0
            })
        );
        assert_eq!(state.track_list, None);
        assert_eq!(state.menu, None);
    }

    #[test]
    fn the_sleep_timer_cycles_counts_down_and_stops_playback() {
        use std::time::{Duration, Instant};

        let mut state = AudiobookUiState::default();
        state.now_playing = NowPlaying {
            key: "A".into(),
            state: PlayerState::Playing,
            ..NowPlaying::default()
        };
        let start = Instant::now();
        assert!(!state.tick_sleep_timer(start));
        state.cycle_sleep_timer(start);
        assert_eq!(state.sleep_timer_minutes, 15);
        assert_eq!(state.sleep_timer_remaining, Some(15));
        // Nothing to redraw until a minute has gone.
        assert!(!state.tick_sleep_timer(start + Duration::from_secs(30)));
        assert!(state.tick_sleep_timer(start + Duration::from_secs(61)));
        assert_eq!(state.sleep_timer_remaining, Some(14));
        assert_eq!(state.take_request(), None);
        // When it is up, playback stops and the timer is off again.
        assert!(state.tick_sleep_timer(start + Duration::from_secs(15 * 60)));
        assert_eq!(state.take_request(), Some(PlayerRequest::Stop));
        assert_eq!(state.sleep_timer_minutes, 0);
        assert_eq!(state.sleep_timer_remaining, None);
        assert!(!state.tick_sleep_timer(start + Duration::from_secs(16 * 60)));

        // SELECT goes through every setting and back to off.
        for expected in [15, 30, 45, 60, 0] {
            state.cycle_sleep_timer(start);
            assert_eq!(state.sleep_timer_minutes, expected);
        }
        // Stopping by hand turns the timer off too.
        state.cycle_sleep_timer(start);
        state.open_menu();
        state.menu = Some(PlayerMenuItem::ALL.len() - 1);
        state.apply_player(ButtonEvent::Select);
        assert_eq!(state.take_request(), Some(PlayerRequest::Stop));
        assert_eq!(state.sleep_timer_minutes, 0);
        // A timer that ends with nothing playing asks for nothing.
        state.now_playing.state = PlayerState::Stopped;
        state.cycle_sleep_timer(start);
        assert!(state.tick_sleep_timer(start + Duration::from_secs(15 * 60)));
        assert_eq!(state.take_request(), None);
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
