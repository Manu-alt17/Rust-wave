//! Reader landing, library, bookmarks, loading, TXT / EPUB page, TOC and options screens.

use core::convert::Infallible;

use embedded_graphics::{
    image::{Image, ImageRaw},
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{
        Circle, CornerRadii, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle, RoundedRectangle,
        Triangle,
    },
};

use crate::{
    app::{
        reader_typography::reader_body_style,
        state::AppState,
        typography::{Text, TextBounds, UiTextStyle},
        widgets::{
            footer::draw_footer,
            header::draw_header,
            status_glyphs::{draw_battery_icon, BATTERY_SIZE},
        },
    },
    cover_cache::{CachedThumbnail, THUMB_HEIGHT, THUMB_WIDTH},
    orientation::{DisplayOrientation, OrientedFrameBuffer},
    reader::{
        eligible_word_spans, ParagraphAlignment, ReaderBook, ReaderCachedPage,
        ReaderDictionaryMode, ReaderLoadingStage, ReaderOption, ReaderSession, ReadingPreference,
        ReadingTheme,
    },
};

pub fn render_continue_reading(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    draw_header(display, state, "CONTINUE READING")?;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    if let Some(session) = state.reader.session.as_ref() {
        Text::new(
            &truncate(&session.book.title, 38),
            Point::new(24, 186),
            heading,
        )
        .draw(display)?;
        Text::new(
            &format!(
                "Runtime page {} is ready.",
                session.current_absolute_page() + 1
            ),
            Point::new(24, 236),
            body,
        )
        .draw(display)?;
        Text::new("SELECT resumes the open page.", Point::new(24, 280), body).draw(display)?;
    } else if let Some(resume) = state.reader.resume.as_ref() {
        Text::new(&truncate(&resume.title, 38), Point::new(24, 186), heading).draw(display)?;
        Text::new(
            &format!("Saved page {} is ready to restore.", resume.page_index + 1),
            Point::new(24, 236),
            body,
        )
        .draw(display)?;
        Text::new(
            "SELECT loads the saved position.",
            Point::new(24, 280),
            body,
        )
        .draw(display)?;
    } else {
        Text::new("No saved book", Point::new(24, 186), heading).draw(display)?;
        Text::new(
            "Open Library and choose a TXT book.",
            Point::new(24, 236),
            body,
        )
        .draw(display)?;
        Text::new(
            "The last-read page is stored on the SD card.",
            Point::new(24, 280),
            body,
        )
        .draw(display)?;
    }
    draw_footer(display, state.display, "SELECT RESUME  BOOT BACK")
}

/// Left margin of the cover grid, matching the Home dashboard grid's margin.
const LIBRARY_GRID_LEFT: i32 = 18;
/// Top of the first grid row, just below the shared header divider.
const LIBRARY_GRID_TOP: i32 = 100;
/// Last pixel row cells may occupy, leaving room for the footer divider.
const LIBRARY_GRID_BOTTOM: i32 = 660;
/// Covers per row. There are no Left/Right buttons on this hardware — only
/// Up/Down/Select — so the grid is really one linear selection index walked
/// in raster order, the same trick the Home dashboard and Reader category
/// grids already use: Down from the top-left cell lands on top-right (next
/// index), not on the cell below.
const LIBRARY_GRID_COLUMNS: usize = 2;
/// Cell width: `LIBRARY_GRID_LEFT` on both sides plus `LIBRARY_GRID_GAP_X`
/// between the two columns fills the full 480px logical width exactly
/// (18 + 218 + 8 + 218 + 18 = 480).
const LIBRARY_CELL_WIDTH: i32 = 218;
const LIBRARY_GRID_GAP_X: i32 = 8;
const LIBRARY_GRID_GAP_Y: i32 = 16;
/// Cell height, fixed regardless of content so every row of the grid lines
/// up the same. `THUMB_HEIGHT` plus a `LIBRARY_COVER_PAD`-sized margin on
/// top and bottom fills it; the sliver left over doubles as the fallback
/// title band for books with no real cover.
const LIBRARY_CELL_HEIGHT: i32 = THUMB_HEIGHT as i32 + LIBRARY_COVER_PAD * 2;
/// Empty space kept between the cover thumbnail and the selection border on
/// every side, so the border stays visible instead of hugging the cover art.
const LIBRARY_COVER_PAD: i32 = 5;

/// Index range `[first, last)` of entries visible in the current grid page,
/// given the selection. Shared by `render_library`'s drawing pass and
/// [`library_visible_books`]'s background-thumbnail visibility window, so
/// both agree on exactly what is on-panel right now.
fn library_grid_window(entry_count: usize, selected: usize) -> (usize, usize) {
    if entry_count == 0 {
        return (0, 0);
    }
    let selected = selected.min(entry_count - 1);
    let total_rows = entry_count.div_ceil(LIBRARY_GRID_COLUMNS);
    let available = LIBRARY_GRID_BOTTOM - LIBRARY_GRID_TOP;
    // Rows fit when `n * height + (n - 1) * gap <= available`, rearranged to
    // avoid an off-by-one from a naive `available / stride` (which would
    // wrongly assume every row, including the last, needs a trailing gap).
    let rows_capacity = ((available + LIBRARY_GRID_GAP_Y)
        / (LIBRARY_CELL_HEIGHT + LIBRARY_GRID_GAP_Y))
        .max(1) as usize;
    let rows_capacity = rows_capacity.min(total_rows.max(1));
    let selected_row = selected / LIBRARY_GRID_COLUMNS;
    let first_row = if selected_row < rows_capacity {
        0
    } else {
        selected_row + 1 - rows_capacity
    };
    let first_index = first_row * LIBRARY_GRID_COLUMNS;
    let last_index = (first_index + rows_capacity * LIBRARY_GRID_COLUMNS).min(entry_count);
    (first_index, last_index)
}

/// Books currently on-panel in the Library grid, in the same raster order
/// `render_library` draws. Used to bound background thumbnail generation
/// ([`crate::cover_cache::CoverCache::pump_pending`]) to what is actually
/// visible ("generate thumbnails lazily, only for the current page").
#[must_use]
pub fn library_visible_books(state: &AppState) -> Vec<ReaderBook> {
    let reader = &state.reader;
    let entries = reader.visible_entries();
    let (first, last) = library_grid_window(entries.len(), reader.library_selected);
    entries[first..last]
        .iter()
        .map(|entry| entry.book.clone())
        .collect()
}

pub fn render_library(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let reader = &state.reader;
    let body = state.display.body_style();
    draw_header(display, state, "LIBRARY")?;

    let entries = reader.visible_entries();
    if entries.is_empty() {
        let message = reader
            .library_error
            .as_deref()
            .unwrap_or("Copy TXT or EPUB books into /RUSTMIX/BOOKS.");
        Text::new(&truncate(message, 54), Point::new(26, 148), body).draw(display)?;
        return draw_footer(display, state.display, "MOVE  SELECT OPEN  BOOT BACK");
    }

    let (first, last) = library_grid_window(entries.len(), reader.library_selected);
    for (offset, entry) in entries[first..last].iter().enumerate() {
        let index = first + offset;
        let row = offset / LIBRARY_GRID_COLUMNS;
        let column = offset % LIBRARY_GRID_COLUMNS;
        let top_left = Point::new(
            LIBRARY_GRID_LEFT + column as i32 * (LIBRARY_CELL_WIDTH + LIBRARY_GRID_GAP_X),
            LIBRARY_GRID_TOP + row as i32 * (LIBRARY_CELL_HEIGHT + LIBRARY_GRID_GAP_Y),
        );
        let thumbnail = reader.library_thumbnails.get(&entry.book.path);
        let progress_percent = reader.library_progress_percent(&entry.book);
        draw_library_cell(
            display,
            state,
            top_left,
            index == reader.library_selected,
            thumbnail,
            &entry.book.title,
            progress_percent,
        )?;
    }

    draw_footer(display, state.display, "MOVE  SELECT OPEN  BOOT BACK")
}

/// One Library grid cell: a bordered tile (selection shown purely by border
/// weight, matching the Home dashboard grid) holding just the cover
/// thumbnail — the cover art already carries the title, so a book with a
/// confirmed real cover shows no text at all. A book with no usable cover
/// (still pending generation, or genuinely placeholder because the EPUB has
/// none / it's a TXT book) falls back to showing the title, since the
/// generic placeholder glyph alone can't identify which book it is.
fn draw_library_cell(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    top_left: Point,
    selected: bool,
    thumbnail: Option<&CachedThumbnail>,
    title: &str,
    progress_percent: Option<u8>,
) -> Result<(), Infallible> {
    let border = if selected {
        PrimitiveStyle::with_stroke(BinaryColor::On, 4)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    Rectangle::new(
        top_left,
        Size::new(LIBRARY_CELL_WIDTH as u32, LIBRARY_CELL_HEIGHT as u32),
    )
    .into_styled(border)
    .draw(display)?;

    // Inset by `LIBRARY_COVER_PAD` on every side, so the selection border
    // stays visibly separated from the cover art instead of hugging it.
    let thumb_point = Point::new(top_left.x + LIBRARY_COVER_PAD, top_left.y + LIBRARY_COVER_PAD);
    let has_real_cover = thumbnail.is_some_and(|thumbnail| !thumbnail.placeholder);
    if let Some(thumbnail) = thumbnail {
        if display.orientation() == DisplayOrientation::Portrait {
            // Fast path: the Library screen only ever runs in Portrait (see
            // `AppState::sync_orientation_for_active_route`), so this
            // skips embedded-graphics' generic per-pixel `Image` draw for
            // what is otherwise a near-full-cell bitmap blit on every single
            // button press.
            display.blit_packed_bitmap_portrait(
                thumb_point,
                thumbnail.width,
                thumbnail.height,
                &thumbnail.bits,
            );
        } else {
            let raw = ImageRaw::<BinaryColor>::new(&thumbnail.bits, u32::from(thumbnail.width));
            Image::new(&raw, thumb_point).draw(display)?;
        }
    } else {
        // Not generated yet: a thin outline placeholder. `pump_pending`
        // fills the real thumbnail in on a later main-loop tick and the
        // cell is redrawn then.
        Rectangle::new(
            thumb_point,
            Size::new(u32::from(THUMB_WIDTH), u32::from(THUMB_HEIGHT)),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;
    }

    if !has_real_cover {
        let detail = state.display.detail_style();
        let baseline = top_left.y + LIBRARY_CELL_HEIGHT - 10;
        Text::new(&truncate(title, 22), Point::new(top_left.x + 8, baseline), detail)
            .draw(display)?;
    }

    if let Some(percent) = progress_percent {
        draw_library_progress_badge(display, state, thumb_point, percent)?;
    }
    Ok(())
}

/// Gap kept between the cover's own edges and the progress badge, on both
/// the top and the left side, so the badge reads as a pill floating just
/// inside the corner of the art rather than flush with it.
const LIBRARY_BADGE_EDGE_GAP: i32 = 6;
/// Horizontal padding between the percentage text and the pill's sides.
const LIBRARY_BADGE_PAD_X: i32 = 10;
/// Vertical padding between the percentage text and the pill's top/bottom.
const LIBRARY_BADGE_PAD_Y: i32 = 4;

/// Reading-completion badge: a fully rounded ("pill") white tag, outlined so
/// it stays legible over dark cover art and opaque so it always sits cleanly
/// on top of the cover image beneath it regardless of what ink the art has
/// there. Anchored to the cover's top-left corner with left-aligned text —
/// centering a short, variable-width label ("7%" vs "100%") inside the pill
/// made it look off-center from one glyph's side bearing to the next, so a
/// fixed left inset reads as more precisely aligned than true centering did.
fn draw_library_progress_badge(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    thumb_point: Point,
    percent: u8,
) -> Result<(), Infallible> {
    let style = state.display.body_style();
    let label = format!("{}%", percent.min(100));
    let text_width = style.text_width(&label);
    // The pill is sized to this label's actual ink, not the font's full
    // line pitch (`line_height`), which reserves room for descenders no
    // digit or '%' glyph has — using it here would center the ink into the
    // pill's top half and leave the bottom padding looking too generous.
    let (ink_top, ink_bottom) = style.text_ink_bounds(&label);
    let ink_height = ink_bottom - ink_top;

    let badge_width = text_width + LIBRARY_BADGE_PAD_X * 2;
    let badge_height = ink_height + LIBRARY_BADGE_PAD_Y * 2;
    let badge_left = thumb_point.x + LIBRARY_BADGE_EDGE_GAP;
    let badge_top = thumb_point.y + LIBRARY_BADGE_EDGE_GAP;

    let badge_style = PrimitiveStyleBuilder::new()
        .stroke_color(BinaryColor::On)
        .stroke_width(1)
        .fill_color(BinaryColor::Off)
        .build();
    let pill_radius = Size::new(badge_height as u32 / 2, badge_height as u32 / 2);
    RoundedRectangle::new(
        Rectangle::new(
            Point::new(badge_left, badge_top),
            Size::new(badge_width as u32, badge_height as u32),
        ),
        CornerRadii::new(pill_radius),
    )
    .into_styled(badge_style)
    .draw(display)?;

    let text_left = badge_left + LIBRARY_BADGE_PAD_X;
    let baseline = badge_top + LIBRARY_BADGE_PAD_Y - ink_top;
    Text::new(&label, Point::new(text_left, baseline), style).draw(display)?;
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LibraryEntryColumns {
    badge: String,
    suffix: String,
}

fn bookmark_entry_columns(
    reader: &crate::reader::ReaderUiState,
    bookmark: &crate::reader::ReaderLocation,
) -> LibraryEntryColumns {
    if let Some(chapter) = reader.bookmark_display_chapter_page(bookmark) {
        LibraryEntryColumns {
            badge: format!("CH {}", chapter.chapter_number),
            suffix: format!("P {}", chapter.page_text()),
        }
    } else {
        LibraryEntryColumns {
            badge: "PAGE".into(),
            suffix: reader.bookmark_display_page(bookmark).to_string(),
        }
    }
}

pub fn render_bookmarks(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    draw_header(display, state, "BOOKMARKS")?;
    let body = state.display.body_style();
    if state.reader.bookmarks.is_empty() {
        Text::new(
            "No saved bookmarks",
            Point::new(24, 164),
            state.display.heading_style(),
        )
        .draw(display)?;
        Text::new(
            "Open a Reader page, choose Reader Options,",
            Point::new(24, 218),
            body,
        )
        .draw(display)?;
        Text::new(
            "then select Add / Remove Bookmark.",
            Point::new(24, 260),
            body,
        )
        .draw(display)?;
    } else {
        for (index, bookmark) in state.reader.bookmarks.iter().take(8).enumerate() {
            let top = 118 + index as i32 * 64;
            let columns = bookmark_entry_columns(&state.reader, bookmark);
            draw_row(
                display,
                state,
                top,
                state.reader.bookmarks_selected == index,
                &truncate(&bookmark.title, 23),
                columns.badge.as_str(),
                columns.suffix.as_str(),
            )?;
        }
    }
    draw_footer(display, state.display, "MOVE  SELECT OPEN  BOOT BACK")
}

pub fn render_loading(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    draw_header(display, state, "OPENING BOOK")?;
    let body = state.display.body_style();
    let heading = state.display.heading_style();
    let loading = state.reader.loading.as_ref();
    let title = loading.map_or("Book", |value| value.book.title.as_str());
    let stage = loading.map_or(ReaderLoadingStage::OpeningFile, |value| value.stage);
    Text::new(&truncate(title, 36), Point::new(24, 172), heading).draw(display)?;
    Text::new(stage.label(), Point::new(24, 234), body).draw(display)?;
    draw_progress(display, stage.progress())?;
    let message = loading.map_or("Preparing reader...", |value| value.message.as_str());
    Text::new(&truncate(message, 52), Point::new(24, 352), body).draw(display)?;
    Text::new(
        "The current page opens before full indexing.",
        Point::new(24, 406),
        body,
    )
    .draw(display)?;
    draw_footer(display, state.display, "BOOT CANCEL")
}

/// Top margin above the reading-progress bar, in logical pixels.
const PROGRESS_TOP: i32 = 12;
/// Height of the reading-progress track, in logical pixels.
const PROGRESS_HEIGHT: i32 = 8;
/// Gap between the reading-progress bar and the first line of book text.
const PROGRESS_TO_CONTENT_GAP: i32 = 14;

pub fn render_page(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let Some(session) = state.reader.session.as_ref() else {
        return render_continue_reading(display, state);
    };
    let size = display.orientation().logical_size();
    let width = size.width as i32;
    let height = size.height as i32;
    let content_top = PROGRESS_TOP + PROGRESS_HEIGHT + PROGRESS_TO_CONTENT_GAP;
    let footer_line = height - 54;
    let body = ReaderBodyGeometry::new(width, content_top, footer_line);
    let body_style = reader_body_style(
        state.reader.preferences.book_font,
        state.reader.preferences.font_size,
        state.reader.preferences.theme,
    );

    draw_reading_progress(display, state, session, width)?;

    if state.reader.preferences.theme == ReadingTheme::HighContrast {
        Rectangle::new(
            Point::new(body.frame.left, body.frame.top),
            Size::new(body.frame.width() as u32, body.frame.height() as u32),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2))
        .draw(display)?;
    }

    if let Some(page) = session.current_cached_page() {
        let line_step = i32::from(body_style.line_height()) + 2;
        let first_baseline = body.text.top + i32::from(body_style.line_height());
        for (index, line) in page
            .lines
            .iter()
            .take(session.layout.lines_per_page)
            .enumerate()
        {
            let baseline = first_baseline + index as i32 * line_step;
            if baseline >= body.text.bottom {
                break;
            }
            let (rendered, left) = aligned_reader_line(
                line.text.as_str(),
                line.paragraph_end,
                session.layout.paragraph_alignment,
                body_style,
                body.text,
            );
            Text::new(rendered.as_str(), Point::new(left, baseline), body_style)
                .draw_clipped(display, body.text)?;
        }
        draw_dictionary_mode_overlay(display, state, page, &body, body_style, session)?;
    } else {
        let baseline = body.text.top + i32::from(body_style.line_height());
        Text::new(
            "Preparing page...",
            Point::new(body.text.left, baseline),
            body_style,
        )
        .draw_clipped(display, body.text)?;
    }

    draw_reader_footer(display, state, width, height, footer_line)
}

/// Reading-progress row that replaces the Reader page's old title bar: a
/// minimal track spanning the full width, filled to the current position,
/// with the percentage printed at its right end. Keeping this the only
/// element above the book text reclaims the header/title rows for content.
fn draw_reading_progress(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    session: &crate::reader::ReaderSession,
    width: i32,
) -> Result<(), Infallible> {
    let style = state.display.detail_style();
    let percent_label = session.reading_percent_label();
    let percent_width = style.text_width(&percent_label);
    let track_left = 14;
    let track_right = (width - 14 - 10 - percent_width).max(track_left + 4);

    Rectangle::new(
        Point::new(track_left, PROGRESS_TOP),
        Size::new((track_right - track_left) as u32, PROGRESS_HEIGHT as u32),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
    .draw(display)?;

    if let Some(percent) = session.reading_percent() {
        let inner_width = (track_right - track_left - 4).max(0);
        let fill_width = inner_width * i32::from(percent.min(100)) / 100;
        if fill_width > 0 {
            Rectangle::new(
                Point::new(track_left + 2, PROGRESS_TOP + 2),
                Size::new(fill_width as u32, (PROGRESS_HEIGHT - 4).max(0) as u32),
            )
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;
        }
    }

    Text::new(
        &percent_label,
        Point::new(track_right + 10, PROGRESS_TOP + PROGRESS_HEIGHT),
        style,
    )
    .draw(display)?;
    Ok(())
}

/// Line-select cursor glyph, kept a full word-space clear of the text it
/// points at so it doesn't read as part of the line.
const LINE_MARKER_GLYPH: &str = ">";
/// Gap between the marker's right edge and the first character of the line.
const LINE_MARKER_GAP: i32 = 10;

/// In-page dictionary lookup mode, drawn on top of the already-rendered book
/// text: a `>` cursor next to the selected line, an underline beneath the
/// selected word once a line is confirmed, and a compact definition panel
/// once a word is confirmed. A hold-SELECT press toggles the whole mode; see
/// `AppState::apply_reader_dictionary_select_long_press`.
fn draw_dictionary_mode_overlay(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    page: &ReaderCachedPage,
    body: &ReaderBodyGeometry,
    body_style: UiTextStyle,
    session: &ReaderSession,
) -> Result<(), Infallible> {
    let (line_index, word_index, definition) = match &state.reader.dictionary_mode {
        ReaderDictionaryMode::Off => return Ok(()),
        ReaderDictionaryMode::LineSelect { line_index } => {
            let line_step = i32::from(body_style.line_height()) + 2;
            let first_baseline = body.text.top + i32::from(body_style.line_height());
            let baseline = first_baseline + *line_index as i32 * line_step;
            let marker_left =
                body.text.left - LINE_MARKER_GAP - body_style.text_width(LINE_MARKER_GLYPH);
            return Text::new(
                LINE_MARKER_GLYPH,
                Point::new(marker_left, baseline),
                body_style,
            )
            .draw(display)
            .map(|_| ());
        }
        ReaderDictionaryMode::WordSelect {
            line_index,
            word_index,
        } => (*line_index, *word_index, None),
        ReaderDictionaryMode::Definition {
            line_index,
            word_index,
            word,
            message,
        } => (
            *line_index,
            *word_index,
            Some((word.as_str(), message.as_str())),
        ),
    };

    let Some(line) = page.lines.get(line_index) else {
        return Ok(());
    };
    let line_step = i32::from(body_style.line_height()) + 2;
    let first_baseline = body.text.top + i32::from(body_style.line_height());
    let baseline = first_baseline + line_index as i32 * line_step;
    let (rendered, left) = aligned_reader_line(
        line.text.as_str(),
        line.paragraph_end,
        session.layout.paragraph_alignment,
        body_style,
        body.text,
    );
    if let Some(&(start, end)) = eligible_word_spans(&rendered).get(word_index) {
        let word_left = left + body_style.text_width(&rendered[..start]);
        let word_width = body_style.text_width(&rendered[start..end]).max(1);
        Rectangle::new(
            Point::new(word_left, baseline + 3),
            Size::new(word_width as u32, 2),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(display)?;
    }

    if let Some((word, definition)) = definition {
        draw_dictionary_definition_panel(display, state, body, word, definition)?;
    }
    Ok(())
}

/// Compact word + definition panel shown once a word is confirmed, drawn
/// over the bottom of the reading body so it never collides with the
/// progress bar or footer.
fn draw_dictionary_definition_panel(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    body: &ReaderBodyGeometry,
    word: &str,
    message: &str,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let detail = state.display.detail_style();
    let panel_top = (body.text.bottom - DEFINITION_PANEL_HEIGHT).max(body.text.top);
    let panel = Rectangle::new(
        Point::new(body.frame.left, panel_top),
        Size::new(
            body.frame.width() as u32,
            (body.text.bottom - panel_top) as u32,
        ),
    );
    panel
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::Off))
        .draw(display)?;
    panel
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2))
        .draw(display)?;

    let text_left = body.frame.left + 12;
    Text::new(
        &truncate(word, 30),
        Point::new(text_left, panel_top + 24),
        heading,
    )
    .draw(display)?;

    for (index, line) in wrap_definition_lines(message, 42, DEFINITION_PANEL_LINES)
        .iter()
        .enumerate()
    {
        Text::new(
            line,
            Point::new(text_left, panel_top + 50 + index as i32 * 22),
            detail,
        )
        .draw(display)?;
    }
    Ok(())
}

/// Sized for up to `DEFINITION_PANEL_LINES` wrapped lines below the word
/// heading -- long dictionary definitions (device-side capped at 220 chars,
/// see `compact_definition` in src/dictionary.rs) need more than 3 lines to
/// display in full.
const DEFINITION_PANEL_HEIGHT: i32 = 220;
const DEFINITION_PANEL_LINES: usize = 7;

fn wrap_definition_lines(value: &str, max_chars: usize, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in value.split_whitespace() {
        if !current.is_empty() && current.len() + 1 + word.len() > max_chars {
            lines.push(current);
            current = String::new();
            if lines.len() >= max_lines {
                break;
            }
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if lines.len() < max_lines && !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Reader page footer: compact button hints (an up/down arrow glyph for
/// page turns, a dot glyph for the options shortcut) on the left, freeing up
/// room to show battery and clock — both dropped from the old header — on
/// the right. Wi-Fi and the date are not shown while reading.
fn draw_reader_footer(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    width: i32,
    height: i32,
    footer_line: i32,
) -> Result<(), Infallible> {
    let style = state.display.detail_style();
    let color = BinaryColor::On;
    let baseline = height - 18;

    Rectangle::new(
        Point::new(14, footer_line),
        Size::new((width - 28) as u32, 1),
    )
    .into_styled(PrimitiveStyle::with_fill(color))
    .draw(display)?;

    let (up_label, down_label, select_label, hold_label) = match &state.reader.dictionary_mode {
        ReaderDictionaryMode::Off => ("Prev", "Next", "Options", "Hold:Dict"),
        ReaderDictionaryMode::LineSelect { .. } => ("Line", "Line", "Pick line", "Hold:Exit"),
        ReaderDictionaryMode::WordSelect { .. } => ("Word", "Word", "Look up", "Hold:Exit"),
        ReaderDictionaryMode::Definition { .. } => ("", "", "Next word", "Hold:Exit"),
    };

    let mut cursor_x = 18;
    if !up_label.is_empty() {
        draw_up_arrow(display, cursor_x, baseline, color)?;
        cursor_x += ARROW_SIZE + ICON_TEXT_GAP;
        let cursor = Text::new(up_label, Point::new(cursor_x, baseline), style).draw(display)?;
        cursor_x = cursor.x + FOOTER_GROUP_GAP;
    }

    if !down_label.is_empty() {
        draw_down_arrow(display, cursor_x, baseline, color)?;
        cursor_x += ARROW_SIZE + ICON_TEXT_GAP;
        let cursor = Text::new(down_label, Point::new(cursor_x, baseline), style).draw(display)?;
        cursor_x = cursor.x + FOOTER_GROUP_GAP;
    }

    draw_dot(display, cursor_x, baseline, color)?;
    cursor_x += DOT_SIZE + ICON_TEXT_GAP;
    let cursor = Text::new(select_label, Point::new(cursor_x, baseline), style).draw(display)?;
    cursor_x = cursor.x + FOOTER_GROUP_GAP;
    Text::new(hold_label, Point::new(cursor_x, baseline), style).draw(display)?;

    let time_label = state.status_time_label();
    let battery_percent = state.battery_percent();
    let battery_label =
        battery_percent.map_or_else(|| "--".to_string(), |percent| format!("{percent}%"));

    let mut right_x = width - 18 - style.text_width(&time_label);
    Text::new(&time_label, Point::new(right_x, baseline), style).draw(display)?;

    right_x -= 10 + style.text_width(&battery_label);
    Text::new(&battery_label, Point::new(right_x, baseline), style).draw(display)?;

    right_x -= 6 + BATTERY_SIZE.width as i32;
    draw_battery_icon(
        display,
        Point::new(right_x, baseline - BATTERY_SIZE.height as i32 + 3),
        battery_percent,
        color,
    )?;

    Ok(())
}

/// Side length of the triangular up/down page-turn glyphs in the footer.
const ARROW_SIZE: i32 = 9;
/// Diameter of the round "options" glyph in the footer.
const DOT_SIZE: i32 = 8;
/// Gap between a footer glyph and the label that follows it.
const ICON_TEXT_GAP: i32 = 6;
/// Gap between one footer hint group and the next.
const FOOTER_GROUP_GAP: i32 = 16;

/// Upward-pointing triangle (page-turn "previous") sitting on `baseline`.
fn draw_up_arrow(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    baseline: i32,
    color: BinaryColor,
) -> Result<(), Infallible> {
    let top = baseline - ARROW_SIZE;
    Triangle::new(
        Point::new(left + ARROW_SIZE / 2, top),
        Point::new(left, baseline),
        Point::new(left + ARROW_SIZE, baseline),
    )
    .into_styled(PrimitiveStyle::with_fill(color))
    .draw(display)
}

/// Downward-pointing triangle (page-turn "next") sitting on `baseline`.
fn draw_down_arrow(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    baseline: i32,
    color: BinaryColor,
) -> Result<(), Infallible> {
    let top = baseline - ARROW_SIZE;
    Triangle::new(
        Point::new(left, top),
        Point::new(left + ARROW_SIZE, top),
        Point::new(left + ARROW_SIZE / 2, baseline),
    )
    .into_styled(PrimitiveStyle::with_fill(color))
    .draw(display)
}

/// Filled circle (Reader options shortcut) sitting on `baseline`.
fn draw_dot(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    baseline: i32,
    color: BinaryColor,
) -> Result<(), Infallible> {
    Circle::new(Point::new(left, baseline - DOT_SIZE), DOT_SIZE as u32)
        .into_styled(PrimitiveStyle::with_fill(color))
        .draw(display)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReaderBodyGeometry {
    text: TextBounds,
    frame: ReaderFrameBounds,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReaderFrameBounds {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl ReaderFrameBounds {
    #[must_use]
    const fn width(self) -> i32 {
        self.right - self.left
    }

    #[must_use]
    const fn height(self) -> i32 {
        self.bottom - self.top
    }
}

impl ReaderBodyGeometry {
    /// Shared Reader body rectangle used by Classic and High Contrast. The
    /// stronger High Contrast frame stays outside this viewport, so switching
    /// themes never changes TXT pagination or cache fingerprints. The left
    /// and right margins match `crate::reader::READER_BODY_MARGIN_PX`, which
    /// TXT/EPUB pagination also uses to compute `ReaderLayout::available_width_px`
    /// -- keeping the two in sync is what lets pagination wrap lines to fill
    /// exactly the width drawn here.
    #[must_use]
    const fn new(width: i32, content_top: i32, footer_line: i32) -> Self {
        let margin = crate::reader::READER_BODY_MARGIN_PX;
        let text = TextBounds::new(margin, content_top, width - margin, footer_line - 12);
        let frame = ReaderFrameBounds {
            left: text.left - 8,
            top: text.top - 8,
            right: text.right + 8,
            bottom: text.bottom + 8,
        };
        Self { text, frame }
    }
}

pub fn render_options(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    draw_header(display, state, "READER OPTIONS")?;
    for (index, option) in ReaderOption::ALL.iter().copied().enumerate() {
        let badge = match option {
            ReaderOption::Bookmark if state.reader.current_page_is_bookmarked() => "REMOVE",
            ReaderOption::Bookmark => "ADD",
            ReaderOption::Bookmarks => "LIST",
            ReaderOption::TableOfContents if state.reader.has_structured_toc() => "LIST",
            _ => option.badge(),
        };
        draw_row(
            display,
            state,
            142 + index as i32 * 66,
            state.reader.options_selected == index,
            option.label(),
            badge,
            "",
        )?;
    }
    draw_footer(display, state.display, "MOVE  SELECT ACTIVATE  BOOT BACK")
}

pub fn render_preferences(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    draw_header(display, state, "READING PREFERENCES")?;
    for (index, preference) in ReadingPreference::ALL.iter().copied().enumerate() {
        let badge = match preference {
            ReadingPreference::ReadingTheme => state.reader.preferences.theme.label(),
            ReadingPreference::Orientation => state.reader.preferences.orientation.label(),
            ReadingPreference::BookFontSize => state.reader.preferences.font_size.label(),
            ReadingPreference::BookFont => state.reader.preferences.book_font.label(),
            ReadingPreference::ParagraphAlignment => {
                state.reader.preferences.paragraph_alignment.label()
            }
            ReadingPreference::ShowProgress if state.reader.preferences.show_progress => "On",
            ReadingPreference::ShowProgress => "Off",
        };
        draw_row(
            display,
            state,
            152 + index as i32 * 78,
            state.reader.preferences_selected == index,
            preference.label(),
            badge,
            "",
        )?;
    }
    draw_footer(
        display,
        state.display,
        "UP/DOWN MOVE  SELECT CHANGE  BOOT BACK",
    )
}

pub fn render_toc(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    draw_header(display, state, "TABLE OF CONTENTS")?;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let toc = state.reader.toc_entries();
    if toc.is_empty() {
        Text::new("No structured TOC", Point::new(24, 196), heading).draw(display)?;
        Text::new(
            "Ordinary TXT files do not provide a formal",
            Point::new(24, 254),
            body,
        )
        .draw(display)?;
        Text::new(
            "table of contents. EPUB books expose their",
            Point::new(24, 296),
            body,
        )
        .draw(display)?;
        Text::new(
            "navigation entries on this screen.",
            Point::new(24, 338),
            body,
        )
        .draw(display)?;
        return draw_footer(display, state.display, "BOOT BACK");
    }

    let first = state.reader.toc_selected.saturating_sub(7);
    for (row, entry) in toc.iter().skip(first).take(8).enumerate() {
        let index = first + row;
        draw_row(
            display,
            state,
            120 + row as i32 * 64,
            state.reader.toc_selected == index,
            &truncate(&entry.label, 27),
            "CH",
            &(entry.spine_index + 1).to_string(),
        )?;
    }
    draw_footer(display, state.display, "MOVE  SELECT OPEN  BOOT BACK")
}

fn aligned_reader_line(
    line: &str,
    paragraph_end: bool,
    alignment: ParagraphAlignment,
    style: crate::app::typography::UiTextStyle,
    bounds: TextBounds,
) -> (String, i32) {
    let width = style.text_width(line);
    let available = bounds.width().max(0);
    match alignment {
        ParagraphAlignment::Left => (line.into(), bounds.left),
        ParagraphAlignment::Center => (line.into(), bounds.left + (available - width).max(0) / 2),
        ParagraphAlignment::Right => (line.into(), bounds.left + (available - width).max(0)),
        ParagraphAlignment::Justified if !paragraph_end => {
            (justify_reader_line(line, style, available), bounds.left)
        }
        ParagraphAlignment::Justified => (line.into(), bounds.left),
    }
}

fn justify_reader_line(
    line: &str,
    style: crate::app::typography::UiTextStyle,
    available: i32,
) -> String {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.len() < 2 {
        return line.into();
    }
    let base = words.join(" ");
    let space = style.text_width(" ").max(1);
    let extra_spaces = ((available - style.text_width(base.as_str())).max(0) / space) as usize;
    let gaps = words.len() - 1;
    let mut output = String::new();
    for (index, word) in words.iter().enumerate() {
        output.push_str(word);
        if index < gaps {
            let remainder = if index < extra_spaces % gaps { 1 } else { 0 };
            let count = 1 + extra_spaces / gaps + remainder;
            output.extend(core::iter::repeat(' ').take(count));
        }
    }
    output
}

fn draw_row(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    top: i32,
    selected: bool,
    label: &str,
    badge: &str,
    suffix: &str,
) -> Result<(), Infallible> {
    let body = state.display.body_style();
    let style = if selected {
        PrimitiveStyle::with_stroke(BinaryColor::On, 4)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    Rectangle::new(Point::new(20, top), Size::new(440, 50))
        .into_styled(style)
        .draw(display)?;
    Text::new(
        if selected { ">" } else { " " },
        Point::new(32, top + 32),
        body,
    )
    .draw(display)?;
    Text::new(label, Point::new(58, top + 32), body).draw(display)?;
    Text::new(badge, Point::new(338, top + 32), body).draw(display)?;
    Text::new(suffix, Point::new(402, top + 32), body).draw(display)?;
    Ok(())
}

fn draw_progress(display: &mut OrientedFrameBuffer<'_>, percent: u8) -> Result<(), Infallible> {
    Rectangle::new(Point::new(24, 278), Size::new(432, 38))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2))
        .draw(display)?;
    let width = 4 * percent as u32;
    Rectangle::new(Point::new(30, 284), Size::new(width.min(420), 26))
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(display)?;
    Ok(())
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.into();
    }
    let mut output: String = value.chars().take(max_chars.saturating_sub(3)).collect();
    output.push_str("...");
    output
}

#[cfg(test)]
mod tests {
    use super::{
        aligned_reader_line, bookmark_entry_columns, library_visible_books, render_bookmarks,
        render_continue_reading, render_library, render_loading, render_options,
        render_preferences, render_toc, ReaderBodyGeometry,
    };
    use crate::{
        app::AppState,
        cover_cache::{CachedThumbnail, THUMB_HEIGHT, THUMB_WIDTH},
        framebuffer::FrameBuffer,
        orientation::OrientedFrameBuffer,
        reader::{
            BookFormat, ParagraphAlignment, PendingReaderOpen, ReaderBook, ReaderChapterPageLabel,
            ReaderLoadingStage, ReaderLocation,
        },
    };

    #[test]
    fn high_contrast_frame_stays_outside_shared_text_viewport() {
        let body = ReaderBodyGeometry::new(480, 90, 746);
        assert!(body.frame.left < body.text.left);
        assert!(body.frame.top < body.text.top);
        assert!(body.frame.right > body.text.right);
        assert!(body.frame.bottom > body.text.bottom);
        assert_eq!(body.text.left, 24);
        assert_eq!(body.text.right, 456);
    }

    #[test]
    fn epub_bookmark_columns_show_chapter_and_chapter_page_total() {
        let bookmark = ReaderLocation {
            path: "NOVEL.EPU".into(),
            title: "Novel".into(),
            format: BookFormat::Epub,
            size_bytes: 123,
            modified_seconds: 456,
            byte_offset: 789,
            page_index: 11,
            epub_chapter: Some(ReaderChapterPageLabel {
                chapter_number: 4,
                page_number: 3,
                page_count: 12,
            }),
            reading_percent: None,
        };
        let reader = crate::reader::ReaderUiState::default();
        assert_eq!(
            bookmark_entry_columns(&reader, &bookmark),
            super::LibraryEntryColumns {
                badge: "CH 4".into(),
                suffix: "P 3/12".into(),
            }
        );
    }

    #[test]
    fn reader_screens_render_without_sd_card() {
        let mut state = AppState::default();
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        render_continue_reading(&mut display, &state).unwrap();
        render_library(&mut display, &state).unwrap();
        render_bookmarks(&mut display, &state).unwrap();
        render_options(&mut display, &state).unwrap();
        render_preferences(&mut display, &state).unwrap();
        render_toc(&mut display, &state).unwrap();
        state.reader.loading = Some(PendingReaderOpen {
            book: ReaderBook {
                path: "a.txt".into(),
                title: "A".into(),
                format: BookFormat::Text,
                size_bytes: 1,
                modified_seconds: 0,
            },
            stage: ReaderLoadingStage::OpeningFile,
            encoding: None,
            epub_document: None,
            resume: None,
            message: "Preparing".into(),
            epub_document_cache_pending: false,
        });
        render_loading(&mut display, &state).unwrap();
    }

    fn epub_book(index: usize) -> ReaderBook {
        ReaderBook {
            path: format!("book{index}.epub"),
            title: format!("Book {index}"),
            format: BookFormat::Epub,
            size_bytes: 10,
            modified_seconds: 1,
        }
    }

    #[test]
    fn library_visible_books_windows_around_the_selection_in_scroll_order() {
        let mut state = AppState::default();
        state.reader.books = (0..20).map(epub_book).collect();

        state.reader.library_selected = 0;
        let visible_from_top = library_visible_books(&state);
        assert!(!visible_from_top.is_empty());
        assert!(visible_from_top.len() < state.reader.books.len());
        assert_eq!(visible_from_top[0].path, "book0.epub");

        // Scrolling to the last book must bring it into the visible window
        // (the whole point of the scroll-into-view windowing) without
        // pulling every other book along with it.
        state.reader.library_selected = state.reader.books.len() - 1;
        let visible_at_end = library_visible_books(&state);
        assert!(visible_at_end
            .iter()
            .any(|book| book.path == "book19.epub"));
        assert!(visible_at_end.len() < state.reader.books.len());
    }

    #[test]
    fn library_visible_books_is_empty_for_an_empty_library() {
        let state = AppState::default();
        assert!(library_visible_books(&state).is_empty());
    }

    #[test]
    fn render_library_draws_a_cached_thumbnail_when_present() {
        let mut state = AppState::default();
        state.reader.books = vec![epub_book(0)];
        state.reader.library_thumbnails.insert(
            "book0.epub".into(),
            CachedThumbnail {
                width: THUMB_WIDTH,
                height: THUMB_HEIGHT,
                bits: vec![0u8; (THUMB_WIDTH as usize / 8) * THUMB_HEIGHT as usize],
                placeholder: false,
            },
        );
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        render_library(&mut display, &state).unwrap();
    }
    #[test]
    fn paragraph_alignment_moves_or_justifies_reader_lines_inside_bounds() {
        let style = AppState::default().display.body_style();
        let bounds = crate::app::typography::TextBounds::new(20, 0, 220, 100);
        let (_, left) =
            aligned_reader_line("short line", true, ParagraphAlignment::Left, style, bounds);
        let (_, center) = aligned_reader_line(
            "short line",
            true,
            ParagraphAlignment::Center,
            style,
            bounds,
        );
        let (_, right) =
            aligned_reader_line("short line", true, ParagraphAlignment::Right, style, bounds);
        assert!(left < center);
        assert!(center < right);
        let (justified, _) = aligned_reader_line(
            "one two three",
            false,
            ParagraphAlignment::Justified,
            style,
            bounds,
        );
        assert!(justified.len() > "one two three".len());
    }
}
