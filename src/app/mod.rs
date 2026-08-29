//! Modular portrait product UI shell.
//!
//! Hardware-independent screen state and drawing code live below this module.
//! `main.rs` wires peripherals, forwards debounced events, captures optional
//! board-service snapshots and asks this shell to render the active route.

use core::convert::Infallible;

use crate::{framebuffer::FrameBuffer, orientation::OrientedFrameBuffer};

pub mod display;
pub mod menu;
pub mod reader_atkinson_next_assets;
pub mod reader_literata_assets;
pub mod reader_serif_assets;
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
/// Detail-screen status cadence inherited from the sample-app clock use case.
pub const SAMPLE_LIVE_REFRESH_SECONDS: u64 = 30;
/// Motion diagnostics refresh at a slower e-paper-safe cadence.
pub const MOTION_LIVE_REFRESH_SECONDS: u64 = 10;
/// Motion-event diagnostics refresh slowly unless an event arrives sooner.
pub const IMU_EVENT_SCREEN_REFRESH_SECONDS: u64 =
    crate::imu_events::IMU_EVENT_SCREEN_REFRESH_SECONDS;
/// Network diagnostics refresh while visible.
pub const NETWORK_LIVE_REFRESH_SECONDS: u64 = 10;
/// Concise network serial heartbeat; UI refresh remains independent.
pub const NETWORK_LOG_HEARTBEAT_SECONDS: u64 = 30;
/// Voice-recording e-paper timer updates stay deliberately coarse.
pub const VOICE_RECORD_SCREEN_REFRESH_SECONDS: u64 =
    crate::voice_notes::VOICE_RECORD_SCREEN_REFRESH_SECONDS;
/// Minimum spacing between panel refreshes triggered by newly generated
/// Library cover thumbnails. Thumbnail *generation* is bounded to ~20-40ms
/// and safe every loop iteration, but the panel refresh itself is not — a
/// real e-paper update takes far longer, so redraws triggered by generation
/// progress must be throttled independently of how fast thumbnails build.
pub const LIBRARY_THUMBNAIL_REFRESH_SECONDS: u64 = 2;
/// Poll the PCF85063 alarm flag and domain schedule once per second.
pub const ALARM_POLL_SECONDS: u64 = 1;

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

    use super::{render_current_screen, AppState, ScreenRoute};
    use crate::{buttons::ButtonEvent, framebuffer::FrameBuffer};

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
        state.home_selected = 5;
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Settings);
        for _ in 0..3 {
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
    fn tools_file_browser_route_is_reachable() {
        let mut state = AppState::default();
        state.home_selected = 4;
        state.apply(ButtonEvent::Select);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Files);
    }

    #[test]
    fn tools_dictionary_route_renders_offline_without_sd_pack() {
        let mut frame = FrameBuffer::new_white();
        let mut state = AppState::default();
        state.home_selected = 4;
        state.apply(ButtonEvent::Select);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Dictionary);
        render_current_screen(&mut frame, &state).unwrap();
    }

    #[test]
    fn tools_unit_converter_route_renders_offline() {
        let mut frame = FrameBuffer::new_white();
        let mut state = AppState::default();
        state.home_selected = 4;
        state.apply(ButtonEvent::Select);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Down);
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::UnitConverter);
        render_current_screen(&mut frame, &state).unwrap();
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
            ("reader-category", |state| {
                state.home_selected = 0;
                state.apply(crate::buttons::ButtonEvent::Select);
            }),
            ("library", |state| {
                state.home_selected = 0;
                state.apply(crate::buttons::ButtonEvent::Select);
                state.apply(crate::buttons::ButtonEvent::Down);
                state.apply(crate::buttons::ButtonEvent::Select);
            }),
            ("settings-paged", |state| {
                state.home_selected = 5;
                state.apply(crate::buttons::ButtonEvent::Select);
            }),
            ("device-info-board", |state| {
                state.home_selected = 5;
                state.apply(crate::buttons::ButtonEvent::Select);
                for _ in 0..4 {
                    state.apply(crate::buttons::ButtonEvent::Down);
                }
                state.apply(crate::buttons::ButtonEvent::Select);
                state.apply(crate::buttons::ButtonEvent::Select);
            }),
            ("calendar", |state| {
                state.home_selected = 1;
                state.apply(crate::buttons::ButtonEvent::Select);
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
                            },
                            crate::reader::ReaderPageLine {
                                text: "father gave me some advice that I have been".into(),
                                paragraph_end: false,
                            },
                            crate::reader::ReaderPageLine {
                                text: "turning over in my mind ever since.".into(),
                                paragraph_end: true,
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
                            },
                            crate::reader::ReaderPageLine {
                                text: "father gave me some advice that I have been".into(),
                                paragraph_end: false,
                            },
                            crate::reader::ReaderPageLine {
                                text: "turning over in my mind ever since.".into(),
                                paragraph_end: true,
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
    fn power_key_short_menu_route_renders_and_returns_to_previous_screen() {
        let mut frame = FrameBuffer::new_white();
        let mut state = AppState::default();
        state.router.navigate_to(ScreenRoute::Dictionary);
        state.open_power_key_menu();
        assert_eq!(state.active_route(), ScreenRoute::PowerKeyMenu);
        render_current_screen(&mut frame, &state).unwrap();
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), ScreenRoute::Dictionary);
        assert!(state.take_power_key_manual_refresh_request());
    }
}
