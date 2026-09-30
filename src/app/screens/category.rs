//! Category screens (Tools, Settings) drawn as icon-tile grids, plus the
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
        menu::MenuEntry,
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

    // One role larger than the equivalent text elsewhere in the shell
    // (Body instead of Detail for the caption/status line, Large instead of
    // Heading for the title): this tile is Home's one hero element, so its
    // text reads a size up from ordinary tile labels.
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

    let caption = t(locale, "CONTINUE READING", "CONTINUA A LEGGERE");
    let caption_line_step = i32::from(caption_style.line_height());
    for line in wrap_to_width(
        caption_style,
        caption,
        text_width,
        CONTINUE_TILE_CAPTION_MAX_LINES,
    ) {
        let baseline = cursor_top + caption_line_step;
        Text::new(&line, Point::new(text_left, baseline), caption_style).draw(display)?;
        cursor_top += caption_line_step;
    }

    let book = state.reader.continue_reading_book();
    let title = book.as_ref().map_or_else(
        || t(locale, "No saved book", "Nessun libro salvato").to_string(),
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
/// Gap between a section header's baseline and its underline.
const TILE_GRID_HEADER_UNDERLINE_GAP: i32 = 3;
/// Gap kept below a section header's underline before its first tile row.
const TILE_GRID_HEADER_BELOW_GAP: i32 = 10;

/// A vertical slice of the Settings grid's layout: either a section header
/// or one row of up to [`TILE_GRID_COLUMNS`] tiles. `start`/`count` index
/// into `category_entries(ScreenRoute::Settings)`.
#[derive(Clone, Copy, Debug)]
enum TileGridBlock {
    Header { primary: bool },
    Row { start: usize, count: usize },
}

/// Font-metric-dependent block heights, computed once per frame since line
/// height varies with the user's chosen text size.
struct TileGridMetrics {
    header_height: i32,
    row_height: i32,
}

fn tile_grid_metrics(state: &AppState) -> TileGridMetrics {
    let heading_line = i32::from(state.display.heading_style().line_height());
    TileGridMetrics {
        header_height: heading_line + TILE_GRID_HEADER_UNDERLINE_GAP + 1 + TILE_GRID_HEADER_BELOW_GAP,
        row_height: COMPACT_TILE_SIZE.height as i32 + TILE_GAP_Y,
    }
}

fn tile_grid_block_height(block: &TileGridBlock, metrics: &TileGridMetrics) -> i32 {
    match block {
        TileGridBlock::Header { .. } => metrics.header_height,
        TileGridBlock::Row { .. } => metrics.row_height,
    }
}

/// Lays `entry_count` entries out as tile rows, optionally split into a
/// "Most used" header + rows and an "Other" header + rows the way the
/// Settings grid uses it (mirroring `screens::reader::library_blocks`'s
/// Reading Now / Recent split). `primary_count == 0` means the category has
/// no such split (Tools before anything was opened, say) — just one plain run of rows with
/// no section captions at all.
fn tile_grid_blocks(entry_count: usize, primary_count: usize) -> Vec<TileGridBlock> {
    let mut blocks = Vec::new();
    if primary_count == 0 {
        push_tile_grid_rows(&mut blocks, 0, entry_count);
        return blocks;
    }
    blocks.push(TileGridBlock::Header { primary: true });
    push_tile_grid_rows(&mut blocks, 0, primary_count);
    if entry_count > primary_count {
        blocks.push(TileGridBlock::Header { primary: false });
        push_tile_grid_rows(&mut blocks, primary_count, entry_count);
    }
    blocks
}

fn push_tile_grid_rows(blocks: &mut Vec<TileGridBlock>, start: usize, end: usize) {
    let mut index = start;
    while index < end {
        let count = (end - index).min(TILE_GRID_COLUMNS);
        blocks.push(TileGridBlock::Row { start: index, count });
        index += count;
    }
}

/// Splits `blocks` into screen-height pages. Unlike the Library grid's book
/// count (unbounded, so it needs a scrolling, selection-following window —
/// see `screens::reader::library_window`), Settings has a fixed, small entry
/// count, so a simple greedy pack into pages is enough: no scrollbar, same
/// "just changes page" behaviour `render_category`'s list already has. A
/// page that would otherwise start mid-section (its header packed onto the
/// previous page) gets that header re-added, so the section caption is never
/// missing from view — there's always headroom to spare once a row's worth
/// of height is no longer being counted against the same page.
fn tile_grid_pages(
    blocks: &[TileGridBlock],
    metrics: &TileGridMetrics,
    available: i32,
) -> Vec<Vec<TileGridBlock>> {
    let mut pages: Vec<Vec<TileGridBlock>> = Vec::new();
    let mut current: Vec<TileGridBlock> = Vec::new();
    let mut current_height = 0;
    let mut last_header: Option<TileGridBlock> = None;

    for block in blocks.iter().copied() {
        let height = tile_grid_block_height(&block, metrics);
        if !current.is_empty() && current_height + height > available {
            pages.push(std::mem::take(&mut current));
            current_height = 0;
            if matches!(block, TileGridBlock::Row { .. }) {
                if let Some(header) = last_header {
                    current.push(header);
                    current_height += tile_grid_block_height(&header, metrics);
                }
            }
        }
        if matches!(block, TileGridBlock::Header { .. }) {
            last_header = Some(block);
        }
        current.push(block);
        current_height += height;
    }
    if !current.is_empty() {
        pages.push(current);
    }
    pages
}

/// Index of the page in `pages` whose row range covers `selected`, so
/// Up/Down navigation past a page boundary lands on the right page without
/// any paging state of its own — same "derive the page from the selection"
/// approach `render_category`'s list uses.
fn tile_grid_page_for_selection(pages: &[Vec<TileGridBlock>], selected: usize) -> usize {
    pages
        .iter()
        .position(|page| {
            page.iter().any(|block| {
                matches!(block, TileGridBlock::Row { start, count } if selected >= *start && selected < *start + *count)
            })
        })
        .unwrap_or(0)
}

/// Compact icon + title tile grid, matching the Home dashboard's grid —
/// used for both Settings and Tools, grouped under "Most used" / "Other"
/// section headers. "Most used" holds up to
/// [`crate::app::menu::MOST_USED_MAX`] entries opened most recently (see
/// [`crate::app::menu::CategoryUsage`]), so the ones actually reached for
/// aren't buried among the rest.
fn render_tile_grid(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    route: ScreenRoute,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let entries = state.category_usage.ordered_entries(route);
    let selected = state.category_selection(route);
    let title = route.label_i18n(locale).to_ascii_uppercase();

    draw_header(display, state, &title)?;

    let primary_count = state.category_usage.most_used_count(route).min(entries.len());
    let blocks = tile_grid_blocks(entries.len(), primary_count);
    let metrics = tile_grid_metrics(state);
    let available = TILE_GRID_BOTTOM - TILE_GRID_TOP;
    let pages = tile_grid_pages(&blocks, &metrics, available);
    let page_index = tile_grid_page_for_selection(&pages, selected);
    let empty: Vec<TileGridBlock> = Vec::new();
    let page = pages.get(page_index).unwrap_or(&empty);

    let mut cursor_y = TILE_GRID_TOP;
    for block in page {
        match *block {
            TileGridBlock::Header { primary } => {
                let label = if primary {
                    t(locale, "MOST USED", "PIÙ USATE")
                } else {
                    t(locale, "OTHER", "ALTRO")
                };
                draw_tile_grid_section_header(display, state, cursor_y, label)?;
            }
            TileGridBlock::Row { start, count } => {
                for offset in 0..count {
                    let index = start + offset;
                    let entry: MenuEntry = entries[index];
                    let x = TILE_GRID_LEFT
                        + offset as i32 * (COMPACT_TILE_SIZE.width as i32 + TILE_GAP_X);
                    draw_home_tile_compact(
                        display,
                        Point::new(x, cursor_y),
                        entry,
                        index == selected,
                        state.display,
                        locale,
                    )?;
                }
            }
        }
        cursor_y += tile_grid_block_height(block, &metrics);
    }

    Ok(())
}

/// Section header: an uppercase caption with a full-width underline beneath
/// it, matching `screens::reader::draw_library_section_header`'s look.
fn draw_tile_grid_section_header(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    top: i32,
    label: &str,
) -> Result<(), Infallible> {
    let style = state.display.heading_style();
    let baseline = top + i32::from(style.line_height());
    Text::new(label, Point::new(TILE_GRID_LEFT, baseline), style).draw(display)?;
    let underline_top = baseline + TILE_GRID_HEADER_UNDERLINE_GAP;
    let underline_width = COMPACT_TILE_SIZE.width as i32 * TILE_GRID_COLUMNS as i32
        + TILE_GAP_X * (TILE_GRID_COLUMNS as i32 - 1);
    Rectangle::new(
        Point::new(TILE_GRID_LEFT, underline_top),
        Size::new(underline_width as u32, 1),
    )
    .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
    .draw(display)?;
    Ok(())
}
