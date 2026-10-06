//! Layout audit: renders every screen in many states, both languages and
//! all UI sizes, records every string drawn and fails when text leaves the
//! screen, runs into the footer or lands on other text.
//!
//! The scenarios use deliberately awkward data (long titles, long network
//! names, long error messages), which is where layouts break first. A new
//! screen or state belongs in [`scenarios`].
//!
//! Set `UX_AUDIT_OUT` to a directory to also get a PNG of every Italian
//! screen per UI size, the findings (`report.tsv`) and every string shown
//! (`strings.tsv`), for a visual review:
//!
//! ```text
//! UX_AUDIT_OUT=/tmp/ux cargo +stable test --target x86_64-unknown-linux-gnu --lib layout_audit
//! ```

use std::{collections::BTreeMap, fmt::Write as _, path::PathBuf};

use embedded_graphics::prelude::Point;

use super::{
    display::{AutoSleep, SleepScreenMode, UiFontSize},
    render_current_screen,
    setup::SetupPage,
    typography::audit::{self, Rec},
    AppState, ScreenRoute,
};
use crate::{
    audiobook::{Audiobook, AudiobookTrack, ListeningPosition, NowPlaying, PlayerState},
    framebuffer::FrameBuffer,
    network::WifiConnectionState,
    network_saved::SavedNetworkEntry,
    orientation::DisplayOrientation,
    ota::{InstallProgress, OtaCheckState},
    reader::{
        BookFormat, ReaderBook, ReaderCachedPage, ReaderDictionaryMode, ReaderLoadingStage,
        ReaderLocation, ReaderPageLine, ReaderPreferences, ReaderSession, ReadingPreference,
        TextEncoding,
    },
    regional::Locale,
    storage::{FilePreview, StorageEntry, StorageEntryKind, StorageNotice},
    usb_disk::UsbDiskPhase,
    wifi_transfer::{JoinAttemptState, WifiTransferSnapshot, WifiTransferState},
};

type Arrange = Box<dyn Fn(&mut AppState)>;

const LONG_TITLE: &str = "Il meraviglioso viaggio di Nils Holgersson attraverso la Svezia";
const LONG_ERROR: &str =
    "esp-idf error ESP_ERR_TIMEOUT (0x107): operation timed out while reading sector 4096";

fn book(path: &str, title: &str) -> ReaderBook {
    ReaderBook {
        path: path.into(),
        title: title.into(),
        format: BookFormat::Epub,
        size_bytes: 900_000,
        modified_seconds: 0,
    }
}

fn location(path: &str, title: &str, percent: u8, page: usize) -> ReaderLocation {
    ReaderLocation {
        path: path.into(),
        title: title.into(),
        format: BookFormat::Epub,
        size_bytes: 900_000,
        modified_seconds: 0,
        page_index: page,
        byte_offset: 9_000 * u64::from(percent),
        epub_chapter: None,
        reading_percent: Some(percent),
    }
}

fn page_lines() -> Vec<ReaderPageLine> {
    [
        ("In my younger and more", false),
        ("vulnerable years my father", false),
        ("gave me some advice that I've", false),
        ("been turning over in my mind", false),
        ("ever since.", true),
        ("\u{201c}Whenever you feel like", false),
        ("criticizing any one,\u{201d} he told", false),
        ("me, \u{201c}just remember that all", false),
        ("the people in this world", false),
        ("haven't had the advantages", false),
        ("that you've had.\u{201d}", true),
    ]
    .into_iter()
    .map(|(text, paragraph_end)| ReaderPageLine {
        text: text.into(),
        paragraph_end,
        image: None,
    })
    .collect()
}

fn session(title: &str, epub: bool) -> ReaderSession {
    let epub_document = epub.then(|| {
        let toc = (0..14)
            .map(|index| crate::epub::EpubTocEntry {
                label: if index % 3 == 0 {
                    format!(
                        "Capitolo {} - Dove si racconta di un lungo viaggio",
                        index + 1
                    )
                } else {
                    format!("Capitolo {}", index + 1)
                },
                text_offset: index as u64 * 1000,
                spine_index: index,
            })
            .collect();
        let chapters = (0..14)
            .map(|index| crate::epub::EpubChapter {
                number: index + 1,
                label: format!("Capitolo {}", index + 1),
                text_offset: index as u64 * 1000,
                text_end_offset: (index as u64 + 1) * 1000,
                spine_index: index,
            })
            .collect();
        crate::epub::EpubDocument::from_resident_for_test(
            title.into(),
            "x".repeat(14_000),
            toc,
            chapters,
            Vec::new(),
            14,
        )
    });
    ReaderSession {
        book: ReaderBook {
            path: "BOOK.EPUB".into(),
            title: title.into(),
            format: if epub {
                BookFormat::Epub
            } else {
                BookFormat::Text
            },
            size_bytes: 300_000,
            modified_seconds: 0,
        },
        encoding: TextEncoding::Utf8,
        epub_document,
        layout: ReaderPreferences::default().layout(),
        current_page: 66,
        page_number_base: 0,
        page_offsets: vec![0; 340],
        indexed_through: 300_000,
        index_complete: true,
        cache: vec![ReaderCachedPage {
            page_index: 66,
            byte_offset: 11_500,
            next_byte_offset: 11_900,
            lines: page_lines(),
        }],
        epub_chapter_pages: Vec::new(),
        epub_pending_chapter: None,
        epub_document_cache_pending: false,
    }
}

fn audiobooks(state: &mut AppState, count: usize, long: bool) {
    let make = |index: usize| {
        let key = if long && index % 2 == 0 {
            format!("{LONG_TITLE} {index}")
        } else {
            format!("Audiolibro {index}")
        };
        Audiobook {
            key: key.clone(),
            title: key,
            tracks: (1..=12)
                .map(|track| AudiobookTrack {
                    path: format!("/sdcard/RUSTMIX/AUDIO/x/{track:02}.mp3").into(),
                    title: format!(
                        "{track:02} - Capitolo {track} con un titolo piuttosto lungo davvero"
                    ),
                    size_bytes: 20_000_000,
                })
                .collect(),
        }
    };
    let books: Vec<Audiobook> = (0..count).map(make).collect();
    let position = ListeningPosition {
        track: 4,
        byte_offset: 9_000_000,
        position_ms: 1_325_000,
    };
    if let Some(first) = books.first() {
        state.audiobooks.positions.set(&first.key, position);
        state.audiobooks.now_playing = NowPlaying {
            key: first.key.clone(),
            title: first.title.clone(),
            track: 4,
            track_count: 12,
            track_title: "05 - Capitolo 5 con un titolo piuttosto lungo davvero".into(),
            position,
            duration_ms: 2_760_000,
            state: PlayerState::Playing,
            error: None,
        };
    }
    state.audiobooks.set_library(Ok(books));
    state.audio.volume_percent = 60;
}

fn files(state: &mut AppState, long: bool) {
    let mut entries = vec![StorageEntry {
        name: "RUSTMIX".into(),
        kind: StorageEntryKind::Directory,
        size_bytes: None,
    }];
    for index in 0..6 {
        entries.push(StorageEntry {
            name: if long {
                format!("Un nome di file davvero molto lungo numero {index} versione finale.epub")
            } else {
                format!("LIBRO{index}.EPUB")
            },
            kind: StorageEntryKind::File,
            size_bytes: Some(123_456_789),
        });
    }
    state.storage.mounted = true;
    state.storage.scan.retained_entries = entries.len();
    state.storage.entries = entries;
    state.storage.selected = 2;
    state.router.navigate_to(ScreenRoute::Files);
}

fn scenarios() -> Vec<(String, Arrange)> {
    let mut list: Vec<(String, Arrange)> = Vec::new();
    let mut add = |name: &str, arrange: Arrange| list.push((name.to_string(), arrange));

    add("home-empty", Box::new(|_| {}));
    add(
        "home-with-book",
        Box::new(|state| {
            state.reader.resume = Some(location("WIND.EPUB", "Il nome del vento", 62, 120));
            state.update_reading_stats_snapshot(crate::reading_stats::ReadingStatsSnapshot {
                available: true,
                today_seconds: 1_380,
                streak_days: 5,
                remaining_book_seconds: Some(12_000),
                ..Default::default()
            });
        }),
    );
    add(
        "home-long-title-big-numbers",
        Box::new(|state| {
            state.reader.resume = Some(location("WIND.EPUB", LONG_TITLE, 62, 120));
            state.update_reading_stats_snapshot(crate::reading_stats::ReadingStatsSnapshot {
                available: true,
                today_seconds: 39_540,
                streak_days: 365,
                remaining_book_seconds: Some(120_000),
                ..Default::default()
            });
            state.home_selected = 0;
        }),
    );
    add(
        "settings",
        Box::new(|state| state.router.navigate_to(ScreenRoute::Settings)),
    );

    // --- Library -----------------------------------------------------------
    add(
        "library-empty",
        Box::new(|state| state.router.navigate_to(ScreenRoute::Library)),
    );
    add(
        "library-error",
        Box::new(|state| {
            state.reader.library_error = Some(LONG_ERROR.into());
            state.router.navigate_to(ScreenRoute::Library);
        }),
    );
    add(
        "library-books",
        Box::new(|state| {
            state.router.navigate_to(ScreenRoute::Library);
            state.reader.books = vec![
                book("A.EPUB", "Mondo Emerso"),
                book("B.EPUB", LONG_TITLE),
                book("C.EPUB", "Mondo Emerso Vol. 2"),
                book("D.EPUB", "Guida Galattica"),
                book("E.EPUB", "Un Altro Libro Mai Aperto"),
            ];
            state.reader.recent = vec![
                location("A.EPUB", "Mondo Emerso", 17, 10),
                location("B.EPUB", LONG_TITLE, 30, 10),
                location("C.EPUB", "Mondo Emerso Vol. 2", 100, 10),
            ];
        }),
    );
    add(
        "library-book-actions",
        Box::new(|state| {
            state.reader.open_book_actions(book("B.EPUB", LONG_TITLE));
            state.router.navigate_to(ScreenRoute::LibraryBookActions);
        }),
    );
    add(
        "library-book-actions-armed",
        Box::new(|state| {
            state.reader.open_book_actions(book("B.EPUB", LONG_TITLE));
            state.reader.recent = vec![location("B.EPUB", LONG_TITLE, 37, 10)];
            state.reader.positions = state.reader.recent.clone();
            state.reader.book_actions_selected = 3;
            state.reader.book_delete_armed = true;
            state.router.navigate_to(ScreenRoute::LibraryBookActions);
        }),
    );
    add(
        "library-book-actions-error",
        Box::new(|state| {
            state.reader.open_book_actions(book("B.EPUB", LONG_TITLE));
            state.reader.book_actions_selected = 3;
            state.reader.book_actions_error = Some(LONG_ERROR.into());
            state.router.navigate_to(ScreenRoute::LibraryBookActions);
        }),
    );
    add(
        "library-book-bookmarks-empty",
        Box::new(|state| {
            state.reader.open_book_actions(book("B.EPUB", LONG_TITLE));
            state.router.navigate_to(ScreenRoute::LibraryBookBookmarks);
        }),
    );
    add(
        "library-book-bookmarks-12",
        Box::new(|state| {
            state.reader.open_book_actions(book("B.EPUB", LONG_TITLE));
            state.reader.bookmarks = (0..12)
                .map(|index| location("B.EPUB", LONG_TITLE, (index * 8) as u8, 1000 + index * 37))
                .collect();
            state.reader.book_bookmarks_selected = 10;
            state.router.navigate_to(ScreenRoute::LibraryBookBookmarks);
        }),
    );

    // --- Reader ------------------------------------------------------------
    add(
        "continue-none",
        Box::new(|state| state.router.navigate_to(ScreenRoute::ContinueReading)),
    );
    add(
        "continue-resume",
        Box::new(|state| {
            state.reader.resume = Some(location("B.EPUB", LONG_TITLE, 62, 12_345));
            state.router.navigate_to(ScreenRoute::ContinueReading);
        }),
    );
    for (name, stage) in [
        ("opening", ReaderLoadingStage::OpeningFile),
        ("saved-position", ReaderLoadingStage::LoadingSavedPosition),
        ("nearby", ReaderLoadingStage::IndexingNearbyPages),
        ("failed", ReaderLoadingStage::Failed),
    ] {
        add(
            &format!("reader-loading-{name}"),
            Box::new(move |state| {
                state.reader.loading = Some(crate::reader::PendingReaderOpen {
                    book: book("B.EPUB", LONG_TITLE),
                    stage,
                    encoding: None,
                    epub_document: None,
                    resume: None,
                    message: if stage == ReaderLoadingStage::Failed {
                        LONG_ERROR.into()
                    } else {
                        "Reading book metadata from the SD card".into()
                    },
                    epub_document_cache_pending: false,
                });
                state.router.navigate_to(ScreenRoute::ReaderLoading);
            }),
        );
    }
    add(
        "reader-page-txt",
        Box::new(|state| {
            state.reader.session = Some(session("The Great Gatsby", false));
            state.router.navigate_to(ScreenRoute::ReaderPage);
        }),
    );
    add(
        "reader-page-epub",
        Box::new(|state| {
            state.reader.session = Some(session("Il nome del vento", true));
            state.router.navigate_to(ScreenRoute::ReaderPage);
        }),
    );
    add(
        "reader-page-bookmarked",
        Box::new(|state| {
            let session = session("Il nome del vento", true);
            state.reader.bookmarks = vec![session.current_location()];
            state.reader.session = Some(session);
            state.router.navigate_to(ScreenRoute::ReaderPage);
        }),
    );
    add(
        "reader-page-full-screen-bookmarked",
        Box::new(|state| {
            let mut session = session("Il nome del vento", true);
            session.layout.full_screen = true;
            state.reader.bookmarks = vec![session.current_location()];
            state.reader.session = Some(session);
            state.router.navigate_to(ScreenRoute::ReaderPage);
        }),
    );
    add(
        "reader-page-landscape",
        Box::new(|state| {
            state.reader.session = Some(session("Il nome del vento", true));
            state.orientation = DisplayOrientation::Landscape;
            state.router.navigate_to(ScreenRoute::ReaderPage);
        }),
    );
    add(
        "reader-dictionary-line",
        Box::new(|state| {
            state.reader.session = Some(session("Il nome del vento", true));
            state.reader.dictionary_mode = ReaderDictionaryMode::LineSelect { line_index: 2 };
            state.router.navigate_to(ScreenRoute::ReaderPage);
        }),
    );
    add(
        "reader-dictionary-word",
        Box::new(|state| {
            state.reader.session = Some(session("Il nome del vento", true));
            state.reader.dictionary_mode = ReaderDictionaryMode::WordSelect {
                line_index: 2,
                word_index: 1,
            };
            state.router.navigate_to(ScreenRoute::ReaderPage);
        }),
    );
    add(
        "reader-dictionary-definition",
        Box::new(|state| {
            state.reader.session = Some(session("Il nome del vento", true));
            state.reader.dictionary_mode = ReaderDictionaryMode::Definition {
                line_index: 2,
                word_index: 1,
                word: "advice".into(),
                message: "s. m. [dal lat. consilium]. - 1. Suggerimento che si d\u{00E0} a una persona perch\u{00E9} si comporti in un determinato modo o prenda una determinata decisione: dare, chiedere, ricevere un consiglio; seguire i consigli di un amico.".into(),
            };
            state.router.navigate_to(ScreenRoute::ReaderPage);
        }),
    );
    add(
        "reader-dictionary-definition-very-long",
        Box::new(|state| {
            state.reader.session = Some(session("Il nome del vento", true));
            state.reader.dictionary_mode = ReaderDictionaryMode::Definition {
                line_index: 2,
                word_index: 1,
                word: "advice".into(),
                message: "Suggerimento che si d\u{00E0} a una persona perch\u{00E9} si comporti in un determinato modo. ".repeat(40),
            };
            state.router.navigate_to(ScreenRoute::ReaderPage);
        }),
    );
    add(
        "reader-options",
        Box::new(|state| {
            state.reader.session = Some(session("Il nome del vento", true));
            state.router.navigate_to(ScreenRoute::ReaderOptions);
        }),
    );
    for (name, percent) in [("start", 0_u8), ("middle", 45), ("end", 100)] {
        add(
            &format!("reader-goto-{name}"),
            Box::new(move |state| {
                state.reader.session = Some(session("Il nome del vento", true));
                state.reader.begin_goto();
                state.reader.goto_percent = percent;
                state.router.navigate_to(ScreenRoute::ReaderGoTo);
            }),
        );
    }
    add(
        "reader-goto-txt",
        Box::new(|state| {
            state.reader.session = Some(session("The Great Gatsby", false));
            state.reader.begin_goto();
            state.router.navigate_to(ScreenRoute::ReaderGoTo);
        }),
    );
    add(
        "reader-toc",
        Box::new(|state| {
            state.reader.session = Some(session("Il nome del vento", true));
            state.reader.toc_selected = 9;
            state.router.navigate_to(ScreenRoute::ReaderToc);
        }),
    );
    add(
        "reader-toc-second-page",
        Box::new(|state| {
            state.reader.session = Some(session("Il nome del vento", true));
            state.reader.toc_selected = 12;
            state.router.navigate_to(ScreenRoute::ReaderToc);
        }),
    );
    add(
        "reader-toc-empty",
        Box::new(|state| {
            state.reader.session = Some(session("The Great Gatsby", false));
            state.router.navigate_to(ScreenRoute::ReaderToc);
        }),
    );
    add(
        "reader-bookmarks-empty",
        Box::new(|state| {
            state.reader.session = Some(session("Il nome del vento", true));
            state.router.navigate_to(ScreenRoute::ReaderBookmarks);
        }),
    );
    add(
        "reader-bookmarks-12",
        Box::new(|state| {
            state.reader.session = Some(session("Il nome del vento", true));
            state.reader.bookmarks = (0..12)
                .map(|index| {
                    // The open book's own, plus one of another book that
                    // must not show up.
                    location(
                        if index == 5 { "C.EPUB" } else { "BOOK.EPUB" },
                        "Il nome del vento",
                        (index * 8) as u8,
                        1000 + index * 37,
                    )
                })
                .collect();
            state.reader.bookmarks_selected = 10;
            state.router.navigate_to(ScreenRoute::ReaderBookmarks);
        }),
    );
    add(
        "reader-preferences-list",
        Box::new(|state| {
            state.reader.session = Some(session("Il nome del vento", true));
            state.reader.preferences.theme = crate::reader::ReadingTheme::HighContrast;
            state.reader.preferences.paragraph_alignment =
                crate::reader::ParagraphAlignment::Justified;
            state.reader.preferences.book_font = crate::reader::BookFont::AtkinsonHyperlegible;
            state.router.navigate_to(ScreenRoute::ReaderPreferences);
        }),
    );
    for (index, preference) in ReadingPreference::ALL.iter().copied().enumerate() {
        add(
            &format!(
                "reader-preference-editor-{}",
                preference.label().replace(' ', "-")
            ),
            Box::new(move |state| {
                state.reader.session = Some(session("Il nome del vento", true));
                state.reader.preferences_selected = index;
                state.reader.open_preference_editor();
                state.router.navigate_to(ScreenRoute::ReaderPreferences);
            }),
        );
    }

    // --- Statistics ---------------------------------------------------------
    add(
        "stats-unavailable",
        Box::new(|state| state.router.navigate_to(ScreenRoute::ReadingStats)),
    );
    // The week of Monday 5 October 2026 seen on its Wednesday, with long
    // days so every figure is at its widest.
    let stats = |period: crate::reading_stats::PeriodStats| -> Arrange {
        Box::new(move |state| {
            use crate::reading_stats::{BookPeriodStats, ReadingStatsSnapshot};
            state.reader.recent = vec![
                location("B.EPUB", LONG_TITLE, 62, 10),
                location("C.EPUB", "Project Hail Mary", 100, 10),
            ];
            let ids: Vec<u32> = state
                .reader
                .recent
                .iter()
                .map(|l| {
                    crate::reading_stats::book_id_for(&l.path, l.size_bytes, l.modified_seconds)
                })
                .collect();
            let mut period = period.clone();
            if !period.books.is_empty() {
                period.books = [
                    (ids[0], 82_800),
                    (ids[1], 5_040),
                    (7, 600),
                    (8, 300),
                    (9, 200),
                ]
                .into_iter()
                .map(|(book_id, total_seconds)| BookPeriodStats {
                    book_id,
                    total_seconds,
                })
                .collect();
            }
            state.stats_view = period.view;
            state.update_reading_stats_snapshot(ReadingStatsSnapshot {
                available: true,
                today_seconds: 7_800,
                sessions_today: 3,
                streak_days: 125,
                chars_per_minute: Some(1_180),
                remaining_chapter_seconds: Some(600),
                remaining_book_seconds: Some(72_000),
                period,
            });
            state.router.navigate_to(ScreenRoute::ReadingStats);
        })
    };
    {
        use crate::reading_stats::{
            BookPeriodStats, CalendarDay, PeriodStats, StatsPeriod, StatsView,
        };
        let weeks = |back| StatsView {
            period: StatsPeriod::Week,
            back,
        };
        let months = |back| StatsView {
            period: StatsPeriod::Month,
            back,
        };
        let day = |year, month, day| CalendarDay { year, month, day };
        let some_book = || {
            vec![BookPeriodStats {
                book_id: 1,
                total_seconds: 1,
            }]
        };
        add(
            "stats-week",
            stats(PeriodStats::sample(
                weeks(0),
                day(2026, 10, 5),
                &[1_800, 5_400, 2_700, 0, 0, 0, 0],
                Some(2),
                some_book(),
                true,
            )),
        );
        // Whole days of reading, across two months: the widest dates, the
        // widest totals and the tallest axis.
        add(
            "stats-week-last",
            stats(PeriodStats::sample(
                weeks(1),
                day(2026, 9, 28),
                &[72_000, 86_000, 0, 43_000, 3_600, 61_200, 50_400],
                None,
                some_book(),
                true,
            )),
        );
        add(
            "stats-week-earlier",
            stats(PeriodStats::sample(
                weeks(3),
                day(2026, 9, 14),
                &[0, 0, 240, 0, 420, 0, 0],
                None,
                some_book(),
                false,
            )),
        );
        let mut october = [0_u32; 31];
        october[..7].copy_from_slice(&[600, 0, 3_000, 4_200, 1_800, 5_400, 2_700]);
        add(
            "stats-month",
            stats(PeriodStats::sample(
                months(0),
                day(2026, 10, 1),
                &october,
                Some(6),
                some_book(),
                true,
            )),
        );
        let september: Vec<u32> = (0..30).map(|index| (index * 37 % 11) * 1_260).collect();
        add(
            "stats-month-last",
            stats(PeriodStats::sample(
                months(1),
                day(2026, 9, 1),
                &september,
                None,
                some_book(),
                true,
            )),
        );
        let february: Vec<u32> = (0..28).map(|index| (index * 53 % 7) * 2_100).collect();
        add(
            "stats-month-earlier",
            stats(PeriodStats::sample(
                months(8),
                day(2026, 2, 1),
                &february,
                None,
                some_book(),
                false,
            )),
        );
        add(
            "stats-no-books",
            stats(PeriodStats::sample(
                weeks(0),
                day(2026, 10, 5),
                &[0; 7],
                Some(2),
                Vec::new(),
                false,
            )),
        );
    }

    // --- Audiobooks -----------------------------------------------------------
    add(
        "audiobooks-empty",
        Box::new(|state| {
            state.audiobooks.set_library(Ok(Vec::new()));
            state.router.navigate_to(ScreenRoute::AudiobookLibrary);
        }),
    );
    add(
        "audiobooks-error",
        Box::new(|state| {
            state.audiobooks.set_library(Err(LONG_ERROR.into()));
            state.router.navigate_to(ScreenRoute::AudiobookLibrary);
        }),
    );
    add(
        "audiobooks-list",
        Box::new(|state| {
            audiobooks(state, 9, true);
            state.router.navigate_to(ScreenRoute::AudiobookLibrary);
        }),
    );
    add(
        "audiobook-player",
        Box::new(|state| {
            audiobooks(state, 3, true);
            state.router.navigate_to(ScreenRoute::AudiobookPlayer);
        }),
    );
    add(
        "audiobook-player-error",
        Box::new(|state| {
            audiobooks(state, 3, true);
            state.audiobooks.now_playing.state = PlayerState::Error;
            state.audiobooks.now_playing.error = Some(LONG_ERROR.into());
            state.router.navigate_to(ScreenRoute::AudiobookPlayer);
        }),
    );
    add(
        "audiobook-player-menu",
        Box::new(|state| {
            audiobooks(state, 3, false);
            state.router.navigate_to(ScreenRoute::AudiobookPlayer);
            state.audiobooks.open_menu();
        }),
    );
    add(
        "audiobook-player-menu-timer",
        Box::new(|state| {
            audiobooks(state, 3, true);
            state.router.navigate_to(ScreenRoute::AudiobookPlayer);
            state.audiobooks.open_menu();
            state.audiobooks.menu = Some(5);
            state
                .audiobooks
                .cycle_sleep_timer(std::time::Instant::now());
        }),
    );
    add(
        "audiobook-player-timer",
        Box::new(|state| {
            audiobooks(state, 3, true);
            state.router.navigate_to(ScreenRoute::AudiobookPlayer);
            for _ in 0..4 {
                state
                    .audiobooks
                    .cycle_sleep_timer(std::time::Instant::now());
            }
        }),
    );
    for (name, selected) in [("first-page", 4_usize), ("second-page", 11)] {
        add(
            &format!("audiobook-tracks-{name}"),
            Box::new(move |state| {
                audiobooks(state, 3, true);
                state.router.navigate_to(ScreenRoute::AudiobookPlayer);
                state.audiobooks.open_menu();
                state.audiobooks.track_list = Some(selected);
            }),
        );
    }

    // --- Files ----------------------------------------------------------------
    add(
        "files-not-mounted",
        Box::new(|state| state.router.navigate_to(ScreenRoute::Files)),
    );
    add(
        "files-error",
        Box::new(|state| {
            files(state, false);
            state.storage.error = Some(LONG_ERROR.into());
        }),
    );
    add(
        "files-list",
        Box::new(|state| {
            files(state, false);
            state.storage.selected = 0;
        }),
    );
    add(
        "files-file-selected",
        Box::new(|state| {
            files(state, true);
            state.storage.selected = 2;
        }),
    );
    add(
        "files-delete-pending",
        Box::new(|state| {
            files(state, true);
            state.storage.selected = 2;
            state.storage.pending_delete = Some(state.storage.entries[2].name.clone());
        }),
    );
    add(
        "files-deleted",
        Box::new(|state| {
            files(state, false);
            state.storage.at_root = false;
            state.storage.current_path = "/sdcard/RUSTMIX/BOOKS".into();
            state.storage.notice = Some(StorageNotice::Deleted(
                "Un nome di file davvero molto lungo numero 3 versione finale.epub".into(),
            ));
        }),
    );
    add(
        "files-delete-failed",
        Box::new(|state| {
            files(state, false);
            state.storage.notice = Some(StorageNotice::DeleteFailed(
                "LIBRO3.EPUB".into(),
                LONG_ERROR.into(),
            ));
        }),
    );
    add(
        "files-empty-folder",
        Box::new(|state| {
            files(state, false);
            state.storage.entries.clear();
            state.storage.scan.retained_entries = 0;
            state.storage.at_root = false;
            state.storage.current_path = "/sdcard/RUSTMIX/VUOTA".into();
        }),
    );
    add(
        "files-list-long-names",
        Box::new(|state| files(state, true)),
    );
    for (name, binary, truncated) in [
        ("text", false, false),
        ("truncated", false, true),
        ("binary", true, false),
    ] {
        add(
            &format!("files-preview-{name}"),
            Box::new(move |state| {
                files(state, false);
                state.storage.preview = Some(FilePreview {
                    name: "Un nome di file davvero molto lungo numero 3 versione finale.txt".into(),
                    text: "ssid=CasaMia\npassword=********\npriority=1\nUna riga molto lunga che supera di parecchio la larghezza disponibile nel riquadro di anteprima\n".repeat(3),
                    truncated,
                    binary,
                });
            }),
        );
    }

    // --- Settings screens -------------------------------------------------------
    add(
        "network-connected",
        Box::new(|state| {
            state.network.ssid = Some("FRITZ!Box 7590 Casa Malerba 5GHz".into());
            state.network.ipv4_address = Some("192.168.178.123".into());
            state.network.rssi_dbm = Some(-67);
            state.network.saved_network_count = 8;
            state.router.navigate_to(ScreenRoute::Network);
        }),
    );
    add(
        "network-missing",
        Box::new(|state| {
            state.network.wifi_state = WifiConnectionState::ConfigurationMissing;
            state.router.navigate_to(ScreenRoute::Network);
        }),
    );
    add(
        "network-failed",
        Box::new(|state| {
            state.network.wifi_state = WifiConnectionState::Failed;
            state.network.error = Some(LONG_ERROR.into());
            state.network_action_selected = 2;
            state.router.navigate_to(ScreenRoute::Network);
        }),
    );
    add(
        "network-details",
        Box::new(|state| {
            state.network.error = Some(LONG_ERROR.into());
            state.router.navigate_to(ScreenRoute::NetworkDetails);
        }),
    );
    add(
        "network-saved-empty",
        Box::new(|state| state.router.navigate_to(ScreenRoute::NetworkSaved)),
    );
    add(
        "network-saved-list",
        Box::new(|state| {
            state.network.ssid = Some("FRITZ!Box 7590 Casa Malerba 5GHz".into());
            state.set_saved_networks(vec![
                SavedNetworkEntry {
                    ssid: "FRITZ!Box 7590 Casa Malerba 5GHz".into(),
                    connected: true,
                },
                SavedNetworkEntry {
                    ssid: "WWWWWWWWWWWWWWWWWWWWWW".into(),
                    connected: true,
                },
                SavedNetworkEntry {
                    ssid: "iPhone di Ema".into(),
                    connected: false,
                },
                SavedNetworkEntry {
                    ssid: "Ufficio".into(),
                    connected: false,
                },
                SavedNetworkEntry {
                    ssid: "Casa".into(),
                    connected: false,
                },
                SavedNetworkEntry {
                    ssid: "Ospiti".into(),
                    connected: false,
                },
                SavedNetworkEntry {
                    ssid: "Settima".into(),
                    connected: false,
                },
            ]);
            state.router.navigate_to(ScreenRoute::NetworkSaved);
        }),
    );
    add(
        "network-join-failed",
        Box::new(|state| {
            state.network.wifi_state = WifiConnectionState::Connecting;
            state.network.ssid = Some("FRITZ!Box 7590 Casa Malerba 5GHz".into());
            state.network.saved_network_count = 8;
            state.network_join_failed = Some("iPhone di Emanuele Malerba".into());
            state.router.navigate_to(ScreenRoute::Network);
        }),
    );
    add(
        "network-saved-menu-other",
        Box::new(|state| {
            state.network.ssid = Some("Casa".into());
            state.set_saved_networks(vec![
                SavedNetworkEntry {
                    ssid: "Casa".into(),
                    connected: false,
                },
                SavedNetworkEntry {
                    ssid: "FRITZ!Box 7590 Casa Malerba 5GHz".into(),
                    connected: false,
                },
            ]);
            state.router.navigate_to(ScreenRoute::NetworkSaved);
            state.apply(crate::buttons::ButtonEvent::Down);
            state.apply(crate::buttons::ButtonEvent::Select);
        }),
    );
    add(
        "network-saved-confirm",
        Box::new(|state| {
            state.network.ssid = Some("Casa".into());
            state.set_saved_networks(vec![
                SavedNetworkEntry {
                    ssid: "Casa".into(),
                    connected: true,
                },
                SavedNetworkEntry {
                    ssid: "Ufficio".into(),
                    connected: false,
                },
            ]);
            state.router.navigate_to(ScreenRoute::NetworkSaved);
            state.apply(crate::buttons::ButtonEvent::Select);
        }),
    );
    for (name, snapshot) in [
        ("off", WifiTransferSnapshot::default()),
        (
            "starting",
            WifiTransferSnapshot {
                state: WifiTransferState::Starting,
                ..Default::default()
            },
        ),
        (
            "lan-ready",
            WifiTransferSnapshot {
                state: WifiTransferState::Ready,
                url: Some("http://192.168.178.123/".into()),
                last_action: "Portal ready".into(),
                ..Default::default()
            },
        ),
        (
            "lan-busy",
            WifiTransferSnapshot {
                state: WifiTransferState::Ready,
                url: Some("http://192.168.178.123/".into()),
                last_action: "Uploaded /BOOKS/Il meraviglioso viaggio di Nils Holgersson.epub"
                    .into(),
                last_bytes: 1_234_567,
                ..Default::default()
            },
        ),
        (
            "failed",
            WifiTransferSnapshot {
                state: WifiTransferState::Failed,
                last_action: "Start failed".into(),
                error: Some(LONG_ERROR.into()),
                ..Default::default()
            },
        ),
        (
            "hotspot-idle",
            WifiTransferSnapshot {
                state: WifiTransferState::Ready,
                url: Some("http://4.3.2.1/".into()),
                ap_ssid: Some("RUSTMIX-5609".into()),
                ap_password: Some("SN72D48N9NNA".into()),
                join: JoinAttemptState::Idle,
                ..Default::default()
            },
        ),
        (
            "hotspot-phone-joined",
            WifiTransferSnapshot {
                state: WifiTransferState::Ready,
                url: Some("http://4.3.2.1/".into()),
                ap_ssid: Some("RUSTMIX-5609".into()),
                ap_password: Some("SN72D48N9NNA".into()),
                hotspot_clients: 1,
                join: JoinAttemptState::Idle,
                ..Default::default()
            },
        ),
        (
            "hotspot-testing",
            WifiTransferSnapshot {
                state: WifiTransferState::Ready,
                url: Some("http://4.3.2.1/".into()),
                ap_ssid: Some("RUSTMIX-5609".into()),
                ap_password: Some("SN72D48N9NNA".into()),
                join: JoinAttemptState::Testing {
                    ssid: "FRITZ!Box 7590 Casa Malerba 5GHz".into(),
                },
                ..Default::default()
            },
        ),
        (
            "hotspot-failed",
            WifiTransferSnapshot {
                state: WifiTransferState::Ready,
                url: Some("http://4.3.2.1/".into()),
                ap_ssid: Some("RUSTMIX-5609".into()),
                ap_password: Some("SN72D48N9NNA".into()),
                join: JoinAttemptState::Failed {
                    ssid: "FRITZ!Box 7590 Casa Malerba 5GHz".into(),
                    error: "wrong password".into(),
                },
                ..Default::default()
            },
        ),
    ] {
        add(
            &format!("wifi-transfer-{name}"),
            Box::new(move |state| {
                state.update_wifi_transfer_snapshot(snapshot.clone());
                state.router.navigate_to(ScreenRoute::WifiTransfer);
            }),
        );
    }

    let ota_states: Vec<(&str, OtaCheckState)> = vec![
        ("idle", OtaCheckState::Idle),
        ("checking", OtaCheckState::Checking),
        ("up-to-date", OtaCheckState::UpToDate),
        (
            "available",
            OtaCheckState::UpdateAvailable {
                version: "v1.5.0-beta.12".into(),
                download_url: "https://example.com/update.bin".into(),
            },
        ),
        (
            "check-failed",
            OtaCheckState::CheckFailed(
                "GitHub API returned HTTP status 403 (rate limit exceeded)".into(),
            ),
        ),
        ("installing", OtaCheckState::Installing),
        (
            "install-failed",
            OtaCheckState::InstallFailed(LONG_ERROR.into()),
        ),
        (
            "bootloader-available",
            OtaCheckState::BootloaderAvailable {
                release: "v1.5.0-beta.2".into(),
                installed: Some("v5.1.4, 2024-06-12".into()),
                asset: crate::bootloader_update::BootloaderAsset {
                    download_url: "https://example.com/x-bootloader.img".into(),
                    sha256: [0; 32],
                    size: 19_008,
                },
            },
        ),
        ("bootloader-preparing", OtaCheckState::PreparingBootloader),
        (
            "bootloader-ready",
            OtaCheckState::BootloaderReady {
                release: "v1.5.0-beta.2".into(),
                new: Some("v5.5.1, 2026-10-02".into()),
            },
        ),
        ("bootloader-installing", OtaCheckState::InstallingBootloader),
        ("bootloader-installed", OtaCheckState::BootloaderInstalled),
        (
            "bootloader-damaged",
            OtaCheckState::BootloaderDamaged("readback".into()),
        ),
        (
            "bootloader-failed",
            OtaCheckState::BootloaderFailed(
                "Batteria al 35%: caricala almeno al 50% o collega il cavo USB.".into(),
            ),
        ),
    ];
    for (name, ota) in ota_states {
        add(
            &format!("ota-{name}"),
            Box::new(move |state| {
                state.installed_bootloader = Some("v5.1.4, 2024-06-12".into());
                state.ota = ota.clone();
                state.router.navigate_to(ScreenRoute::OtaUpdate);
            }),
        );
    }

    for (name, progress) in [
        ("sized", InstallProgress::new(900_000, Some(1_900_000))),
        ("done", InstallProgress::new(1_900_000, Some(1_900_000))),
        ("unsized", InstallProgress::new(600_000, None)),
    ] {
        add(
            &format!("ota-installing-progress-{name}"),
            Box::new(move |state| {
                state.installed_bootloader = Some("v5.1.4, 2024-06-12".into());
                state.ota = OtaCheckState::Installing;
                state.ota_install_progress = Some(progress);
                state.router.navigate_to(ScreenRoute::OtaUpdate);
            }),
        );
    }

    add(
        "audio",
        Box::new(|state| state.router.navigate_to(ScreenRoute::Audio)),
    );
    add(
        "audio-muted",
        Box::new(|state| {
            state.audio.muted = true;
            state.audio_action_selected = 4;
            state.router.navigate_to(ScreenRoute::Audio);
        }),
    );
    add(
        "audio-details",
        Box::new(|state| {
            state.audio.error = Some(LONG_ERROR.into());
            state.router.navigate_to(ScreenRoute::AudioDetails);
        }),
    );
    add(
        "clock",
        Box::new(|state| state.router.navigate_to(ScreenRoute::Clock)),
    );
    add(
        "clock-no-rtc",
        Box::new(|state| {
            state.board.rtc = None;
            state.board.power = None;
            state.router.navigate_to(ScreenRoute::Clock);
        }),
    );
    add(
        "clock-set-time",
        Box::new(|state| {
            state.router.navigate_to(ScreenRoute::Clock);
            state.apply(crate::buttons::ButtonEvent::Select);
        }),
    );
    add(
        "clock-set-time-new-york",
        Box::new(|state| {
            state.regional.timezone = crate::regional::TimeZoneProfile::AmericaNewYork;
            state.router.navigate_to(ScreenRoute::Clock);
            state.apply(crate::buttons::ButtonEvent::Select);
        }),
    );
    for (name, presses) in [("day", 1), ("minute", 5), ("save", 6)] {
        add(
            &format!("clock-set-time-{name}"),
            Box::new(move |state| {
                state.router.navigate_to(ScreenRoute::Clock);
                for _ in 0..=presses {
                    state.apply(crate::buttons::ButtonEvent::Select);
                }
            }),
        );
    }
    add(
        "clock-details",
        Box::new(|state| {
            state.board.rtc_clock_integrity_was_lost = true;
            state.router.navigate_to(ScreenRoute::ClockDetails);
        }),
    );
    for (name, sleep, auto) in [
        ("default", SleepScreenMode::Sequential, AutoSleep::Minutes10),
        ("cover-never", SleepScreenMode::BookCover, AutoSleep::Never),
        ("random-60", SleepScreenMode::Random, AutoSleep::Minutes60),
    ] {
        add(
            &format!("display-{name}"),
            Box::new(move |state| {
                state.display.sleep_screen = sleep;
                state.display.auto_sleep = auto;
                state.router.navigate_to(ScreenRoute::Display);
            }),
        );
    }
    add(
        "upload-wifi-page-closed",
        Box::new(|state| {
            state.network.wifi_state = crate::network::WifiConnectionState::Connected;
            state.network.ipv4_address = Some("192.168.178.123".into());
            state.network.ssid = Some("Casa".into());
            state.wifi_page_closed_notice = true;
            state.router.navigate_to(ScreenRoute::Upload);
        }),
    );
    add(
        "display-fixed-wallpaper-row",
        Box::new(|state| {
            state.display.sleep_screen = SleepScreenMode::Fixed;
            state.display_action_selected = crate::app::state::DISPLAY_FIXED_WALLPAPER_ROW;
            state.router.navigate_to(ScreenRoute::Display);
        }),
    );
    // The fixed-wallpaper chooser: a wallpaper on show, the one standby
    // already shows, an empty folder, a file that cannot be read.
    let wallpaper = || {
        crate::sleep_images::decode_sleep_bmp(include_bytes!(
            "../../examples/sd-card/RUSTMIX/SLEEP/SLEEP.BMP"
        ))
        .expect("the example wallpaper decodes")
    };
    for (name, fixed, current, readable) in [
        ("sleep-picker", false, false, true),
        ("sleep-picker-current", true, true, true),
        ("sleep-picker-unreadable", false, false, false),
    ] {
        add(
            name,
            Box::new(move |state| {
                if fixed {
                    state.display.sleep_screen = SleepScreenMode::Fixed;
                }
                state.sleep_picker.set_listing(
                    (1..=7).map(|n| format!("SLEEP{n:03}.BMP")).collect(),
                    Some(
                        if current {
                            "SLEEP001.BMP"
                        } else {
                            "SLEEP004.BMP"
                        }
                        .into(),
                    ),
                );
                if !current {
                    state.sleep_picker.step(true);
                }
                let shown = state
                    .sleep_picker
                    .selected_name()
                    .expect("a wallpaper is selected")
                    .to_string();
                state
                    .sleep_picker
                    .set_preview(&shown, readable.then(wallpaper));
                state.router.navigate_to(ScreenRoute::SleepPicker);
            }),
        );
    }
    add(
        "sleep-picker-empty",
        Box::new(|state| {
            state.sleep_picker.set_listing(Vec::new(), None);
            state.router.navigate_to(ScreenRoute::SleepPicker);
        }),
    );
    add(
        "language",
        Box::new(|state| state.router.navigate_to(ScreenRoute::Language)),
    );
    add(
        "power-key-menu",
        Box::new(|state| state.open_power_key_menu()),
    );
    add(
        "power-key-menu-restart",
        Box::new(|state| {
            state.open_power_key_menu();
            state.apply(crate::buttons::ButtonEvent::Down);
        }),
    );
    add(
        "device-info",
        Box::new(|state| state.router.navigate_to(ScreenRoute::DeviceInfo)),
    );
    for (name, presses) in [("reset-selected", 0), ("reset-armed", 1), ("reset-done", 2)] {
        add(
            &format!("device-info-{name}"),
            Box::new(move |state| {
                state.installed_bootloader = Some("v5.1.4, 2024-06-12".into());
                state.router.navigate_to(ScreenRoute::DeviceInfo);
                state.apply(crate::buttons::ButtonEvent::Down);
                for _ in 0..presses {
                    state.apply(crate::buttons::ButtonEvent::Select);
                }
            }),
        );
    }
    add(
        "device-info-board",
        Box::new(|state| state.router.navigate_to(ScreenRoute::DeviceInfoBoard)),
    );
    add(
        "device-info-runtime",
        Box::new(|state| state.router.navigate_to(ScreenRoute::DeviceInfoRuntime)),
    );
    for (name, selected, connected) in [
        ("wifi-connected", 0_usize, true),
        ("wifi-hotspot", 0, false),
        ("usb", 1, true),
    ] {
        add(
            &format!("upload-{name}"),
            Box::new(move |state| {
                state.upload_selected = selected;
                if connected {
                    state.network.wifi_state = WifiConnectionState::Connected;
                    state.network.ssid =
                        Some("Una rete Wi-Fi di casa dal nome molto lungo 5GHz".into());
                    state.network.ipv4_address = Some("192.168.100.200".into());
                } else {
                    state.network.wifi_state = WifiConnectionState::ConfigurationMissing;
                    state.network.ssid = None;
                    state.network.ipv4_address = None;
                }
                state.router.navigate_to(ScreenRoute::Upload);
            }),
        );
    }
    add(
        "usb-idle",
        Box::new(|state| state.router.navigate_to(ScreenRoute::UsbDisk)),
    );
    add(
        "usb-active",
        Box::new(|state| {
            state.usb_disk = UsbDiskPhase::Active;
            state.router.navigate_to(ScreenRoute::UsbDisk);
        }),
    );
    add(
        "usb-failed",
        Box::new(|state| {
            state.usb_disk = UsbDiskPhase::Failed(LONG_ERROR.into());
            state.router.navigate_to(ScreenRoute::UsbDisk);
        }),
    );

    // --- First run ---------------------------------------------------------
    add(
        "home-no-books",
        Box::new(|state| state.library_known_empty = true),
    );
    add(
        "library-error-card-mounted",
        Box::new(|state| {
            state.storage.mounted = true;
            state.reader.library_error = Some(LONG_ERROR.into());
            state.router.navigate_to(ScreenRoute::Library);
        }),
    );
    for (name, page) in [
        ("language", SetupPage::Language),
        ("keys", SetupPage::Keys),
        ("wifi-ready", SetupPage::Wifi),
        ("clock", SetupPage::Clock),
        ("book", SetupPage::Book),
        ("done", SetupPage::Done),
    ] {
        add(
            &format!("setup-{name}"),
            Box::new(move |state| {
                state.network.ssid =
                    Some("Una rete Wi-Fi di casa dal nome molto lungo 5GHz".into());
                state.begin_first_run_setup(page.index());
            }),
        );
    }
    add(
        "setup-wifi",
        Box::new(|state| {
            state.network.wifi_state = WifiConnectionState::ConfigurationMissing;
            state.begin_first_run_setup(SetupPage::Wifi.index());
        }),
    );
    add(
        "setup-wifi-saved-offline",
        Box::new(|state| {
            state.network.wifi_state = WifiConnectionState::Failed;
            state.network.saved_network_count = 2;
            state.begin_first_run_setup(SetupPage::Wifi.index());
        }),
    );
    add(
        "setup-clock-no-rtc",
        Box::new(|state| {
            state.board.rtc = None;
            state.begin_first_run_setup(SetupPage::Clock.index());
            state.setup.selected = 1;
        }),
    );
    add(
        "setup-book-after-upload",
        Box::new(|state| {
            state.begin_first_run_setup(SetupPage::Book.index());
            state.setup.upload_started = true;
            state.setup.selected = 2;
        }),
    );
    for (name, page) in [
        ("language", SetupPage::Language),
        ("book", SetupPage::Book),
        ("done", SetupPage::Done),
    ] {
        add(
            &format!("setup-again-{name}"),
            Box::new(move |state| {
                state.setup.start_from_settings(state.regional.locale);
                state.setup.page = page;
                state.router.navigate_to(ScreenRoute::Setup);
            }),
        );
    }
    add(
        "card-warning",
        Box::new(|state| {
            state.show_card_warning();
            state.card_warning_selected = 1;
        }),
    );
    list
}

fn save_png(frame: &FrameBuffer, orientation: DisplayOrientation, path: &std::path::Path) {
    let size = orientation.logical_size();
    let mut canvas = image::GrayImage::new(size.width, size.height);
    for y in 0..size.height {
        for x in 0..size.width {
            let logical = Point::new(x as i32, y as i32);
            let black = orientation
                .map_logical_to_native(logical)
                .and_then(|native| frame.is_black(native))
                .unwrap_or(false);
            canvas.put_pixel(x, y, image::Luma([if black { 0 } else { 255 }]));
        }
    }
    canvas.save(path).unwrap();
}

fn visible(rec: &Rec) -> bool {
    rec.width > 0 && !rec.text.trim().is_empty()
}

/// Horizontal extent actually inked, after clipping.
fn extent(rec: &Rec) -> (i32, i32) {
    let (mut left, mut right) = (rec.x, rec.x + rec.width);
    if let Some((clip_left, _, clip_right, _)) = rec.clip {
        left = left.max(clip_left);
        right = right.min(clip_right);
    }
    (left, right)
}

/// First content row below the shared header: text above it is the header
/// itself, whose clock is right-aligned past the content margin by design.
const HEADER_BOTTOM: i32 = 58;
/// Right content margin every screen below the header keeps.
const RIGHT_MARGIN: i32 = 22;
/// The footer separator: content text stays above it.
const FOOTER_LINE: i32 = 746;

#[test]
fn layout_audit_finds_no_text_off_screen_or_overlapping() {
    let out_dir: Option<PathBuf> = std::env::var("UX_AUDIT_OUT").ok().map(PathBuf::from);
    if let Some(dir) = out_dir.as_ref() {
        let _ = std::fs::create_dir_all(dir);
    }

    let mut base = AppState::default();
    base.board.rtc = Some(crate::rtc::RtcDateTime {
        year: 2026,
        month: 10,
        day: 3,
        weekday: 6,
        hour: 20,
        minute: 48,
        second: 0,
    });
    base.board.power = Some(crate::power::PowerSnapshot {
        battery_percent: Some(100),
        battery_voltage_mv: Some(4180),
        vbus_present: true,
        charging: true,
    });
    base.network.wifi_state = WifiConnectionState::Connected;

    let mut report = String::new();
    let mut strings: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for (name, arrange) in scenarios() {
        for locale in [Locale::Italian, Locale::English] {
            for font in [UiFontSize::Compact, UiFontSize::Standard, UiFontSize::Large] {
                let mut state = base.clone();
                state.regional.locale = locale;
                state.display.font_size = font;
                arrange(&mut state);
                let mut frame = FrameBuffer::new_white();
                audit::start();
                render_current_screen(&mut frame, &state).unwrap();
                let records = audit::take();
                let size = state.orientation.logical_size();
                let (width, height) = (size.width as i32, size.height as i32);
                let tag = format!("{name}\t{}\t{}", locale.name(), font.marker());
                let portrait_shell = state.orientation == DisplayOrientation::Portrait
                    && state.active_route() != ScreenRoute::ReaderPage;

                let shown: Vec<&Rec> = records.iter().filter(|rec| visible(rec)).collect();
                for rec in &shown {
                    let (left, right) = extent(rec);
                    let mut finding = |kind: &str, amount: i32| {
                        writeln!(
                            report,
                            "{kind}\t{tag}\t{amount}\t{}\t{}\t{:?}",
                            rec.x, rec.baseline, rec.text
                        )
                        .unwrap();
                    };
                    if right > width {
                        finding("OFFSCREEN", right - width);
                    } else if portrait_shell
                        && right > width - RIGHT_MARGIN
                        && rec.baseline >= HEADER_BOTTOM
                        && rec.baseline < FOOTER_LINE
                    {
                        finding("MARGIN", right - (width - RIGHT_MARGIN));
                    }
                    if left < 0 {
                        finding("OFFLEFT", -left);
                    }
                    if rec.ink_bottom > height {
                        finding("OFFBOTTOM", rec.ink_bottom - height);
                    }
                    if portrait_shell && rec.ink_bottom > FOOTER_LINE && rec.baseline < 760 {
                        finding("INTOFOOTER", rec.ink_bottom - FOOTER_LINE);
                    }
                }
                for (index, a) in shown.iter().enumerate() {
                    for b in shown.iter().skip(index + 1) {
                        // Clipped text is cut to its box by the renderer.
                        if a.clip.is_some() || b.clip.is_some() {
                            continue;
                        }
                        let (a_left, a_right) = extent(a);
                        let (b_left, b_right) = extent(b);
                        let horizontal = a_right.min(b_right) - a_left.max(b_left);
                        let vertical = a.ink_bottom.min(b.ink_bottom) - a.ink_top.max(b.ink_top);
                        if horizontal > 2 && vertical > 2 {
                            writeln!(
                                report,
                                "OVERLAP\t{tag}\t{horizontal}x{vertical}\t{}\t{}\t{:?} <> {:?}",
                                a.x, a.baseline, a.text, b.text
                            )
                            .unwrap();
                        }
                    }
                }
                strings
                    .entry((name.clone(), locale.name().to_string()))
                    .or_default()
                    .extend(shown.iter().map(|rec| rec.text.clone()));

                if let Some(dir) = out_dir.as_ref() {
                    // Italian in `<size>/`, English beside it in `<size>-en/`.
                    let dir = dir.join(match locale {
                        Locale::Italian => font.marker().to_string(),
                        Locale::English => format!("{}-en", font.marker()),
                    });
                    let _ = std::fs::create_dir_all(&dir);
                    save_png(&frame, state.orientation, &dir.join(format!("{name}.png")));
                }
            }
        }
    }
    if let Some(dir) = out_dir.as_ref() {
        std::fs::write(dir.join("report.tsv"), &report).unwrap();
        let mut listing = String::new();
        for ((name, locale), mut texts) in strings {
            texts.sort();
            texts.dedup();
            for text in texts {
                writeln!(listing, "{name}\t{locale}\t{text}").unwrap();
            }
        }
        std::fs::write(dir.join("strings.tsv"), &listing).unwrap();
    }
    assert!(
        report.is_empty(),
        "text off screen, past a margin or overlapping \
         (kind, scenario, language, size, px, x, baseline, text):\n{report}"
    );
}
