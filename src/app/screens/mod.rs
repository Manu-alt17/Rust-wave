//! Screen rendering boundary for the product shell.

use core::convert::Infallible;

use crate::orientation::OrientedFrameBuffer;

use super::{router::ScreenRoute, state::AppState};

pub mod audio;
pub mod category;
pub mod clock;
pub mod device_info;
pub mod display;
pub mod files;
pub mod home;
pub mod language;
pub mod network;
pub mod ota;
pub mod power_key;
pub mod reader;
pub mod reading_stats;

/// Draw the active screen selected by the router.
pub fn render_active_screen(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    match state.active_route() {
        ScreenRoute::Home => home::render_home(display, state),
        route if route.is_category() => category::render_category(display, state),
        ScreenRoute::ContinueReading => reader::render_continue_reading(display, state),
        ScreenRoute::Library => reader::render_library(display, state),
        ScreenRoute::LibraryBookActions => reader::render_library_book_actions(display, state),
        ScreenRoute::LibraryBookBookmarks => reader::render_library_book_bookmarks(display, state),
        ScreenRoute::ReaderBookmarks => reader::render_bookmarks(display, state),
        ScreenRoute::ReaderLoading => reader::render_loading(display, state),
        ScreenRoute::ReaderPage => reader::render_page(display, state),
        ScreenRoute::ReaderOptions => reader::render_options(display, state),
        ScreenRoute::ReaderPreferences => reader::render_preferences(display, state),
        ScreenRoute::ReaderToc => reader::render_toc(display, state),
        ScreenRoute::ReadingStats => reading_stats::render_reading_stats(display, state),
        ScreenRoute::Clock => clock::render_clock(display, state),
        ScreenRoute::ClockSetTime => clock::render_clock_set_time(display, state),
        ScreenRoute::ClockDetails => clock::render_clock_details(display, state),
        ScreenRoute::Network => network::render_network(display, state),
        ScreenRoute::NetworkDetails => network::render_network_details(display, state),
        ScreenRoute::NetworkSaved => network::render_network_saved(display, state),
        ScreenRoute::WifiTransfer => network::render_wifi_transfer(display, state),
        ScreenRoute::Audio => audio::render_audio(display, state),
        ScreenRoute::AudioDetails => audio::render_audio_details(display, state),
        ScreenRoute::Files => files::render_files(display, state),
        ScreenRoute::Display => display::render_display(display, state),
        ScreenRoute::Language => language::render_language(display, state),
        ScreenRoute::PowerKeyMenu => power_key::render_power_key_menu(display, state),
        ScreenRoute::DeviceInfo => device_info::render_device_info(display, state),
        ScreenRoute::DeviceInfoBoard => device_info::render_device_info_board(display, state),
        ScreenRoute::DeviceInfoRuntime => device_info::render_device_info_runtime(display, state),
        ScreenRoute::OtaUpdate => ota::render_ota_update(display, state),
        ScreenRoute::Settings => {
            unreachable!("category routes handled above")
        }
    }
}
