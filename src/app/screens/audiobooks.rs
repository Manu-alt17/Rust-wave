//! Audiobook library list and player.

use core::convert::Infallible;

use embedded_graphics::{pixelcolor::BinaryColor, prelude::Point};
use embedded_iconoir::{
    icons::size96px::music::{Headset, Pause, Play},
    prelude::IconoirNewIcon,
};

use crate::{
    app::{
        audiobooks::PlayerMenuItem,
        i18n::t,
        state::AppState,
        typography::Text,
        widgets::{
            footer::{
                back_only, draw_footer, draw_footer_paged, footer_hints, select_and_back, FooterKey,
            },
            header::draw_header,
            home_tile::draw_iconoir_icon,
            layout::{CONTENT_LEFT, CONTENT_WIDTH, FIRST_BASELINE, FIRST_ROW_TOP},
            list::{draw_list_row, draw_row_frame, page_window, ROW_GAP, ROW_PAD_X, ROW_STEP},
            progress::draw_progress_bar,
            text::{draw_paragraph, draw_text_centered, draw_text_fit, wrap_to_width},
        },
    },
    audiobook::{format_clock_ms, PlayerState},
    orientation::OrientedFrameBuffer,
    regional::Locale,
};

const LEFT: i32 = CONTENT_LEFT;
const WIDTH: i32 = CONTENT_WIDTH;
/// Height of a title row: title, then track count and progress.
const BOOK_ROW_HEIGHT: i32 = 86;
const BOOK_ROW_STEP: i32 = BOOK_ROW_HEIGHT + ROW_GAP;
const ROWS_PER_PAGE: usize = 7;

pub fn render_audiobook_library(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let audiobooks = &state.audiobooks;
    draw_header(display, state, t(locale, "AUDIOBOOKS", "AUDIOLIBRI"))?;

    if audiobooks.books.is_empty() {
        // As in the Library: a card that cannot be read is said in plain
        // words, and an empty list offers the way to fill it.
        let unreadable = audiobooks.scan_error.is_some();
        let message = if unreadable {
            t(
                locale,
                "The memory card cannot be read. Check that it is pushed all the way in, then restart the device.",
                "La scheda di memoria non si legge. Controlla che sia inserita fino in fondo, poi riavvia il dispositivo.",
            )
        } else {
            t(
                locale,
                "No audiobooks yet. Press SELECT to open Upload and copy them from a computer or a phone. They are MP3 files in the RUSTMIX/AUDIO folder of the card: one file per title, or one folder per title with its tracks inside.",
                "Nessun audiolibro. Premi SELECT per aprire Carica e copiarli dal computer o dal telefono. Sono file MP3 nella cartella RUSTMIX/AUDIO della scheda: un file per titolo, oppure una cartella per titolo con dentro le tracce.",
            )
        };
        let after = draw_paragraph(display, message, LEFT, FIRST_BASELINE, body, WIDTH, 9, 6)?;
        if let Some(error) = audiobooks
            .scan_error
            .as_deref()
            .filter(|_| state.storage.mounted)
        {
            draw_paragraph(display, error, LEFT, after + 10, body, WIDTH, 3, 4)?;
        }
        let hint = if unreadable {
            back_only(locale)
        } else {
            select_and_back(locale, t(locale, "UPLOAD", "CARICA"))
        };
        return draw_footer(display, state, &hint);
    }

    let page = audiobooks.selected / ROWS_PER_PAGE;
    let first = page * ROWS_PER_PAGE;
    for (row, book) in audiobooks
        .books
        .iter()
        .enumerate()
        .skip(first)
        .take(ROWS_PER_PAGE)
    {
        let top = FIRST_ROW_TOP + (row - first) as i32 * BOOK_ROW_STEP;
        draw_row_frame(display, top, BOOK_ROW_HEIGHT, row == audiobooks.selected)?;
        let loaded = audiobooks.now_playing.key == book.key;
        let mut title = book.title.clone();
        if loaded && audiobooks.now_playing.is_active() {
            title = format!("\u{00BB} {title}");
        }
        draw_text_fit(
            display,
            &title,
            Point::new(LEFT + ROW_PAD_X, top + 34),
            heading,
            WIDTH - 2 * ROW_PAD_X,
        )?;
        let tracks = match (book.tracks.len(), locale) {
            (1, Locale::English) => "1 track".to_string(),
            (1, Locale::Italian) => "1 traccia".to_string(),
            (count, Locale::English) => format!("{count} tracks"),
            (count, Locale::Italian) => format!("{count} tracce"),
        };
        let progress = audiobooks.progress_percent(book);
        let status = match progress {
            Some(percent) => format!("{tracks} \u{00B7} {percent}%"),
            None => tracks,
        };
        Text::new(&status, Point::new(LEFT + ROW_PAD_X, top + 66), body).draw(display)?;
        if let Some(percent) = progress {
            draw_progress_bar(display, LEFT + 230, top + 56, 188, 10, percent)?;
        }
    }

    let pages = audiobooks.books.len().div_ceil(ROWS_PER_PAGE);
    draw_footer_paged(
        display,
        state,
        &select_and_back(locale, t(locale, "PLAY", "ASCOLTA")),
        Some((page + 1, pages)),
    )
}

pub fn render_audiobook_player(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let large = state.display.large_style();
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let now = &state.audiobooks.now_playing;
    if let Some(selected) = state.audiobooks.track_list {
        return render_track_list(display, state, selected);
    }
    if let Some(selected) = state.audiobooks.menu {
        return render_player_menu(display, state, selected);
    }
    draw_header(display, state, t(locale, "NOW PLAYING", "IN ASCOLTO"))?;

    let icon_left = (480 - 96) / 2;
    match now.state {
        PlayerState::Playing | PlayerState::Loading => {
            draw_iconoir_icon(
                display,
                Point::new(icon_left, 76),
                &Play::new(BinaryColor::On),
            )?;
        }
        PlayerState::Paused => {
            draw_iconoir_icon(
                display,
                Point::new(icon_left, 76),
                &Pause::new(BinaryColor::On),
            )?;
        }
        _ => draw_iconoir_icon(
            display,
            Point::new(icon_left, 76),
            &Headset::new(BinaryColor::On),
        )?,
    }

    let mut baseline = 214;
    for line in wrap_to_width(large, &now.title, WIDTH, 2) {
        draw_text_centered(display, &line, LEFT, WIDTH, baseline, large)?;
        baseline += i32::from(large.line_height()) + 4;
    }

    let track = if now.track_count > 0 {
        match locale {
            Locale::English => format!("Track {} of {}", now.track + 1, now.track_count),
            Locale::Italian => format!("Traccia {} di {}", now.track + 1, now.track_count),
        }
    } else {
        String::new()
    };
    Text::new(&track, Point::new(LEFT, 330), heading).draw(display)?;
    if now.track_count > 1 {
        draw_text_fit(
            display,
            &now.track_title,
            Point::new(LEFT, 364),
            body,
            WIDTH,
        )?;
    }

    let percent = if now.duration_ms > 0 {
        (now.position.position_ms.min(now.duration_ms) * 100 / now.duration_ms) as u8
    } else {
        0
    };
    draw_progress_bar(display, LEFT, 392, WIDTH, 16, percent)?;
    let elapsed = format_clock_ms(now.position.position_ms);
    let total = format_clock_ms(now.duration_ms);
    Text::new(&elapsed, Point::new(LEFT, 440), body).draw(display)?;
    Text::new(
        &total,
        Point::new(LEFT + WIDTH - body.text_width(&total), 440),
        body,
    )
    .draw(display)?;

    let status = match now.state {
        PlayerState::Loading => t(locale, "Opening...", "Apertura...").to_string(),
        PlayerState::Playing => t(locale, "Playing", "In riproduzione").to_string(),
        PlayerState::Paused => t(locale, "Paused", "In pausa").to_string(),
        PlayerState::Stopped => t(locale, "Stopped", "Fermo").to_string(),
        PlayerState::Finished => t(locale, "Finished", "Finito").to_string(),
        PlayerState::Error => now
            .error
            .clone()
            .unwrap_or_else(|| t(locale, "Playback error", "Errore di riproduzione").into()),
    };
    draw_paragraph(display, &status, LEFT, 500, heading, WIDTH, 3, 4)?;
    let volume = format!("Volume {}%", state.audio.volume_percent);
    Text::new(&volume, Point::new(LEFT, 620), body).draw(display)?;
    if let Some(minutes) = state.audiobooks.sleep_timer_remaining {
        let timer = match locale {
            Locale::English => format!("Stops in {minutes} min"),
            Locale::Italian => format!("Si ferma tra {minutes} min"),
        };
        Text::new(&timer, Point::new(LEFT, 660), body).draw(display)?;
    }

    // Three keys already fill the line: BOOT, back as everywhere, is left
    // out here.
    draw_footer(
        display,
        state,
        &footer_hints(
            locale,
            &[
                (FooterKey::Select, t(locale, "PAUSE", "PAUSA")),
                (FooterKey::UpDown, "VOLUME"),
                (FooterKey::Hold, "MENU"),
            ],
        ),
    )
}

/// Rows of the player menu and of the track list shown at a time.
const LIST_ROWS_PER_PAGE: usize = 9;

/// The sleep timer as the menu row shows it: the minutes left, or off.
fn sleep_timer_value(state: &AppState) -> String {
    let locale = state.regional.locale;
    match state.audiobooks.sleep_timer_remaining {
        Some(minutes) => format!("{minutes} min"),
        None => t(locale, "Off", "Spento").to_string(),
    }
}

/// The player menu a held SELECT opens: one row per action, the screen to
/// itself so nothing of the player shows through.
fn render_player_menu(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    selected: usize,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    draw_header(display, state, t(locale, "PLAYER", "RIPRODUZIONE"))?;
    for (index, item) in PlayerMenuItem::ALL.iter().enumerate() {
        let value = match item {
            PlayerMenuItem::SleepTimer => sleep_timer_value(state),
            PlayerMenuItem::Tracks => match state.audiobooks.now_playing.track_count {
                0 => String::new(),
                count => format!("{}/{count}", state.audiobooks.now_playing.track + 1),
            },
            _ => String::new(),
        };
        draw_list_row(
            display,
            state.display,
            FIRST_ROW_TOP + index as i32 * ROW_STEP,
            item.label_i18n(locale),
            &value,
            index == selected,
        )?;
    }
    draw_footer(
        display,
        state,
        &footer_hints(
            locale,
            &[
                (FooterKey::Select, t(locale, "RUN", "ESEGUI")),
                (FooterKey::Boot, t(locale, "CLOSE", "CHIUDI")),
            ],
        ),
    )
}

/// The tracks of the title being played, a page at a time; SELECT starts
/// the one under the cursor.
fn render_track_list(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    selected: usize,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let audiobooks = &state.audiobooks;
    let tracks = audiobooks.playing_tracks();
    draw_header(display, state, t(locale, "TRACKS", "TRACCE"))?;
    let (first, end, page, pages) = page_window(selected, tracks.len(), LIST_ROWS_PER_PAGE);
    for (row, track) in tracks.iter().enumerate().take(end).skip(first) {
        let label = format!("{}. {}", row + 1, track.title);
        let value = if row == audiobooks.now_playing.track {
            t(locale, "playing", "in ascolto")
        } else {
            ""
        };
        draw_list_row(
            display,
            state.display,
            FIRST_ROW_TOP + (row - first) as i32 * ROW_STEP,
            &label,
            value,
            row == selected,
        )?;
    }
    draw_footer_paged(
        display,
        state,
        &select_and_back(locale, t(locale, "PLAY", "ASCOLTA")),
        Some((page, pages)),
    )
}

#[cfg(test)]
mod tests {
    use embedded_graphics::pixelcolor::BinaryColor;

    use super::{render_audiobook_library, render_audiobook_player};
    use crate::{
        app::{
            display::UiFontSize,
            typography::{style_for, UiTextRole},
            widgets::text::{truncate_to_width, wrap_to_width},
            AppState,
        },
        framebuffer::FrameBuffer,
        orientation::OrientedFrameBuffer,
    };

    #[test]
    fn long_titles_wrap_then_end_in_an_ellipsis() {
        let style = style_for(UiFontSize::Standard, UiTextRole::Large, BinaryColor::On);
        let lines = wrap_to_width(
            style,
            "Il meraviglioso viaggio di Nils Holgersson attraverso la Svezia",
            200,
            2,
        );
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with('\u{2026}'));
        assert!(lines.iter().all(|line| style.text_width(line) <= 200));
        assert_eq!(truncate_to_width(style, "Breve", 200), "Breve");
    }

    #[test]
    fn library_and_player_render_empty_and_with_the_menu_open() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let mut state = AppState::default();
        render_audiobook_library(&mut display, &state).unwrap();
        render_audiobook_player(&mut display, &state).unwrap();
        state.audiobooks.open_menu();
        render_audiobook_player(&mut display, &state).unwrap();
        state.audiobooks.track_list = Some(0);
        render_audiobook_player(&mut display, &state).unwrap();
    }
}
