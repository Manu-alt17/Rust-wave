//! The first-run pages: which one is showing, which row is selected and
//! what a key does there. No drawing (see `screens::setup`) and no file
//! work (see `crate::first_run`).
//!
//! The language comes first, then five numbered pages. None is mandatory:
//! on every page the first row is what the page is for and the last row
//! moves on without doing it, and BOOT goes back one page.

use crate::{buttons::ButtonEvent, first_run::SetupProgress, regional::Locale};

/// Numbered pages, the language page not counted.
pub const SETUP_NUMBERED_PAGES: usize = 5;

/// One page of the first-run setup.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SetupPage {
    /// Language, shown in both languages.
    #[default]
    Language,
    /// What the keys do.
    Keys,
    /// Connect a Wi-Fi network from the phone.
    Wifi,
    /// Check the date, the time and the time zone.
    Clock,
    /// How to add books.
    Book,
    /// Where to find these pages again.
    Done,
}

impl SetupPage {
    /// Position kept in the card's marker file.
    #[must_use]
    pub const fn index(self) -> u8 {
        match self {
            Self::Language => 0,
            Self::Keys => 1,
            Self::Wifi => 2,
            Self::Clock => 3,
            Self::Book => 4,
            Self::Done => 5,
        }
    }

    /// Page for a saved position; one this firmware does not know starts
    /// over from the language.
    #[must_use]
    pub const fn from_index(index: u8) -> Self {
        match index {
            1 => Self::Keys,
            2 => Self::Wifi,
            3 => Self::Clock,
            4 => Self::Book,
            5 => Self::Done,
            _ => Self::Language,
        }
    }

    /// Number shown in the footer, `None` for the language page.
    #[must_use]
    pub const fn number(self) -> Option<usize> {
        match self {
            Self::Language => None,
            other => Some(other.index() as usize),
        }
    }

    /// Selectable rows on the page.
    #[must_use]
    pub const fn row_count(self) -> usize {
        match self {
            Self::Language | Self::Keys | Self::Wifi | Self::Clock => 2,
            Self::Book => 3,
            Self::Done => 1,
        }
    }

    const fn previous(self) -> Option<Self> {
        match self {
            Self::Language => None,
            Self::Keys => Some(Self::Language),
            Self::Wifi => Some(Self::Keys),
            Self::Clock => Some(Self::Wifi),
            Self::Book => Some(Self::Clock),
            Self::Done => Some(Self::Book),
        }
    }
}

/// What the rest of the firmware has to do after a key on a setup page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupOutcome {
    /// Nothing outside these pages.
    None,
    /// Use this language from now on.
    LanguageChosen(Locale),
    /// Open the phone portal; closing it comes back here.
    OpenWifiPortal,
    /// Open the date and time editor; leaving it comes back here.
    OpenClockEditor,
    /// Open "Connect to PC"; it ends in a restart.
    OpenUsbDisk,
    /// The pages are over: go Home.
    Finished,
    /// Pages reopened from Settings and left with BOOT.
    LeftToSettings,
}

/// Position and selection of the first-run pages.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SetupUiState {
    pub page: SetupPage,
    pub selected: usize,
    /// Whether the pages are running: screens opened from them come back
    /// to them instead of to their usual parent.
    pub active: bool,
    /// Reopened from Settings rather than shown on a new card: nothing is
    /// written to the card's marker, and BOOT on the first page leaves.
    pub from_settings: bool,
    /// An upload was opened from the book page, so its last row reads as
    /// "next" rather than "later".
    pub upload_started: bool,
    /// Page BOOT returns to from the last one: the page that led there.
    done_back: Option<SetupPage>,
    progress_request: Option<SetupProgress>,
    guide_request: bool,
}

impl SetupUiState {
    /// Show the pages for a new card, from the saved page.
    pub fn start_first_run(&mut self, page_index: u8, wifi_ready: bool) {
        *self = Self {
            active: true,
            ..Self::default()
        };
        self.go(SetupPage::from_index(page_index), wifi_ready);
        if self.page == SetupPage::Language {
            // English first: it is the language more people can read.
            self.selected = 0;
        }
    }

    /// Show the pages again from Settings, the current language selected.
    pub fn start_from_settings(&mut self, locale: Locale) {
        *self = Self {
            active: true,
            from_settings: true,
            selected: language_row(locale),
            ..Self::default()
        };
    }

    /// Row selected when a page opens: the first, except on the Wi-Fi page
    /// of a device that already has a network.
    fn go(&mut self, page: SetupPage, wifi_ready: bool) {
        self.page = page;
        self.selected = usize::from(page == SetupPage::Wifi && wifi_ready);
        if !self.from_settings {
            self.progress_request = Some(SetupProgress::Page(page.index()));
        }
    }

    fn finish(&mut self) -> SetupOutcome {
        if !self.from_settings {
            self.progress_request = Some(SetupProgress::Done);
        }
        self.active = false;
        SetupOutcome::Finished
    }

    /// Apply the rocker or SELECT on the current page.
    pub fn apply(&mut self, event: ButtonEvent, wifi_ready: bool) -> SetupOutcome {
        let rows = self.page.row_count();
        match event {
            ButtonEvent::Up => {
                self.selected = self.selected.checked_sub(1).unwrap_or(rows - 1);
                SetupOutcome::None
            }
            ButtonEvent::Down => {
                self.selected = (self.selected + 1) % rows;
                SetupOutcome::None
            }
            ButtonEvent::Select => self.select(wifi_ready),
        }
    }

    fn select(&mut self, wifi_ready: bool) -> SetupOutcome {
        let last = self.selected + 1 >= self.page.row_count();
        match self.page {
            SetupPage::Language => {
                let locale = if self.selected == 0 {
                    Locale::English
                } else {
                    Locale::Italian
                };
                self.guide_request = !self.from_settings;
                self.go(SetupPage::Keys, wifi_ready);
                SetupOutcome::LanguageChosen(locale)
            }
            SetupPage::Keys => {
                if last {
                    // "Skip the setup": straight to the page that says where
                    // to find it again.
                    self.done_back = Some(SetupPage::Keys);
                    self.go(SetupPage::Done, wifi_ready);
                } else {
                    self.go(SetupPage::Wifi, wifi_ready);
                }
                SetupOutcome::None
            }
            SetupPage::Wifi => {
                if last {
                    self.go(SetupPage::Clock, wifi_ready);
                    SetupOutcome::None
                } else {
                    // Back from the portal, the cursor is on "next".
                    self.selected = 1;
                    SetupOutcome::OpenWifiPortal
                }
            }
            SetupPage::Clock => {
                if last {
                    // Back from the editor, the cursor is on "fine as it is".
                    self.selected = 0;
                    SetupOutcome::OpenClockEditor
                } else {
                    self.go(SetupPage::Book, wifi_ready);
                    SetupOutcome::None
                }
            }
            SetupPage::Book => match self.selected {
                0 => {
                    self.upload_started = true;
                    self.selected = 2;
                    SetupOutcome::OpenWifiPortal
                }
                1 => {
                    self.upload_started = true;
                    self.selected = 2;
                    // Disk mode ends in a restart: pick up at the last page.
                    if !self.from_settings {
                        self.progress_request = Some(SetupProgress::Page(SetupPage::Done.index()));
                    }
                    SetupOutcome::OpenUsbDisk
                }
                _ => {
                    self.done_back = Some(SetupPage::Book);
                    self.go(SetupPage::Done, wifi_ready);
                    SetupOutcome::None
                }
            },
            SetupPage::Done => self.finish(),
        }
    }

    /// BOOT: one page back. On the first page it leaves, when the pages
    /// were opened from Settings, and does nothing on a new card.
    pub fn back(&mut self, wifi_ready: bool) -> SetupOutcome {
        let previous = if self.page == SetupPage::Done {
            self.done_back.or(self.page.previous())
        } else {
            self.page.previous()
        };
        match previous {
            Some(page) => {
                self.go(page, wifi_ready);
                if page == SetupPage::Language {
                    self.selected = 0;
                }
                SetupOutcome::None
            }
            None if self.from_settings => {
                self.active = false;
                SetupOutcome::LeftToSettings
            }
            None => SetupOutcome::None,
        }
    }

    /// Write the current page to the card again: "Connect to PC" had saved
    /// the last page ahead of its restart, and was left without one.
    pub fn save_current_page(&mut self) {
        if !self.from_settings {
            self.progress_request = Some(SetupProgress::Page(self.page.index()));
        }
    }

    /// Select the row of `locale` on the language page, after BOOT brought
    /// the pages back to it.
    pub fn select_language(&mut self, locale: Locale) {
        if self.page == SetupPage::Language {
            self.selected = language_row(locale);
        }
    }

    /// How far the pages got, for the runtime owner in main.rs to write to
    /// the card.
    #[must_use]
    pub fn take_progress(&mut self) -> Option<SetupProgress> {
        self.progress_request.take()
    }

    /// The language was just chosen on a new card: the runtime owner in
    /// main.rs puts the quick guide among the books.
    #[must_use]
    pub fn take_guide_request(&mut self) -> bool {
        core::mem::take(&mut self.guide_request)
    }
}

const fn language_row(locale: Locale) -> usize {
    match locale {
        Locale::English => 0,
        Locale::Italian => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::{SetupOutcome, SetupPage, SetupUiState};
    use crate::{buttons::ButtonEvent, first_run::SetupProgress, regional::Locale};

    fn first_run() -> SetupUiState {
        let mut setup = SetupUiState::default();
        setup.start_first_run(0, false);
        setup
    }

    #[test]
    fn select_on_every_first_row_walks_the_pages_in_order() {
        let mut setup = first_run();
        assert_eq!(setup.page, SetupPage::Language);
        assert_eq!(setup.take_progress(), Some(SetupProgress::Page(0)));
        assert_eq!(
            setup.apply(ButtonEvent::Select, false),
            SetupOutcome::LanguageChosen(Locale::English)
        );
        assert!(setup.take_guide_request());
        assert_eq!(setup.page, SetupPage::Keys);
        assert_eq!(setup.apply(ButtonEvent::Select, false), SetupOutcome::None);
        assert_eq!(setup.page, SetupPage::Wifi);
        // First row opens the portal and stays on the page.
        assert_eq!(
            setup.apply(ButtonEvent::Select, false),
            SetupOutcome::OpenWifiPortal
        );
        assert_eq!((setup.page, setup.selected), (SetupPage::Wifi, 1));
        assert_eq!(setup.apply(ButtonEvent::Select, true), SetupOutcome::None);
        assert_eq!(setup.page, SetupPage::Clock);
        assert_eq!(setup.apply(ButtonEvent::Select, true), SetupOutcome::None);
        assert_eq!(setup.page, SetupPage::Book);
        assert_eq!(setup.take_progress(), Some(SetupProgress::Page(4)));
        setup.apply(ButtonEvent::Up, true);
        assert_eq!(setup.selected, 2);
        assert_eq!(setup.apply(ButtonEvent::Select, true), SetupOutcome::None);
        assert_eq!(setup.page, SetupPage::Done);
        assert_eq!(
            setup.apply(ButtonEvent::Select, true),
            SetupOutcome::Finished
        );
        assert!(!setup.active);
        assert_eq!(setup.take_progress(), Some(SetupProgress::Done));
    }

    #[test]
    fn italian_is_one_step_away_and_boot_goes_back_a_page() {
        let mut setup = first_run();
        setup.apply(ButtonEvent::Down, false);
        assert_eq!(
            setup.apply(ButtonEvent::Select, false),
            SetupOutcome::LanguageChosen(Locale::Italian)
        );
        assert_eq!(setup.back(false), SetupOutcome::None);
        assert_eq!(setup.page, SetupPage::Language);
        // Nowhere to go back to on a new card: the page stays.
        assert_eq!(setup.back(false), SetupOutcome::None);
        assert!(setup.active);
    }

    #[test]
    fn skipping_from_the_keys_lands_on_the_last_page_and_boot_returns() {
        let mut setup = first_run();
        setup.apply(ButtonEvent::Select, false);
        setup.apply(ButtonEvent::Down, false);
        setup.apply(ButtonEvent::Select, false);
        assert_eq!(setup.page, SetupPage::Done);
        setup.back(false);
        assert_eq!(setup.page, SetupPage::Keys);
    }

    #[test]
    fn a_device_with_a_network_opens_the_wifi_page_on_next() {
        let mut setup = first_run();
        setup.apply(ButtonEvent::Select, true);
        setup.apply(ButtonEvent::Select, true);
        assert_eq!((setup.page, setup.selected), (SetupPage::Wifi, 1));
    }

    #[test]
    fn the_usb_cable_saves_the_last_page_before_the_restart() {
        let mut setup = SetupUiState::default();
        setup.start_first_run(SetupPage::Book.index(), true);
        let _ = setup.take_progress();
        setup.apply(ButtonEvent::Down, true);
        assert_eq!(
            setup.apply(ButtonEvent::Select, true),
            SetupOutcome::OpenUsbDisk
        );
        assert_eq!(setup.take_progress(), Some(SetupProgress::Page(5)));
        assert!(setup.upload_started);
    }

    #[test]
    fn reopened_from_settings_it_writes_nothing_and_boot_leaves() {
        let mut setup = SetupUiState::default();
        setup.start_from_settings(Locale::Italian);
        assert_eq!((setup.page, setup.selected), (SetupPage::Language, 1));
        assert_eq!(
            setup.apply(ButtonEvent::Select, true),
            SetupOutcome::LanguageChosen(Locale::Italian)
        );
        assert!(!setup.take_guide_request());
        assert_eq!(setup.take_progress(), None);
        setup.back(true);
        assert_eq!(setup.back(true), SetupOutcome::LeftToSettings);
        assert!(!setup.active);
    }

    #[test]
    fn an_unknown_saved_page_starts_over() {
        assert_eq!(SetupPage::from_index(9), SetupPage::Language);
        for page in [
            SetupPage::Language,
            SetupPage::Keys,
            SetupPage::Wifi,
            SetupPage::Clock,
            SetupPage::Book,
            SetupPage::Done,
        ] {
            assert_eq!(SetupPage::from_index(page.index()), page);
        }
    }
}
