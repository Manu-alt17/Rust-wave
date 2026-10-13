//! The Settings category screen drawn as an icon-tile grid, plus the
//! Continue Reading card the Home dashboard shares.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{CornerRadii, PrimitiveStyle, Rectangle, RoundedRectangle},
};

use crate::{
    app::{
        i18n::t,
        menu::category_entries,
        router::ScreenRoute,
        state::AppState,
        typography::{Text, UiTextRole, UiTextStyle},
        widgets::{
            header::draw_header,
            home_tile::{draw_home_tile_compact, COMPACT_TILE_SIZE, TILE_GAP_X, TILE_GAP_Y},
        },
    },
    cover_cache::{THUMB_HEIGHT, THUMB_WIDTH},
    orientation::OrientedFrameBuffer,
};

pub fn render_category(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    render_tile_grid(display, state, state.active_route())
}

/// Padding kept between the Continue Reading tile's border and its cover
/// art / text column on every side.
const CONTINUE_TILE_PAD: i32 = 16;
/// Gap between the cover art and the text column.
const CONTINUE_TILE_TEXT_GAP: i32 = 16;
/// Gap between the "Continue Reading" caption and the book title.
const CONTINUE_TILE_TITLE_GAP: i32 = 8;
/// The eyebrow caption wraps onto at most this many lines rather than being
/// cut off — at the tile's narrow text-column width it doesn't reliably fit
/// on one line once drawn at `UiTextRole::Body`.
const CONTINUE_TILE_CAPTION_MAX_LINES: usize = 2;
/// Book title wraps onto at most this many lines before extra words are
/// dropped.
const CONTINUE_TILE_TITLE_MAX_LINES: usize = 2;
/// Gap between the "Cap. N · P%" status line and the progress bar sitting
/// right below it.
const CONTINUE_TILE_STATUS_GAP: i32 = 10;
/// Height of the progress track, matching the Reader page's own progress bar
/// (`draw_reading_progress` in `screens/reader.rs`).
const CONTINUE_TILE_PROGRESS_HEIGHT: i32 = 10;
/// Full tile height: the cover's native height plus top/bottom padding, so
/// the cover fills the tile edge-to-edge vertically the same way it does on
/// the Library grid. Shared with the Home dashboard (see `screens::home`),
/// which draws the exact same card at the same size.
pub(crate) const CONTINUE_TILE_HEIGHT: i32 = THUMB_HEIGHT as i32 + CONTINUE_TILE_PAD * 2;
/// Corner radius matching the rest of the Home-style tile grid.
const CONTINUE_TILE_CORNER_RADIUS: Size = Size::new(16, 16);
/// Corner radius the cover thumbnail itself is masked to (see the
/// `mask_rounded_corners` call in [`draw_continue_reading_tile`]) — a little
/// smaller than `CONTINUE_TILE_CORNER_RADIUS` so the cover's curve reads as
/// concentric with the tile border around it rather than flatter or sharper
/// than the frame it sits `CONTINUE_TILE_PAD` from.
const CONTINUE_TILE_THUMB_CORNER_RADIUS: i32 = 12;
/// Selected-tile indicator dot, matching `home_tile`'s.
const CONTINUE_TILE_DOT_SIZE: Size = Size::new(10, 10);
const CONTINUE_TILE_DOT_INSET: i32 = 18;
/// Space kept between the caption and the selection dot.
const CONTINUE_TILE_CAPTION_DOT_GAP: i32 = 6;

/// Continue Reading's full-width hero card (cover, title, reading progress),
/// drawn only on the Home dashboard (see `screens::home::render_home`) —
/// there is no separate Reader screen showing a second copy of it any more,
/// so this is the one place a book's "what's in progress" state shows at a
/// glance.
pub(crate) fn draw_continue_reading_tile(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    top_left: Point,
    width: i32,
    selected: bool,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let border = if selected {
        PrimitiveStyle::with_stroke(BinaryColor::On, 4)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    RoundedRectangle::new(
        Rectangle::new(
            top_left,
            Size::new(width as u32, CONTINUE_TILE_HEIGHT as u32),
        ),
        CornerRadii::new(CONTINUE_TILE_CORNER_RADIUS),
    )
    .into_styled(border)
    .draw(display)?;

    if selected {
        let dot_top_left = Point::new(
            top_left.x + width - CONTINUE_TILE_DOT_INSET - CONTINUE_TILE_DOT_SIZE.width as i32,
            top_left.y + CONTINUE_TILE_DOT_INSET,
        );
        Rectangle::new(dot_top_left, CONTINUE_TILE_DOT_SIZE)
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;
    }

    // Home is always Portrait (every route except ReaderPage
    // forces it — see `AppState::sync_orientation_for_active_route`), so
    // the fast blit path the Library grid uses can run unconditionally here.
    let cover_point = Point::new(
        top_left.x + CONTINUE_TILE_PAD,
        top_left.y + CONTINUE_TILE_PAD,
    );
    if let Some((_, thumbnail)) = state.reader.continue_reading_thumbnail.as_ref() {
        display.blit_packed_bitmap_portrait(
            cover_point,
            thumbnail.width,
            thumbnail.height,
            &thumbnail.bits,
        );
    } else {
        Rectangle::new(
            cover_point,
            Size::new(u32::from(THUMB_WIDTH), u32::from(THUMB_HEIGHT)),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;
    }
    // Whitens the cover's four corners at draw time so it reads as rounded
    // like the rest of the tile, instead of baking the rounding into the
    // cached thumbnail bitmap (see `screens::reader::draw_library_cell`'s
    // matching call for why: that would force every cover on SD to be
    // re-decoded on the cache format bump).
    display.mask_rounded_corners(
        cover_point,
        Size::new(u32::from(THUMB_WIDTH), u32::from(THUMB_HEIGHT)),
        CONTINUE_TILE_THUMB_CORNER_RADIUS,
    );

    let text_left = cover_point.x + THUMB_WIDTH as i32 + CONTINUE_TILE_TEXT_GAP;
    let text_right = top_left.x + width - CONTINUE_TILE_PAD;
    let text_width = (text_right - text_left).max(0);

    // This tile is Home's one hero element: its title is Large where an
    // ordinary tile label is Heading. The caption/status line is Body, the
    // interface's smallest size.
    let caption_style = state.display.body_style();
    let title_style = state.display.text_style(UiTextRole::Large, BinaryColor::On);
    let status_style = state.display.body_style();

    // Top group: eyebrow caption, then the (possibly multi-line) title —
    // both anchored to the tile's top edge rather than centered, matching
    // the reference layout. `cursor_top` tracks the top edge of whatever
    // comes next; each block converts it to a text baseline by adding its
    // own line step, so a caption that wraps to two lines simply pushes the
    // title down instead of overlapping it.
    let mut cursor_top = top_left.y + CONTINUE_TILE_PAD;

    let caption = t(locale, "CONTINUE READING", "CONTINUA");
    let caption_line_step = i32::from(caption_style.line_height());
    // The caption shares the tile's top edge with the selection dot: it
    // stays clear of the dot whether or not the dot is showing, so selecting
    // the tile never reflows it.
    let dot_left =
        top_left.x + width - CONTINUE_TILE_DOT_INSET - CONTINUE_TILE_DOT_SIZE.width as i32;
    let caption_width = (dot_left - CONTINUE_TILE_CAPTION_DOT_GAP - text_left).clamp(0, text_width);
    for line in wrap_to_width(
        caption_style,
        caption,
        caption_width,
        CONTINUE_TILE_CAPTION_MAX_LINES,
    ) {
        let baseline = cursor_top + caption_line_step;
        Text::new(&line, Point::new(text_left, baseline), caption_style).draw(display)?;
        cursor_top += caption_line_step;
    }

    let book = state.reader.continue_reading_book();
    let title = book.as_ref().map_or_else(
        || {
            // With no book on the card at all, the card says what SELECT
            // does: it opens Upload (see `AppState::activate_continue_reading`).
            if state.library_known_empty {
                t(locale, "Add your first book", "Aggiungi il primo libro")
            } else {
                t(locale, "No saved book", "Nessun libro salvato")
            }
            .to_string()
        },
        |book| book.title.clone(),
    );
    cursor_top += CONTINUE_TILE_TITLE_GAP;
    let title_line_step = i32::from(title_style.line_height());
    for line in wrap_to_width(
        title_style,
        &title,
        text_width,
        CONTINUE_TILE_TITLE_MAX_LINES,
    ) {
        let baseline = cursor_top + title_line_step;
        Text::new(&line, Point::new(text_left, baseline), title_style).draw(display)?;
        cursor_top += title_line_step;
    }

    // Bottom group: the "how much is left" status text directly above a
    // full-width progress bar, both anchored to the tile's bottom edge. The
    // status text can wrap to more than one line (see
    // `continue_reading_status_lines`), so its block height is measured
    // first and the progress bar stays pinned to the tile's bottom edge
    // regardless of how many lines came out.
    if let Some(percent) = state.reader.continue_reading_percent() {
        let lines = continue_reading_status_lines(state, percent, status_style, text_width);
        let line_step = i32::from(status_style.line_height());
        let status_block_height = lines.len() as i32 * line_step;
        let progress_top =
            top_left.y + CONTINUE_TILE_HEIGHT - CONTINUE_TILE_PAD - CONTINUE_TILE_PROGRESS_HEIGHT;
        let mut baseline =
            progress_top - CONTINUE_TILE_STATUS_GAP - status_block_height + line_step;
        for line in &lines {
            Text::new(line, Point::new(text_left, baseline), status_style).draw(display)?;
            baseline += line_step;
        }
        draw_continue_reading_progress_bar(display, text_left, text_right, progress_top, percent)?;
    }

    Ok(())
}

/// Status lines shown above the Continue Reading progress bar: the
/// remaining-time estimate first (the thing users check most, per product
/// direction — see the Reading Stats badge redesign), then the chapter/
/// percent line, e.g.:
/// ```text
/// circa 3h 20m rimanenti
/// Cap. 12 - 60%
/// ```
/// Wrapped to `max_width` at `style` (a readable body-sized style, not
/// truncated) rather than a single ellipsis-truncated line, since the
/// remaining-time clause can run long enough to need it. The remaining-time
/// line is only present once a reading speed has actually been recorded
/// (see `state.reading_stats`, refreshed on entering this screen); the
/// chapter/percent line is always present given a known `percent`. Used by
/// [`draw_continue_reading_tile`], the Home dashboard's Continue Reading
/// card.
pub(crate) fn continue_reading_status_lines(
    state: &AppState,
    percent: u8,
    style: UiTextStyle,
    max_width: i32,
) -> Vec<String> {
    let locale = state.regional.locale;
    let mut lines = Vec::new();

    if let Some(seconds) = state.reading_stats.remaining_book_seconds {
        let remaining_text = format!(
            "{} {} {}",
            t(locale, "about", "circa"),
            crate::reading_stats::format_duration_seconds(seconds),
            t(locale, "left", "rimanenti"),
        );
        lines.extend(wrap_to_width(style, &remaining_text, max_width, 2));
    }

    let percent_label = format!("{}%", percent.min(100));
    let chapter_number = continue_reading_chapter_number(state);
    lines.push(chapter_number.map_or_else(
        || percent_label.clone(),
        |number| format!("Cap. {number} - {percent_label}"),
    ));
    lines
}

/// Current chapter number for the Continue Reading badge, preferring the
/// current chapter from an active session and falling back to the one
/// stashed on the resume location. `None` for a TXT book or an EPUB with no
/// structured navigation, in which case the badge shows only the percentage.
fn continue_reading_chapter_number(state: &AppState) -> Option<usize> {
    let reader = &state.reader;
    reader
        .session
        .as_ref()
        .and_then(|session| session.current_epub_chapter_page_label())
        .map(|chapter| chapter.chapter_number)
        .or_else(|| {
            reader
                .resume
                .as_ref()
                .and_then(|resume| resume.epub_chapter.as_ref())
                .map(|chapter| chapter.chapter_number)
        })
}

/// Reading-progress track + fill, spanning the Continue Reading tile's full
/// text-column width (the percentage is printed separately, on the status
/// line above — see [`continue_reading_status_label`]). Mirrors the Reader
/// page's own full-width `draw_reading_progress` (`screens/reader.rs`),
/// duplicated rather than shared since the two draw into unrelated bounds (a
/// tile's text column here vs. the whole screen width there).
fn draw_continue_reading_progress_bar(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    right: i32,
    top: i32,
    percent: u8,
) -> Result<(), Infallible> {
    let track_radii = CornerRadii::new(Size::new(
        CONTINUE_TILE_PROGRESS_HEIGHT as u32 / 2,
        CONTINUE_TILE_PROGRESS_HEIGHT as u32 / 2,
    ));
    RoundedRectangle::new(
        Rectangle::new(
            Point::new(left, top),
            Size::new(
                (right - left).max(0) as u32,
                CONTINUE_TILE_PROGRESS_HEIGHT as u32,
            ),
        ),
        track_radii,
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
    .draw(display)?;

    let inner_width = (right - left - 4).max(0);
    let fill_width = inner_width * i32::from(percent.min(100)) / 100;
    if fill_width > 0 {
        let fill_height = (CONTINUE_TILE_PROGRESS_HEIGHT - 4).max(0);
        RoundedRectangle::new(
            Rectangle::new(
                Point::new(left + 2, top + 2),
                Size::new(fill_width as u32, fill_height as u32),
            ),
            CornerRadii::new(Size::new(fill_height as u32 / 2, fill_height as u32 / 2)),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(display)?;
    }
    Ok(())
}

/// Greedy pixel-width word-wrap to at most `max_lines` lines. Words past
/// `max_lines` are simply dropped rather
/// than forcing an ellipsis, since a book title overrunning two lines here is
/// rare enough that "good enough" wins over exact-fit complexity.
fn wrap_to_width(style: UiTextStyle, text: &str, max_width: i32, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if !current.is_empty() && style.text_width(&candidate) > max_width {
            lines.push(current);
            current = word.to_string();
            if lines.len() >= max_lines {
                return lines;
            }
        } else {
            current = candidate;
        }
    }
    if lines.len() < max_lines && !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Left margin shared with the Home grid.
const TILE_GRID_LEFT: i32 = 22;
/// First row's top, directly below the shared product header — tight like
/// the Library grid's own top, now that tiles are compact rather than full
/// size.
const TILE_GRID_TOP: i32 = 46;
/// Bottom edge content must stay clear of. Matches the y every other screen's
/// footer separator sits at (`widgets::footer::draw_footer`), even though
/// this screen draws no footer of its own, so the grid's bottom margin still
/// reads consistent with the rest of the shell.
const TILE_GRID_BOTTOM: i32 = 736;
/// Tiles per row.
const TILE_GRID_COLUMNS: usize = 3;

/// Compact icon + title tile grid, matching the Home dashboard's grid, used
/// for Settings. Tiles keep the order of `menu::category_entries`, so each
/// one is always in the same place. Rows that do not fit go on further
/// pages, and the page shown is the one holding the selection.
fn render_tile_grid(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    route: ScreenRoute,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let selected = state.category_selection(route);
    let title = route.label_i18n(locale).to_ascii_uppercase();

    draw_header(display, state, &title)?;

    let row_height = COMPACT_TILE_SIZE.height as i32 + TILE_GAP_Y;
    let rows_per_page = ((TILE_GRID_BOTTOM - TILE_GRID_TOP) / row_height).max(1) as usize;
    let per_page = rows_per_page * TILE_GRID_COLUMNS;
    let first = selected / per_page * per_page;
    for (index, entry) in category_entries(route)
        .iter()
        .copied()
        .enumerate()
        .skip(first)
        .take(per_page)
    {
        let slot = index - first;
        let x = TILE_GRID_LEFT
            + (slot % TILE_GRID_COLUMNS) as i32 * (COMPACT_TILE_SIZE.width as i32 + TILE_GAP_X);
        let y = TILE_GRID_TOP + (slot / TILE_GRID_COLUMNS) as i32 * row_height;
        draw_home_tile_compact(
            display,
            Point::new(x, y),
            entry,
            index == selected,
            state.display,
            locale,
        )?;
    }

    Ok(())
}
