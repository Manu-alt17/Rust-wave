//! Audiobook library list and player.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};
use embedded_iconoir::{
    icons::size96px::music::{Headset, Pause, Play},
    prelude::IconoirNewIcon,
};

use crate::{
    app::{
        audiobooks::PlayerMenuItem,
        i18n::t,
        state::AppState,
        typography::{Text, UiTextStyle},
        widgets::{footer::draw_footer, header::draw_header, home_tile::draw_iconoir_icon},
    },
    audiobook::{format_clock_ms, PlayerState},
    orientation::OrientedFrameBuffer,
    regional::Locale,
};

const LEFT: i32 = 22;
const WIDTH: i32 = 436;
const LIST_TOP: i32 = 70;
const ROW_HEIGHT: i32 = 96;
const ROWS_PER_PAGE: usize = 7;

pub fn render_audiobook_library(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();
    let audiobooks = &state.audiobooks;
    draw_header(display, state, t(locale, "AUDIOBOOKS", "AUDIOLIBRI"))?;

    if audiobooks.books.is_empty() {
        let message = match &audiobooks.scan_error {
            Some(error) => format!(
                "{} {error}",
                t(locale, "SD card unreadable:", "Scheda SD illeggibile:")
            ),
            None => t(
                locale,
                "No audiobooks yet. Copy MP3 files into RUSTMIX/AUDIO on the SD card: one folder per title, its tracks inside.",
                "Nessun audiolibro. Copia i file MP3 nella cartella RUSTMIX/AUDIO della scheda SD: una cartella per titolo, con dentro le tracce.",
            )
            .to_string(),
        };
        let mut baseline = 140;
        for line in wrap_to_width(body, &message, WIDTH, 8) {
            Text::new(&line, Point::new(LEFT, baseline), body).draw(display)?;
            baseline += i32::from(body.line_height()) + 6;
        }
        draw_footer(display, state, t(locale, "BOOT BACK", "BOOT INDIETRO"))?;
        return Ok(());
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
        let top = LIST_TOP + (row - first) as i32 * ROW_HEIGHT;
        let selected = row == audiobooks.selected;
        Rectangle::new(
            Point::new(LEFT, top),
            Size::new(WIDTH as u32, (ROW_HEIGHT - 10) as u32),
        )
        .into_styled(PrimitiveStyle::with_stroke(
            BinaryColor::On,
            if selected { 3 } else { 1 },
        ))
        .draw(display)?;
        let loaded = audiobooks.now_playing.key == book.key;
        let mut title = book.title.clone();
        if loaded && audiobooks.now_playing.is_active() {
            title = format!("\u{00BB} {title}");
        }
        let title_width = WIDTH - 28;
        Text::new(
            &truncate_to_width(heading, &title, title_width),
            Point::new(LEFT + 14, top + 34),
            heading,
        )
        .draw(display)?;
        let tracks = match (book.tracks.len(), locale) {
            (1, Locale::English) => "1 track".to_string(),
            (1, Locale::Italian) => "1 traccia".to_string(),
            (count, Locale::English) => format!("{count} tracks"),
            (count, Locale::Italian) => format!("{count} tracce"),
        };
        let status = match audiobooks.progress_percent(book) {
            Some(percent) => format!("{tracks} \u{00B7} {percent}%"),
            None => tracks,
        };
        Text::new(&status, Point::new(LEFT + 14, top + 66), detail).draw(display)?;
        if let Some(percent) = audiobooks.progress_percent(book) {
            draw_bar(display, LEFT + 230, top + 56, 190, 10, percent)?;
        }
    }
    let pages = audiobooks.books.len().div_ceil(ROWS_PER_PAGE);
    let hint = if pages > 1 {
        match locale {
            Locale::English => format!("SELECT PLAY \u{00B7} PAGE {}/{pages}", page + 1),
            Locale::Italian => format!("SELECT ASCOLTA \u{00B7} PAGINA {}/{pages}", page + 1),
        }
    } else {
        t(locale, "SELECT PLAY", "SELECT ASCOLTA").to_string()
    };
    draw_footer(display, state, &hint)
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
        let width = large.text_width(&line);
        Text::new(
            &line,
            Point::new(LEFT + (WIDTH - width) / 2, baseline),
            large,
        )
        .draw(display)?;
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
        Text::new(
            &truncate_to_width(body, &now.track_title, WIDTH),
            Point::new(LEFT, 364),
            body,
        )
        .draw(display)?;
    }

    let percent = if now.duration_ms > 0 {
        (now.position.position_ms.min(now.duration_ms) * 100 / now.duration_ms) as u8
    } else {
        0
    };
    draw_bar(display, LEFT, 392, WIDTH, 16, percent)?;
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
    let mut status_baseline = 500;
    for line in wrap_to_width(heading, &status, WIDTH, 3) {
        Text::new(&line, Point::new(LEFT, status_baseline), heading).draw(display)?;
        status_baseline += i32::from(heading.line_height()) + 4;
    }
    let volume = format!(
        "{} {}%",
        t(locale, "Volume", "Volume"),
        state.audio.volume_percent
    );
    Text::new(&volume, Point::new(LEFT, 620), body).draw(display)?;

    if let Some(selected) = state.audiobooks.menu {
        draw_menu(display, state, selected)?;
        return draw_footer(
            display,
            state,
            t(
                locale,
                "SELECT RUN \u{00B7} BOOT CLOSE",
                "SELECT ESEGUI \u{00B7} BOOT CHIUDI",
            ),
        );
    }
    draw_footer(
        display,
        state,
        t(
            locale,
            "SELECT PLAY/PAUSE \u{00B7} UP/DOWN VOLUME \u{00B7} HOLD MENU",
            "SELECT PLAY/PAUSA \u{00B7} SU/GI\u{00D9} VOLUME \u{00B7} TIENI MENU",
        ),
    )
}

/// The player menu, drawn over the lower half of the screen.
fn draw_menu(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    selected: usize,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let body = state.display.body_style();
    let row = 52;
    let top = 736 - row * PlayerMenuItem::ALL.len() as i32 - 20;
    let height = (736 - top) as u32;
    Rectangle::new(
        Point::new(LEFT - 8, top - 10),
        Size::new((WIDTH + 16) as u32, height),
    )
    .into_styled(PrimitiveStyle::with_fill(BinaryColor::Off))
    .draw(display)?;
    Rectangle::new(
        Point::new(LEFT - 8, top - 10),
        Size::new((WIDTH + 16) as u32, height),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2))
    .draw(display)?;
    for (index, item) in PlayerMenuItem::ALL.iter().enumerate() {
        let row_top = top + index as i32 * row;
        let chosen = index == selected;
        if chosen {
            Rectangle::new(
                Point::new(LEFT, row_top),
                Size::new(WIDTH as u32, (row - 8) as u32),
            )
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;
        }
        let style = if chosen {
            body.with_color(BinaryColor::Off)
        } else {
            body
        };
        Text::new(
            item.label_i18n(locale),
            Point::new(LEFT + 16, row_top + 30),
            style,
        )
        .draw(display)?;
    }
    Ok(())
}

fn draw_bar(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    percent: u8,
) -> Result<(), Infallible> {
    Rectangle::new(
        Point::new(left, top),
        Size::new(width as u32, height as u32),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
    .draw(display)?;
    let filled = (width - 4) * i32::from(percent.min(100)) / 100;
    if filled > 0 {
        Rectangle::new(
            Point::new(left + 2, top + 2),
            Size::new(filled as u32, (height - 4).max(1) as u32),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(display)?;
    }
    Ok(())
}

/// Greedy word wrap to at most `max_lines` lines; the last line ends in an
/// ellipsis when text was left over.
fn wrap_to_width(style: UiTextStyle, text: &str, max_width: i32, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut words = text.split_whitespace().peekable();
    while let Some(word) = words.next() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if current.is_empty() || style.text_width(&candidate) <= max_width {
            current = candidate;
            continue;
        }
        if lines.len() + 1 == max_lines {
            let rest: Vec<&str> = std::iter::once(word).chain(words).collect();
            current = format!("{current} {}", rest.join(" "));
            break;
        }
        lines.push(std::mem::replace(&mut current, word.to_string()));
    }
    if !current.is_empty() && lines.len() < max_lines {
        lines.push(truncate_to_width(style, &current, max_width));
    }
    lines
}

/// Trim `text` to `max_width` pixels, ending in an ellipsis when cut.
fn truncate_to_width(style: UiTextStyle, text: &str, max_width: i32) -> String {
    if style.text_width(text) <= max_width {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String =
            chars.iter().collect::<String>().trim_end().to_string() + "\u{2026}";
        if style.text_width(&candidate) <= max_width {
            return candidate;
        }
    }
    "\u{2026}".into()
}

#[cfg(test)]
mod tests {
    use super::{truncate_to_width, wrap_to_width};
    use crate::app::display::DisplayPreferences;

    #[test]
    fn long_titles_wrap_then_end_in_an_ellipsis() {
        let style = DisplayPreferences::default().body_style();
        let lines = wrap_to_width(
            style,
            "Il nome del vento, prima giornata delle cronache dell'assassino del re, edizione integrale",
            200,
            2,
        );
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with('\u{2026}'));
        assert!(lines.iter().all(|line| style.text_width(line) <= 200));
        assert_eq!(truncate_to_width(style, "Breve", 200), "Breve");
    }
}
