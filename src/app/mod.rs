//! Modular portrait product UI shell.
//!
//! Hardware-independent screen state and drawing code live below this module.
//! `main.rs` wires peripherals, forwards debounced events, captures optional
//! board-service snapshots and asks this shell to render the active route.

use core::convert::Infallible;

use crate::{framebuffer::FrameBuffer, orientation::OrientedFrameBuffer};

pub mod display;
pub mod i18n;
pub mod menu;
pub mod reader_atkinson_next_assets;
pub mod reader_literata_assets;
pub mod reader_typography;
pub mod router;
pub mod screens;
pub mod state;
pub mod typography;
pub mod widgets;

pub use router::ScreenRoute;
pub use state::AppState;

/// Idle interval before the panel controller and ALDO3 rail enter sleep.
pub const PANEL_IDLE_SLEEP_SECONDS: u64 = 60;
/// Idle interval, uniform across every screen including Reader, before the
/// board arms real MCU hardware deep sleep on its own, without a power-key
/// press. Reuses the exact same sleep-image entry and deep-sleep path as a
/// manual power-key press; only the trigger differs.
pub const AUTO_DEEP_SLEEP_IDLE_SECONDS: u64 = 10 * 60;
/// Development bench build, selected by setting `RUSTMIX_DEV_BENCH` in the
/// build environment: keeps a board left on USB reachable for unattended
/// flashing and logging. No idle-timeout standby (a PMIC power-off makes the
/// serial port vanish until someone presses Power), and no CPU frequency
/// scaling or automatic light sleep, which starve the USB-Serial-JTAG
/// console and truncate log lines. The Power key still sleeps on demand.
/// Release builds never set it.
pub const DEV_BENCH_BUILD: bool = option_env!("RUSTMIX_DEV_BENCH").is_some();
/// Whether the idle timeout above arms standby at all.
pub const AUTO_DEEP_SLEEP_ENABLED: bool = !DEV_BENCH_BUILD;
/// Detail-screen status cadence inherited from the sample-app clock use case.
pub const SAMPLE_LIVE_REFRESH_SECONDS: u64 = 30;
/// Route-independent poll cadence for the persistent header's PMIC charging
/// indicator. Home (the usual screen right after boot, and where a user
/// plugging in USB is most likely to be sitting) is deliberately excluded
/// from `ScreenRoute::uses_live_status`'s periodic refresh, since that also
/// forces a status sample and an e-paper repaint on a
/// screen meant to stay otherwise idle. This poll is cheaper and does not
/// repaint on its own: every tick it takes a light reading (RTC + PMIC)
/// and only triggers a screen refresh when the charging flag
/// actually flips, so a device that is neither plugged nor unplugged causes
/// no extra panel wear. This is what catches both a charger plugged in while
/// idling on Home, and the AXP2101's charger-status classification still
/// settling in the instant right after a cold power-on with VBUS already
/// present.
pub const CHARGING_STATUS_POLL_SECONDS: u64 = 5;
/// Network diagnostics refresh while visible.
pub const NETWORK_LIVE_REFRESH_SECONDS: u64 = 10;
/// Concise network serial heartbeat; UI refresh remains independent.
pub const NETWORK_LOG_HEARTBEAT_SECONDS: u64 = 30;
/// Minimum spacing between panel refreshes triggered by newly generated
/// Library cover thumbnails. Thumbnail *generation* is bounded to ~20-40ms
/// and safe every loop iteration, but the panel refresh itself is not — a
/// real e-paper update takes far longer, so redraws triggered by generation
/// progress must be throttled independently of how fast thumbnails build.
pub const LIBRARY_THUMBNAIL_REFRESH_SECONDS: u64 = 2;
/// Minimum continuous time on a reader-active route before Wi-Fi and the
/// audio rail are suspended for battery savings. Long enough that briefly
/// opening a book from Library and backing out doesn't thrash the radio.
pub const READER_POWER_SAVE_GRACE_SECONDS: u64 = 20;

/// Clear the native frame and render the active product screen through the
/// orientation adapter.
pub fn render_current_screen(frame: &mut FrameBuffer, state: &AppState) -> Result<(), Infallible> {
    frame.clear_white();
    let mut display = OrientedFrameBuffer::new(frame, state.orientation);
    screens::render_active_screen(&mut display, state)
}

#[cfg(test)]
mod tests {
    use embedded_graphics::prelude::Point;

    use super::{menu::home_entries, render_current_screen, AppState, ScreenRoute};
    use crate::{buttons::ButtonEvent, framebuffer::FrameBuffer};

    /// Position of `route` on the Home dashboard, so these tests keep
    /// working as tiles are added or removed.
    fn home_index(route: ScreenRoute) -> usize {
        home_entries()
            .iter()
            .position(|entry| entry.route == route)
            .expect("route is on the Home dashboard")
    }

    #[test]
    fn home_renderer_uses_the_shared_white_header_and_footer_chrome() {
        let mut frame = FrameBuffer::new_white();
        render_current_screen(&mut frame, &AppState::default()).unwrap();
        // Home now shares the same white header/footer chrome as every other
        // screen instead of its own dedicated black dashboard bar.
        assert_eq!(frame.is_black(Point::new(0, 479)), Some(false));
        assert_eq!(frame.is_black(Point::new(799, 479)), Some(false));
        // The outer margin beside the category rows remains white.
        assert_eq!(frame.is_black(Point::new(400, 0)), Some(false));
    }

    #[test]
    fn settings_display_renderer_is_reachable_from_home() {
        let mut frame = FrameBuffer::new_white();
        let mut state = AppState::default();
        state.home_selected = home_index(ScreenRoute::Settings);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Settings);
        for _ in 0..4 {
            state.apply(ButtonEvent::Down);
        }
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Display);
        render_current_screen(&mut frame, &state).unwrap();
        // The header background is now white (black text/icons on white),
        // so the former header-corner pixel is no longer inked.
        assert_eq!(frame.is_black(Point::new(0, 479)), Some(false));
    }

    #[test]
    fn home_file_browser_route_renders() {
        let mut frame = FrameBuffer::new_white();
        let mut state = AppState::default();
        state.home_selected = home_index(ScreenRoute::Files);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Files);
        render_current_screen(&mut frame, &state).unwrap();
    }

    /// Sample Library books for the `library` preview shot: two
    /// in-progress, one finished, two never opened — enough to exercise all
    /// three status-bar styles the Library grid draws. Must run *after*
    /// navigating into the Library route (which calls `refresh_library`,
    /// rescanning `books_root` from disk and wiping any book data set
    /// beforehand).
    fn seed_library_preview_books(state: &mut AppState) {
        let book = |path: &str, title: &str| crate::reader::ReaderBook {
            path: path.into(),
            title: title.into(),
            format: crate::reader::BookFormat::Epub,
            size_bytes: 900_000,
            modified_seconds: 0,
        };
        let location = |path: &str, title: &str, percent: u8| crate::reader::ReaderLocation {
            path: path.into(),
            title: title.into(),
            format: crate::reader::BookFormat::Epub,
            size_bytes: 900_000,
            modified_seconds: 0,
            page_index: 10,
            byte_offset: 9_000 * u64::from(percent),
            epub_chapter: None,
            reading_percent: Some(percent),
        };
        state.reader.books = vec![
            book("MONDO1.EPUB", "Mondo Emerso"),
            book("DUNGEON.EPUB", "Dungeon Crawler Carl"),
            book("MONDO2.EPUB", "Mondo Emerso Vol. 2"),
            book("GALATTICA.EPUB", "Guida Galattica"),
            book("EXTRA.EPUB", "Un Altro Libro Mai Aperto"),
        ];
        state.reader.recent = vec![
            location("MONDO1.EPUB", "Mondo Emerso", 17),
            location("DUNGEON.EPUB", "Dungeon Crawler Carl", 30),
            location("MONDO2.EPUB", "Mondo Emerso Vol. 2", 100),
        ];
    }

    /// Sample Library books for the `library-deep-scroll` preview shot: 2
    /// in-progress plus 8 never-opened books, so the Recent section runs 4
    /// rows deep — enough to scroll well past its header and exercise the
    /// "keep the current section's header pinned in view" behavior in
    /// `screens::reader::library_window`.
    fn seed_library_preview_books_long(state: &mut AppState) {
        let book = |path: &str, title: &str| crate::reader::ReaderBook {
            path: path.into(),
            title: title.into(),
            format: crate::reader::BookFormat::Epub,
            size_bytes: 900_000,
            modified_seconds: 0,
        };
        let location = |path: &str, title: &str, percent: u8| crate::reader::ReaderLocation {
            path: path.into(),
            title: title.into(),
            format: crate::reader::BookFormat::Epub,
            size_bytes: 900_000,
            modified_seconds: 0,
            page_index: 10,
            byte_offset: 9_000 * u64::from(percent),
            epub_chapter: None,
            reading_percent: Some(percent),
        };
        state.reader.books = vec![
            book("MONDO1.EPUB", "Mondo Emerso"),
            book("DUNGEON.EPUB", "Dungeon Crawler Carl"),
            book("N3.EPUB", "Racconti dal Nord"),
            book("N4.EPUB", "L'Ultimo Faro"),
            book("N5.EPUB", "Cronache di Vetro"),
            book("N6.EPUB", "Il Giardino Sommerso"),
            book("N7.EPUB", "Sentieri di Polvere"),
            book("N8.EPUB", "La Torre Bianca"),
            book("N9.EPUB", "Voci nella Nebbia"),
            book("N10.EPUB", "Il Codice Perduto"),
        ];
        state.reader.recent = vec![
            location("MONDO1.EPUB", "Mondo Emerso", 17),
            location("DUNGEON.EPUB", "Dungeon Crawler Carl", 30),
        ];
    }

    /// Host-only visual review aid: renders a few representative screens to
    /// PNGs in the OS temp directory (logical portrait orientation, as a
    /// human looks at the device) so UI layout changes can be eyeballed
    /// without physical hardware. Not part of the normal test run.
    #[test]
    #[ignore = "run explicitly with `cargo test -- --ignored` for a visual UI review"]
    fn export_screen_previews() {
        let mut state = AppState::default();
        state.board.rtc = Some(crate::rtc::RtcDateTime {
            year: 2026,
            month: 8,
            day: 13,
            weekday: 4,
            hour: 14,
            minute: 37,
            second: 0,
        });
        state.board.power = Some(crate::power::PowerSnapshot {
            battery_percent: Some(63),
            battery_voltage_mv: Some(3950),
            vbus_present: false,
            charging: false,
        });
        state.network.wifi_state = crate::network::WifiConnectionState::Connected;

        let out_dir = std::env::temp_dir().join("rustmix-ui-preview");
        let _ = std::fs::create_dir_all(&out_dir);

        let shots: &[(&str, fn(&mut AppState))] = &[
            ("home", |_| {}),
            ("home-with-book", |state| {
                state.reader.resume = Some(crate::reader::ReaderLocation {
                    path: "WIND.EPUB".into(),
                    title: "Il nome del vento".into(),
                    format: crate::reader::BookFormat::Epub,
                    size_bytes: 900_000,
                    modified_seconds: 0,
                    page_index: 120,
                    byte_offset: 558_000,
                    epub_chapter: None,
                    reading_percent: Some(62),
                });
                state.update_reading_stats_snapshot(crate::reading_stats::ReadingStatsSnapshot {
                    available: true,
                    today_seconds: 1_380,
                    streak_days: 5,
                    remaining_book_seconds: Some(12_000),
                    ..Default::default()
                });
            }),
            ("wifi-transfer", |state| {
                state.update_wifi_transfer_snapshot(crate::wifi_transfer::WifiTransferSnapshot {
                    state: crate::wifi_transfer::WifiTransferState::Ready,
                    url: Some("http://192.168.1.10/".into()),
                    code: Some("244126".into()),
                    last_action: "Portal ready".into(),
                    last_bytes: 0,
                    ..Default::default()
                });
                state
                    .router
                    .navigate_to(crate::app::ScreenRoute::WifiTransfer);
            }),
            ("wifi-transfer-hotspot", |state| {
                state.update_wifi_transfer_snapshot(crate::wifi_transfer::WifiTransferSnapshot {
                    state: crate::wifi_transfer::WifiTransferState::Ready,
                    url: Some("http://192.168.71.1/".into()),
                    code: Some("713284".into()),
                    ap_ssid: Some("RUSTMIX-5609".into()),
                    ap_password: Some("SN72D48N9NNA".into()),
                    join: crate::wifi_transfer::JoinAttemptState::Idle,
                    ..Default::default()
                });
                state
                    .router
                    .navigate_to(crate::app::ScreenRoute::WifiTransfer);
            }),
            ("library", |state| {
                // Italian, matching the product's primary target locale —
                // otherwise the section captions and status labels render
                // in their English fallback ("RECENT"/"NEW"/"DONE") instead
                // of "RECENTI"/"NUOVO"/"COMPLETATO".
                state.regional.locale = crate::regional::Locale::Italian;
                state.home_selected = home_index(ScreenRoute::Library);
                state.apply(crate::buttons::ButtonEvent::Select);
                // Seeded after navigating in: entering the Library route
                // calls `refresh_library`, which rescans `books_root` from
                // disk and would otherwise wipe book data set beforehand.
                seed_library_preview_books(state);
            }),
            ("library-recent-section", |state| {
                state.regional.locale = crate::regional::Locale::Italian;
                state.home_selected = home_index(ScreenRoute::Library);
                state.apply(crate::buttons::ButtonEvent::Select);
                // One never-opened book and one finished book, so Recent's
                // single row shows New and Completato side by side: New
                // sorts first, Completato last, per the requested order.
                let book = |path: &str, title: &str| crate::reader::ReaderBook {
                    path: path.into(),
                    title: title.into(),
                    format: crate::reader::BookFormat::Epub,
                    size_bytes: 900_000,
                    modified_seconds: 0,
                };
                let location = |path: &str, title: &str, percent: u8| crate::reader::ReaderLocation {
                    path: path.into(),
                    title: title.into(),
                    format: crate::reader::BookFormat::Epub,
                    size_bytes: 900_000,
                    modified_seconds: 0,
                    page_index: 10,
                    byte_offset: 9_000 * u64::from(percent),
                    epub_chapter: None,
                    reading_percent: Some(percent),
                };
                state.reader.books = vec![
                    book("MONDO1.EPUB", "Mondo Emerso"),
                    book("DUNGEON.EPUB", "Dungeon Crawler Carl"),
                    book("MONDO2.EPUB", "Mondo Emerso Vol. 2"),
                    book("GALATTICA.EPUB", "Guida Galattica"),
                ];
                state.reader.recent = vec![
                    location("MONDO1.EPUB", "Mondo Emerso", 17),
                    location("DUNGEON.EPUB", "Dungeon Crawler Carl", 30),
                    location("MONDO2.EPUB", "Mondo Emerso Vol. 2", 100),
                ];
                // Reading Now has 2 entries (0-1); Recent is [3: New, 2:
                // Completato] — select the New entry to scroll Recent into
                // view.
                state.reader.library_selected = 3;
            }),
            ("library-zero-percent", |state| {
                // Reproduces a report from real device data: one book at
                // 99% and the rest opened but sitting at 0% — the 0% ones
                // must land in Recent as "New", not in Reading Now, or
                // Recent never gets a header at all when every book has a
                // saved position.
                state.regional.locale = crate::regional::Locale::Italian;
                state.home_selected = home_index(ScreenRoute::Library);
                state.apply(crate::buttons::ButtonEvent::Select);
                let book = |path: &str, title: &str| crate::reader::ReaderBook {
                    path: path.into(),
                    title: title.into(),
                    format: crate::reader::BookFormat::Epub,
                    size_bytes: 900_000,
                    modified_seconds: 0,
                };
                let location = |path: &str, title: &str, percent: u8| crate::reader::ReaderLocation {
                    path: path.into(),
                    title: title.into(),
                    format: crate::reader::BookFormat::Epub,
                    size_bytes: 900_000,
                    modified_seconds: 0,
                    page_index: 0,
                    byte_offset: 9_000 * u64::from(percent),
                    epub_chapter: None,
                    reading_percent: Some(percent),
                };
                state.reader.books = vec![
                    book("MONDO1.EPUB", "Mondo Emerso"),
                    book("DUNGEON.EPUB", "Dungeon Crawler Carl"),
                    book("GALATTICA.EPUB", "Guida Galattica"),
                ];
                state.reader.recent = vec![
                    location("MONDO1.EPUB", "Mondo Emerso", 99),
                    location("DUNGEON.EPUB", "Dungeon Crawler Carl", 0),
                    location("GALATTICA.EPUB", "Guida Galattica", 0),
                ];
            }),
            ("library-book-actions", |state| {
                // Held SELECT on a Library cover opens this overlay for the
                // selected book.
                state.regional.locale = crate::regional::Locale::Italian;
                state.home_selected = home_index(ScreenRoute::Library);
                state.apply(crate::buttons::ButtonEvent::Select);
                // Seeded after navigating in: entering the Library route
                // calls `refresh_library`, which rescans `books_root` from
                // disk and would otherwise wipe book data set beforehand.
                state.reader.books = vec![crate::reader::ReaderBook {
                    path: "MONDO1.EPUB".into(),
                    title: "Mondo Emerso".into(),
                    format: crate::reader::BookFormat::Epub,
                    size_bytes: 900_000,
                    modified_seconds: 0,
                }];
                assert!(state.apply_library_select_long_press());
            }),
            ("library-book-bookmarks", |state| {
                state.regional.locale = crate::regional::Locale::Italian;
                let book = crate::reader::ReaderBook {
                    path: "MONDO1.EPUB".into(),
                    title: "Mondo Emerso".into(),
                    format: crate::reader::BookFormat::Epub,
                    size_bytes: 900_000,
                    modified_seconds: 0,
                };
                state.reader.books = vec![book.clone()];
                state.reader.bookmarks = vec![
                    crate::reader::ReaderLocation {
                        path: "MONDO1.EPUB".into(),
                        title: "Mondo Emerso".into(),
                        format: crate::reader::BookFormat::Epub,
                        size_bytes: 900_000,
                        modified_seconds: 0,
                        page_index: 12,
                        byte_offset: 40_000,
                        epub_chapter: None,
                        reading_percent: Some(17),
                    },
                    crate::reader::ReaderLocation {
                        path: "MONDO1.EPUB".into(),
                        title: "Mondo Emerso".into(),
                        format: crate::reader::BookFormat::Epub,
                        size_bytes: 900_000,
                        modified_seconds: 0,
                        page_index: 40,
                        byte_offset: 120_000,
                        epub_chapter: None,
                        reading_percent: Some(52),
                    },
                    // A bookmark from a different book, to prove the list is
                    // filtered rather than showing every saved bookmark.
                    crate::reader::ReaderLocation {
                        path: "OTHER.EPUB".into(),
                        title: "Un Altro Libro".into(),
                        format: crate::reader::BookFormat::Epub,
                        size_bytes: 500_000,
                        modified_seconds: 0,
                        page_index: 5,
                        byte_offset: 10_000,
                        epub_chapter: None,
                        reading_percent: Some(5),
                    },
                ];
                state.reader.open_book_actions(book);
                state
                    .router
                    .navigate_to(crate::app::ScreenRoute::LibraryBookBookmarks);
            }),
            ("library-deep-scroll", |state| {
                state.regional.locale = crate::regional::Locale::Italian;
                state.home_selected = home_index(ScreenRoute::Library);
                state.apply(crate::buttons::ButtonEvent::Select);
                seed_library_preview_books_long(state);
                // Reading Now has 2 entries (0-1); Recent's rows are
                // [2,3],[4,5],[6,7],[8,9] — index 7 sits in the third row,
                // scrolled well past the "RECENT" header.
                state.reader.library_selected = 7;
            }),
            ("settings-paged", |state| {
                state.home_selected = home_index(ScreenRoute::Settings);
                state.apply(crate::buttons::ButtonEvent::Select);
            }),
            ("device-info-board", |state| {
                state.home_selected = home_index(ScreenRoute::Settings);
                state.apply(crate::buttons::ButtonEvent::Select);
                for _ in 0..4 {
                    state.apply(crate::buttons::ButtonEvent::Down);
                }
                state.apply(crate::buttons::ButtonEvent::Select);
                state.apply(crate::buttons::ButtonEvent::Select);
            }),
            ("files", |state| {
                state.home_selected = home_index(ScreenRoute::Files);
                state.apply(crate::buttons::ButtonEvent::Select);
            }),
            ("statistics-empty", |state| {
                state.home_selected = home_index(ScreenRoute::ReadingStats);
                state.apply(crate::buttons::ButtonEvent::Select);
            }),
            ("statistics-with-data", |state| {
                state.update_reading_stats_snapshot(crate::reading_stats::ReadingStatsSnapshot {
                    available: true,
                    today_seconds: 1_800,
                    sessions_today: 3,
                    week_seconds: 13_320,
                    month_seconds: 72_000,
                    streak_days: 5,
                    chars_per_minute: Some(180),
                    remaining_chapter_seconds: Some(600),
                    remaining_book_seconds: Some(12_000),
                    last_7_days: [
                        crate::reading_stats::DayBar {
                            weekday: 1,
                            total_seconds: 1_800,
                        },
                        crate::reading_stats::DayBar {
                            weekday: 2,
                            total_seconds: 3_600,
                        },
                        crate::reading_stats::DayBar {
                            weekday: 3,
                            total_seconds: 0,
                        },
                        crate::reading_stats::DayBar {
                            weekday: 4,
                            total_seconds: 2_700,
                        },
                        crate::reading_stats::DayBar {
                            weekday: 5,
                            total_seconds: 900,
                        },
                        crate::reading_stats::DayBar {
                            weekday: 6,
                            total_seconds: 2_400,
                        },
                        crate::reading_stats::DayBar {
                            weekday: 0,
                            total_seconds: 1_920,
                        },
                    ],
                    books_this_month: vec![
                        crate::reading_stats::BookMonthStats {
                            book_id: 1,
                            total_seconds: 8_280,
                        },
                        crate::reading_stats::BookMonthStats {
                            book_id: 2,
                            total_seconds: 5_040,
                        },
                    ],
                });
                state.reader.recent = vec![
                    crate::reader::ReaderLocation {
                        path: "GATSBY.TXT".into(),
                        title: "Il nome del vento".into(),
                        format: crate::reader::BookFormat::Text,
                        size_bytes: 300_000,
                        modified_seconds: 0,
                        page_index: 0,
                        byte_offset: 186_000,
                        epub_chapter: None,
                        reading_percent: Some(62),
                    },
                    crate::reader::ReaderLocation {
                        path: "HAILMARY.TXT".into(),
                        title: "Project Hail Mary".into(),
                        format: crate::reader::BookFormat::Text,
                        size_bytes: 400_000,
                        modified_seconds: 0,
                        page_index: 0,
                        byte_offset: 400_000,
                        epub_chapter: None,
                        reading_percent: Some(100),
                    },
                ];
                let ids: Vec<u32> = state
                    .reader
                    .recent
                    .iter()
                    .map(|location| {
                        crate::reading_stats::book_id_for(
                            &location.path,
                            location.size_bytes,
                            location.modified_seconds,
                        )
                    })
                    .collect();
                if let Some(snapshot_books) = Some(&mut state.reading_stats.books_this_month) {
                    for (entry, id) in snapshot_books.iter_mut().zip(ids) {
                        entry.book_id = id;
                    }
                }
                state.home_selected = home_index(ScreenRoute::ReadingStats);
                state.apply(crate::buttons::ButtonEvent::Select);
            }),
            ("reader-preferences", |state| {
                state.reader.preferences_selected = 0;
                state
                    .router
                    .navigate_to(crate::app::ScreenRoute::ReaderPreferences);
            }),
            ("reader-page", |state| {
                state.reader.session = Some(crate::reader::ReaderSession {
                    book: crate::reader::ReaderBook {
                        path: "GATSBY.TXT".into(),
                        title: "The Great Gatsby".into(),
                        format: crate::reader::BookFormat::Text,
                        size_bytes: 300_000,
                        modified_seconds: 0,
                    },
                    encoding: crate::reader::TextEncoding::Utf8,
                    epub_document: None,
                    layout: crate::reader::ReaderPreferences::default().layout(),
                    current_page: 66,
                    page_number_base: 0,
                    page_offsets: vec![0; 340],
                    indexed_through: 300_000,
                    index_complete: true,
                    cache: vec![crate::reader::ReaderCachedPage {
                        page_index: 66,
                        byte_offset: 0,
                        next_byte_offset: 0,
                        lines: vec![
                            crate::reader::ReaderPageLine {
                                text: "In my younger and more vulnerable years my".into(),
                                paragraph_end: false,
                                image: None,
                            },
                            crate::reader::ReaderPageLine {
                                text: "father gave me some advice that I have been".into(),
                                paragraph_end: false,
                                image: None,
                            },
                            crate::reader::ReaderPageLine {
                                text: "turning over in my mind ever since.".into(),
                                paragraph_end: true,
                                image: None,
                            },
                        ],
                    }],
                    epub_chapter_pages: Vec::new(),
                    epub_pending_chapter: None,
                    epub_document_cache_pending: false,
                });
                state
                    .router
                    .navigate_to(crate::app::ScreenRoute::ReaderPage);
            }),
            ("reader-page-landscape", |state| {
                state.reader.session = Some(crate::reader::ReaderSession {
                    book: crate::reader::ReaderBook {
                        path: "GATSBY.TXT".into(),
                        title: "The Great Gatsby".into(),
                        format: crate::reader::BookFormat::Text,
                        size_bytes: 300_000,
                        modified_seconds: 0,
                    },
                    encoding: crate::reader::TextEncoding::Utf8,
                    epub_document: None,
                    layout: crate::reader::ReaderPreferences::default().layout(),
                    current_page: 66,
                    page_number_base: 0,
                    page_offsets: vec![0; 340],
                    indexed_through: 300_000,
                    index_complete: true,
                    cache: vec![crate::reader::ReaderCachedPage {
                        page_index: 66,
                        byte_offset: 0,
                        next_byte_offset: 0,
                        lines: vec![
                            crate::reader::ReaderPageLine {
                                text: "In my younger and more vulnerable years my".into(),
                                paragraph_end: false,
                                image: None,
                            },
                            crate::reader::ReaderPageLine {
                                text: "father gave me some advice that I have been".into(),
                                paragraph_end: false,
                                image: None,
                            },
                            crate::reader::ReaderPageLine {
                                text: "turning over in my mind ever since.".into(),
                                paragraph_end: true,
                                image: None,
                            },
                        ],
                    }],
                    epub_chapter_pages: Vec::new(),
                    epub_pending_chapter: None,
                    epub_document_cache_pending: false,
                });
                state.orientation = crate::orientation::DisplayOrientation::Landscape;
                state
                    .router
                    .navigate_to(crate::app::ScreenRoute::ReaderPage);
            }),
            // Visual check for the inline-image spike: the book path does
            // not exist on this host, so `draw_reader_inline_image` hits its
            // extraction-failure path and this exercises
            // `draw_inline_image_placeholder` end to end (bordered box +
            // alt text) exactly as a real corrupt/unsupported embedded
            // image would on device, without needing a real EPUB fixture.
            ("reader-page-with-image", |state| {
                state.reader.session = Some(crate::reader::ReaderSession {
                    book: crate::reader::ReaderBook {
                        path: "NOWHERE.EPUB".into(),
                        title: "An Illustrated Book".into(),
                        format: crate::reader::BookFormat::Epub,
                        size_bytes: 900_000,
                        modified_seconds: 0,
                    },
                    encoding: crate::reader::TextEncoding::Utf8,
                    epub_document: None,
                    layout: crate::reader::ReaderPreferences::default().layout(),
                    current_page: 12,
                    page_number_base: 0,
                    page_offsets: vec![0; 40],
                    indexed_through: 300_000,
                    index_complete: true,
                    cache: vec![crate::reader::ReaderCachedPage {
                        page_index: 12,
                        byte_offset: 0,
                        next_byte_offset: 0,
                        lines: {
                            let mut lines = vec![
                                crate::reader::ReaderPageLine {
                                    text: "A chapter with a figure follows.".into(),
                                    paragraph_end: true,
                                    image: None,
                                },
                                crate::reader::ReaderPageLine {
                                    text: String::new(),
                                    paragraph_end: false,
                                    image: Some(crate::reader::ReaderPageImage {
                                        href: "OEBPS/images/fig1.jpg".into(),
                                        alt: "Diagram of the lighthouse mechanism".into(),
                                        slot_span: 6,
                                        box_width: 752,
                                        box_height: 208,
                                    }),
                                },
                            ];
                            for _ in 1..6 {
                                lines.push(crate::reader::ReaderPageLine {
                                    text: String::new(),
                                    paragraph_end: false,
                                    image: None,
                                });
                            }
                            lines.push(crate::reader::ReaderPageLine {
                                text: "Text resumes here after the image.".into(),
                                paragraph_end: true,
                                image: None,
                            });
                            lines
                        },
                    }],
                    epub_chapter_pages: Vec::new(),
                    epub_pending_chapter: None,
                    epub_document_cache_pending: false,
                });
                state
                    .router
                    .navigate_to(crate::app::ScreenRoute::ReaderPage);
            }),
        ];

        for (name, arrange) in shots {
            let mut shot_state = state.clone();
            arrange(&mut shot_state);
            let mut frame = FrameBuffer::new_white();
            render_current_screen(&mut frame, &shot_state).unwrap();
            save_oriented_png(
                &frame,
                shot_state.orientation,
                &out_dir.join(format!("{name}.png")),
            );
        }
        println!("wrote previews to {}", out_dir.display());

        fn save_oriented_png(
            frame: &FrameBuffer,
            orientation: crate::orientation::DisplayOrientation,
            path: &std::path::Path,
        ) {
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
    }

    #[test]
    fn power_key_long_menu_route_renders_and_returns_to_previous_screen() {
        let mut frame = FrameBuffer::new_white();
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Library);
        state.open_power_key_menu();
        assert_eq!(state.active_route(), ScreenRoute::PowerKeyMenu);
        render_current_screen(&mut frame, &state).unwrap();
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Library);
        assert!(state.take_power_key_manual_refresh_request());
    }
}
