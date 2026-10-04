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

use embedded_iconoir::{
    icons::{
        size24px::{
            editor::{AlignCenter, AlignJustify, AlignLeft, AlignRight},
            navigation::{FastArrowDownBox, FastArrowRightBox},
        },
        size48px::{
            activities::{BookmarkBook, Percentage},
            editor::List,
            organization::BookmarkEmpty,
            system::Settings as SettingsIcon,
        },
    },
    prelude::IconoirNewIcon,
};

use crate::{
    app::{
        i18n::t,
        reader_typography::reader_body_style,
        state::AppState,
        typography::{Text, TextBounds, UiTextStyle},
        widgets::{
            footer::{
                back_action, back_only, draw_footer, draw_footer_paged, footer_hints,
                select_and_back, FooterKey,
            },
            header::draw_header,
            home_tile::{
                draw_icon_tile, draw_iconoir_icon, COMPACT_TILE_SIZE, TILE_GAP_X, TILE_GAP_Y,
            },
            layout::{
                CONTENT_BOTTOM, CONTENT_LEFT, CONTENT_RIGHT, CONTENT_WIDTH, FIRST_BASELINE,
                FIRST_ROW_TOP, SCREEN_WIDTH,
            },
            list::{
                centered_baseline, draw_list_row, draw_row_frame, draw_section_title, page_window,
                ROW_GAP, ROW_HEIGHT, ROW_PAD_X, ROW_STEP,
            },
            status_glyphs::{draw_battery_icon, BATTERY_SIZE},
            text::{
                draw_paragraph, draw_text_centered, draw_text_fit, truncate_to_width,
                wrap_to_width, ELLIPSIS,
            },
        },
    },
    cover_cache::{CachedThumbnail, THUMB_HEIGHT, THUMB_WIDTH},
    orientation::{DisplayOrientation, OrientedFrameBuffer},
    reader::{
        eligible_word_spans, BookFont, BookFontSize, LibraryBookAction, ParagraphAlignment,
        ReaderBook, ReaderCachedPage, ReaderDictionaryMode, ReaderLoadingStage, ReaderLocation,
        ReaderOption, ReaderOrientation, ReaderPreferences, ReaderSession, ReaderUiState,
        ReadingPreference, ReadingTheme, READER_BODY_MARGIN_PX,
    },
    regional::Locale,
};

pub fn render_continue_reading(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    draw_header(display, state, t(locale, "CONTINUE", "CONTINUA"))?;
    let (title, detail, action) = if let Some(session) = state.reader.session.as_ref() {
        let page = session.current_absolute_page() + 1;
        (
            session.book.title.as_str(),
            match locale {
                Locale::English => format!("You are on page {page}."),
                Locale::Italian => format!("Sei a pagina {page}."),
            },
            t(locale, "RESUME", "RIPRENDI"),
        )
    } else if let Some(resume) = state.reader.resume.as_ref() {
        let page = resume.page_index + 1;
        (
            resume.title.as_str(),
            match locale {
                Locale::English => format!("Saved position: page {page}."),
                Locale::Italian => format!("Posizione salvata: pagina {page}."),
            },
            t(locale, "RESUME", "RIPRENDI"),
        )
    } else {
        (
            t(locale, "No book in progress", "Nessun libro in lettura"),
            t(
                locale,
                "Open the Library and choose a book. The last page read is saved on the SD card.",
                "Apri la Libreria e scegli un libro. L'ultima pagina letta viene salvata sulla scheda SD.",
            )
            .to_string(),
            t(locale, "LIBRARY", "LIBRERIA"),
        )
    };
    let next = draw_paragraph(
        display,
        title,
        CONTENT_LEFT,
        FIRST_BASELINE,
        preferences.heading_style(),
        CONTENT_WIDTH,
        3,
        2,
    )?;
    draw_paragraph(
        display,
        &detail,
        CONTENT_LEFT,
        next + 8,
        preferences.body_style(),
        CONTENT_WIDTH,
        4,
        6,
    )?;
    draw_footer(display, state, &select_and_back(locale, action))
}

/// Left margin of the cover grid, matching the Home dashboard grid's margin.
const LIBRARY_GRID_LEFT: i32 = 14;
/// Top of the first section header. Matches the gap the Home dashboard uses
/// right below the shared header (`home::CARD_TOP`) instead of leaving extra
/// breathing room above the first "IN LETTURA" caption.
const LIBRARY_GRID_TOP: i32 = 46;
/// Last pixel row cells may occupy, leaving room for the footer's "Tieni ●
/// Opzioni" hold-SELECT hint below it ([`draw_library_footer`]). The "more
/// below" chevron ([`draw_library_scroll_hint`]) lives inside the scrollbar
/// gutter at the bottom of the track, not below `LIBRARY_GRID_BOTTOM`, so it
/// doesn't need extra room reserved for it here.
const LIBRARY_GRID_BOTTOM: i32 = 736;
/// Covers per row. There are no Left/Right buttons on this hardware — only
/// Up/Down/Select — so the grid is really one linear selection index walked
/// in raster order, the same trick the Home dashboard grid already uses:
/// Down from the top-left cell lands on top-right (next index), not on the
/// cell below.
const LIBRARY_GRID_COLUMNS: usize = 2;
/// Cell width: `LIBRARY_GRID_LEFT` on the left, `LIBRARY_GRID_GAP_X` between
/// the two columns, and a scrollbar gutter on the right fill the full 480px
/// logical width exactly (14 + 218 + 6 + 218 = 456, leaving 24px for
/// [`draw_library_scrollbar`]).
const LIBRARY_CELL_WIDTH: i32 = 218;
const LIBRARY_GRID_GAP_X: i32 = 6;
/// Gap kept below every row (including the last one in a section) before
/// whatever comes next — another row or the following section's header.
const LIBRARY_GRID_GAP_Y: i32 = 8;
/// Empty space kept between the cover thumbnail and the cell's selection
/// border / text column on every side.
const LIBRARY_COVER_PAD: i32 = 5;
/// Corner radius for a Library cell's border, matching the Home dashboard's
/// Continue Reading tile (`CONTINUE_TILE_CORNER_RADIUS` in
/// `screens/category.rs`) so both cards read as the same rounded style.
const LIBRARY_CELL_CORNER_RADIUS: Size = Size::new(16, 16);
/// Corner radius the cover thumbnail itself is masked to (see
/// [`draw_library_cell`]) — a little smaller than
/// `LIBRARY_CELL_CORNER_RADIUS` so the cover's curve reads as concentric
/// with the cell border around it rather than flatter or sharper than the
/// frame it sits inside `LIBRARY_COVER_PAD` from.
const LIBRARY_THUMB_CORNER_RADIUS: i32 = 12;
/// Height of just the cover-art portion of a cell; the status row sits below
/// it (see [`library_metrics`]) — no title line any more, so a cell is just
/// the cover plus one status row, trading "the cover art already carries the
/// title" for a shorter cell that fits more rows on-panel.
const LIBRARY_COVER_BLOCK_HEIGHT: i32 = THUMB_HEIGHT as i32 + LIBRARY_COVER_PAD * 2;
/// Gap between the cover art and the status row below it.
const LIBRARY_BAR_GAP: i32 = 6;
/// Gap kept between the status row and the cell's bottom border, so the
/// label's text doesn't sit flush against it.
const LIBRARY_CELL_BOTTOM_PAD: i32 = 6;
/// Height of the status bar/track itself, matching Home's Continue Reading
/// card (`category::CONTINUE_TILE_PROGRESS_HEIGHT`) so the two screens'
/// progress bars read as the same element.
const LIBRARY_BAR_HEIGHT: i32 = 10;
/// Horizontal gap between the bar's right edge and its label (percentage /
/// "COMPLETATO" / "NUOVO"), which sits beside the bar on the same line
/// rather than on a line of its own — one fewer line per cell than stacking
/// them, freeing enough height for a second row of covers per section.
const LIBRARY_BAR_LABEL_GAP: i32 = 8;
/// Horizontal breathing room kept between the bar's rounded ends and the
/// text column's own left/right margins (`left`/`right` in
/// [`draw_library_status_row`]) — otherwise the pill-shaped bar (see
/// [`LIBRARY_BAR_HEIGHT`]) reads as touching the cell border on the left and
/// crowding the label on the right, both of which the flush-fit sharp
/// rectangle this replaced never showed.
const LIBRARY_BAR_INSET: i32 = 3;
/// Gap between a section header's baseline and its underline.
const LIBRARY_HEADER_UNDERLINE_GAP: i32 = 3;
/// Gap kept below a section header's underline before the first row —
/// deliberately more generous than the header's own internal spacing, so the
/// underline reads as separating the caption from the books below it rather
/// than sitting arbitrarily close to both.
const LIBRARY_HEADER_BELOW_GAP: i32 = 10;

/// A cell's reading-status, driving both its status bar's fill style and the
/// label drawn under it. `InProgress` carries the live percentage; the other
/// two states are binary, so there's nothing left to show but the label.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LibraryCellStatus {
    InProgress(u8),
    Completed,
    New,
}

/// One book plus the reading-status its cell should draw.
struct LibraryGridEntry {
    book: ReaderBook,
    status: LibraryCellStatus,
}

/// A vertical slice of the Library screen's layout: either a section header
/// or one row of up to [`LIBRARY_GRID_COLUMNS`] book cells. `start`/`count`
/// index into the flat entry list [`library_grid_entries`] returns.
#[derive(Clone, Copy, Debug)]
enum LibraryBlock {
    Header { in_progress: bool },
    Row { start: usize, count: usize },
}

/// Font-metric-dependent block heights, computed once per frame from the
/// active display preferences rather than hardcoded, since line height
/// varies with the user's chosen text size.
struct LibraryMetrics {
    header_height: i32,
    row_height: i32,
}

/// Splits the Library grid's books into "Reading Now" (Recent entries with
/// *some* real progress: 1-99%) and "Recent" (never-opened books and Recent
/// entries still at 0%, then finished Recent entries last), in the exact
/// order [`crate::reader::ReaderUiState::visible_entries`] walks for
/// keyboard navigation — so `library_selected`, which indexes into that same
/// order, always lands on the cell actually drawn at that index. A Recent
/// entry sitting at 0% (opened once but never actually read past the first
/// page) reads as "New" rather than "Reading Now" — a saved position record
/// alone isn't "in progress" until it has a percentage to show for it.
/// Within "Recent", never-opened / 0% books come first and finished ones
/// last — a book still waiting to be started is more actionable than one
/// already closed out, so it gets the space closer to the top instead of
/// being pushed down by completed books.
/// Returns the flat entry list plus how many of its leading entries are
/// "Reading Now".
fn library_grid_entries(reader: &crate::reader::ReaderUiState) -> (Vec<LibraryGridEntry>, usize) {
    // Same classification and order as `ReaderUiState::visible_entries`,
    // which `library_selected` indexes into: books outside Recent use their
    // saved position as well, instead of always showing as "New".
    let (recent, other) = reader.library_sections();
    let mut in_progress = Vec::new();
    let mut new = Vec::new();
    let mut completed = Vec::new();
    for entry in recent.into_iter().chain(other) {
        let percent = entry
            .location
            .as_ref()
            .map_or(0, crate::reader::ReaderLocation::reading_percent_estimate);
        if percent >= 100 {
            completed.push(LibraryGridEntry {
                book: entry.book,
                status: LibraryCellStatus::Completed,
            });
        } else if percent == 0 {
            new.push(LibraryGridEntry {
                book: entry.book,
                status: LibraryCellStatus::New,
            });
        } else {
            in_progress.push(LibraryGridEntry {
                book: entry.book,
                status: LibraryCellStatus::InProgress(percent),
            });
        }
    }
    let in_progress_count = in_progress.len();
    let mut entries = in_progress;
    entries.extend(new);
    entries.extend(completed);
    (entries, in_progress_count)
}

/// Lays `entry_count` entries (the first `in_progress_count` of them
/// "Reading Now", the rest "Recent") out as a header + rows per non-empty
/// section, skipping a section entirely when it has no entries (e.g. no book
/// is in progress yet on a fresh library).
fn library_blocks(entry_count: usize, in_progress_count: usize) -> Vec<LibraryBlock> {
    let mut blocks = Vec::new();
    if in_progress_count > 0 {
        blocks.push(LibraryBlock::Header { in_progress: true });
        push_library_rows(&mut blocks, 0, in_progress_count);
    }
    if entry_count > in_progress_count {
        blocks.push(LibraryBlock::Header { in_progress: false });
        push_library_rows(&mut blocks, in_progress_count, entry_count);
    }
    blocks
}

fn push_library_rows(blocks: &mut Vec<LibraryBlock>, start: usize, end: usize) {
    let mut index = start;
    while index < end {
        let count = (end - index).min(LIBRARY_GRID_COLUMNS);
        blocks.push(LibraryBlock::Row {
            start: index,
            count,
        });
        index += count;
    }
}

fn library_metrics(state: &AppState) -> LibraryMetrics {
    let heading_line = i32::from(state.display.heading_style().line_height());
    let label_line = i32::from(state.display.detail_style().line_height());
    // The bar and its label share one line; that line's height is whichever
    // of the two is taller (in practice the label's line pitch, since
    // `LIBRARY_BAR_HEIGHT` is deliberately shorter than a line of text).
    let status_row_height = LIBRARY_BAR_HEIGHT.max(label_line);
    LibraryMetrics {
        header_height: heading_line + LIBRARY_HEADER_UNDERLINE_GAP + 1 + LIBRARY_HEADER_BELOW_GAP,
        row_height: LIBRARY_COVER_BLOCK_HEIGHT
            + LIBRARY_BAR_GAP
            + status_row_height
            + LIBRARY_CELL_BOTTOM_PAD
            + LIBRARY_GRID_GAP_Y,
    }
}

fn library_block_height(block: &LibraryBlock, metrics: &LibraryMetrics) -> i32 {
    match block {
        LibraryBlock::Header { .. } => metrics.header_height,
        LibraryBlock::Row { .. } => metrics.row_height,
    }
}

/// Minimal-scroll window over `blocks` that keeps the row holding `selected`
/// on-panel — the block-based generalization of the old flat-grid
/// `library_grid_window`, needed now that section headers take up variable
/// vertical space alongside the book rows. Grows the window forward from the
/// top, sliding its start forward exactly enough to keep `selected`'s row
/// included whenever it stops fitting, then extends the end further if
/// there's still room left after that.
fn library_window(
    blocks: &[LibraryBlock],
    selected: usize,
    metrics: &LibraryMetrics,
    available: i32,
) -> (usize, usize) {
    if blocks.is_empty() {
        return (0, 0);
    }
    let heights: Vec<i32> = blocks
        .iter()
        .map(|block| library_block_height(block, metrics))
        .collect();
    let total: i32 = heights.iter().sum();
    if total <= available {
        return (0, blocks.len());
    }
    let selected_block = blocks
        .iter()
        .position(|block| {
            matches!(block, LibraryBlock::Row { start, count } if selected >= *start && selected < *start + *count)
        })
        .unwrap_or(0);

    let mut first = 0usize;
    let mut end = 0usize;
    let mut height = 0i32;
    for index in 0..blocks.len() {
        height += heights[index];
        end = index + 1;
        while height > available && first < index {
            height -= heights[first];
            first += 1;
        }
        if index >= selected_block {
            break;
        }
    }
    while end < blocks.len() && height + heights[end] <= available {
        height += heights[end];
        end += 1;
    }

    // Whatever section's rows ended up on-panel, keep its header pinned in
    // view too — otherwise scrolling a few rows into a long section drops
    // its caption off the top and there's no longer any label telling you
    // which section ("Reading Now" vs. "Recent") you're actually looking
    // at. Trims rows off the *end* to make room if needed, but never past
    // the selected one, and only when the header and the selected row can
    // actually fit together at all — deep into a long section, pinning the
    // header would otherwise force the window open past `available` just to
    // span from the top of the section down to the selection, so it's
    // skipped rather than violated in that case.
    if matches!(blocks[first], LibraryBlock::Row { .. }) {
        if let Some(header) = blocks[..first]
            .iter()
            .rposition(|block| matches!(block, LibraryBlock::Header { .. }))
        {
            let min_height: i32 = heights[header..=selected_block].iter().sum();
            if min_height <= available {
                let mut trimmed_end = end;
                let mut trimmed_height: i32 = heights[header..trimmed_end].iter().sum();
                while trimmed_height > available && trimmed_end > selected_block + 1 {
                    trimmed_end -= 1;
                    trimmed_height -= heights[trimmed_end];
                }
                first = header;
                end = trimmed_end;
            }
        }
    }

    (first, end)
}

/// Books currently on-panel in the Library grid, in the same order
/// `render_library` draws. Used to bound background thumbnail generation
/// ([`crate::cover_cache::CoverCache::pump_pending`]) to what is actually
/// visible ("generate thumbnails lazily, only for the current page").
#[must_use]
pub fn library_visible_books(state: &AppState) -> Vec<ReaderBook> {
    let reader = &state.reader;
    let (entries, in_progress_count) = library_grid_entries(reader);
    if entries.is_empty() {
        return Vec::new();
    }
    let blocks = library_blocks(entries.len(), in_progress_count);
    let metrics = library_metrics(state);
    let available = LIBRARY_GRID_BOTTOM - LIBRARY_GRID_TOP;
    let (first_block, last_block) =
        library_window(&blocks, reader.library_selected, &metrics, available);
    blocks[first_block..last_block]
        .iter()
        .filter_map(|block| match *block {
            LibraryBlock::Row { start, count } => Some((start, count)),
            LibraryBlock::Header { .. } => None,
        })
        .flat_map(|(start, count)| {
            entries[start..start + count]
                .iter()
                .map(|entry| entry.book.clone())
        })
        .collect()
}

pub fn render_library(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let reader = &state.reader;
    let preferences = state.display;
    draw_header(display, state, t(locale, "LIBRARY", "LIBRERIA"))?;

    let (entries, in_progress_count) = library_grid_entries(reader);
    if entries.is_empty() {
        let (title, message) = match reader.library_error.as_deref() {
            Some(error) => (
                t(locale, "Library unavailable", "Libreria non disponibile"),
                error,
            ),
            None => (
                t(locale, "No books yet", "Nessun libro"),
                t(
                    locale,
                    "Copy EPUB or TXT books into the /RUSTMIX/BOOKS folder of the SD card, or send them from your phone with Upload.",
                    "Copia libri EPUB o TXT nella cartella /RUSTMIX/BOOKS della scheda SD, oppure inviali dal telefono con Carica.",
                ),
            ),
        };
        let next = draw_section_title(display, preferences, FIRST_BASELINE, title)?;
        draw_paragraph(
            display,
            message,
            CONTENT_LEFT,
            next,
            preferences.body_style(),
            CONTENT_WIDTH,
            6,
            6,
        )?;
        return draw_footer(display, state, &back_only(locale));
    }

    let blocks = library_blocks(entries.len(), in_progress_count);
    let metrics = library_metrics(state);
    let available = LIBRARY_GRID_BOTTOM - LIBRARY_GRID_TOP;
    let (first_block, last_block) =
        library_window(&blocks, reader.library_selected, &metrics, available);

    let mut cursor_y = LIBRARY_GRID_TOP;
    for block in &blocks[first_block..last_block] {
        match *block {
            LibraryBlock::Header { in_progress } => {
                let label = if in_progress {
                    t(locale, "READING NOW", "IN LETTURA")
                } else {
                    t(locale, "RECENT", "RECENTI")
                };
                draw_library_section_header(display, state, cursor_y, label)?;
            }
            LibraryBlock::Row { start, count } => {
                for offset in 0..count {
                    let index = start + offset;
                    let entry = &entries[index];
                    let top_left = Point::new(
                        LIBRARY_GRID_LEFT
                            + offset as i32 * (LIBRARY_CELL_WIDTH + LIBRARY_GRID_GAP_X),
                        cursor_y,
                    );
                    let thumbnail = reader.library_thumbnails.get(&entry.book.path);
                    draw_library_cell(
                        display,
                        state,
                        top_left,
                        metrics.row_height - LIBRARY_GRID_GAP_Y,
                        index == reader.library_selected,
                        thumbnail,
                        &entry.book.title,
                        entry.status,
                    )?;
                }
            }
        }
        cursor_y += library_block_height(block, &metrics);
    }

    let heights: Vec<i32> = blocks
        .iter()
        .map(|block| library_block_height(block, &metrics))
        .collect();
    let total_height: i32 = heights.iter().sum();
    let visible_height: i32 = heights[first_block..last_block].iter().sum();
    let offset_height: i32 = heights[..first_block].iter().sum();
    let more_below = last_block < blocks.len();
    draw_library_scrollbar(
        display,
        LIBRARY_GRID_TOP,
        // The track stops short of the grid's true bottom, leaving room in
        // the scrollbar gutter for the "more below" chevron so the two never
        // overlap — the row grid itself still uses the full height.
        LIBRARY_GRID_BOTTOM - LIBRARY_SCROLLBAR_CHEVRON_RESERVE,
        offset_height,
        visible_height,
        total_height,
    )?;
    if more_below {
        draw_library_scroll_hint(display)?;
    }
    draw_library_footer(display, state)?;

    Ok(())
}

/// Library footer: SELECT opens the book, and holding it is the only way to
/// the per-book actions (mark read or unread, bookmarks, delete), which
/// nothing else on this screen hints at.
fn draw_library_footer(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    draw_footer(
        display,
        state,
        &footer_hints(
            locale,
            &[
                (FooterKey::Select, t(locale, "OPEN", "APRI")),
                (FooterKey::Hold, t(locale, "OPTIONS", "OPZIONI")),
                (FooterKey::Boot, back_action(locale)),
            ],
        ),
    )
}

/// Section header: an uppercase caption with a full-width underline beneath
/// it, matching the two-section "Reading Now" / "Recent" grouping the
/// reference redesign uses in place of one flat, undifferentiated grid.
fn draw_library_section_header(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    top: i32,
    label: &str,
) -> Result<(), Infallible> {
    let style = state.display.heading_style();
    let baseline = top + i32::from(style.line_height());
    Text::new(label, Point::new(LIBRARY_GRID_LEFT, baseline), style).draw(display)?;
    let underline_top = baseline + LIBRARY_HEADER_UNDERLINE_GAP;
    let underline_width = LIBRARY_CELL_WIDTH * 2 + LIBRARY_GRID_GAP_X;
    Rectangle::new(
        Point::new(LIBRARY_GRID_LEFT, underline_top),
        Size::new(underline_width as u32, 1),
    )
    .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
    .draw(display)?;
    Ok(())
}

/// One Library grid cell: a bordered tile (selection shown purely by border
/// weight, matching the Home dashboard grid) holding the cover thumbnail and
/// a status bar + label reporting whether it's still being read, finished,
/// or never opened. No title line under the cover — the cover art already
/// carries it, and dropping the line buys back its height in every cell. A
/// book without a usable cover gets its title written on the placeholder
/// instead (see [`draw_placeholder_title`]).
#[allow(clippy::too_many_arguments)]
fn draw_library_cell(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    top_left: Point,
    cell_height: i32,
    selected: bool,
    thumbnail: Option<&CachedThumbnail>,
    title: &str,
    status: LibraryCellStatus,
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
            Size::new(LIBRARY_CELL_WIDTH as u32, cell_height as u32),
        ),
        CornerRadii::new(LIBRARY_CELL_CORNER_RADIUS),
    )
    .into_styled(border)
    .draw(display)?;

    // Inset by `LIBRARY_COVER_PAD` on every side, so the selection border
    // stays visibly separated from the cover art instead of hugging it.
    let thumb_point = Point::new(
        top_left.x + LIBRARY_COVER_PAD,
        top_left.y + LIBRARY_COVER_PAD,
    );
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
    // Whitens the cover's four corners so it reads as rounded, matching the
    // cell border's own `LIBRARY_CELL_CORNER_RADIUS`. Done here at draw
    // time rather than baked into the cached thumbnail bitmap: baking it in
    // would need bumping the cache format version, which invalidates and
    // regenerates every cover on SD (real JPEG/PNG decode work) the next
    // time the Library is opened. This costs only a bounded handful of
    // pixel writes per cell instead.
    display.mask_rounded_corners(
        thumb_point,
        Size::new(u32::from(THUMB_WIDTH), u32::from(THUMB_HEIGHT)),
        LIBRARY_THUMB_CORNER_RADIUS,
    );
    if thumbnail.is_none_or(|thumbnail| thumbnail.placeholder) {
        draw_placeholder_title(display, state, thumb_point, title)?;
    }

    let text_left = top_left.x + LIBRARY_COVER_PAD;
    let text_right = top_left.x + LIBRARY_CELL_WIDTH - LIBRARY_COVER_PAD;
    let bar_row_top = top_left.y + LIBRARY_COVER_BLOCK_HEIGHT + LIBRARY_BAR_GAP;
    let bar_row_height =
        cell_height - LIBRARY_COVER_BLOCK_HEIGHT - LIBRARY_BAR_GAP - LIBRARY_CELL_BOTTOM_PAD;
    draw_library_status_row(
        display,
        state,
        text_left,
        text_right,
        bar_row_top,
        bar_row_height,
        status,
        locale,
    )?;
    Ok(())
}

/// Most lines of title a placeholder cover shows.
const PLACEHOLDER_TITLE_LINES: usize = 5;

/// The title of a book without a usable cover, on a white label across the
/// middle of its placeholder, so the cell still says which book it is.
fn draw_placeholder_title(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    cover: Point,
    title: &str,
) -> Result<(), Infallible> {
    let style = state.display.heading_style();
    // Clear of the placeholder's spine line on the left.
    let (margin_left, margin_right, padding) = (16, 10, 8);
    let text_width = i32::from(THUMB_WIDTH) - margin_left - margin_right - 2 * padding;
    let lines = wrap_to_width(style, title.trim(), text_width, PLACEHOLDER_TITLE_LINES);
    if lines.is_empty() {
        return Ok(());
    }
    let line_height = i32::from(style.line_height());
    let label_height = line_height * lines.len() as i32 + 2 * padding;
    let label = Rectangle::new(
        Point::new(
            cover.x + margin_left,
            cover.y + (i32::from(THUMB_HEIGHT) - label_height) / 2,
        ),
        Size::new((text_width + 2 * padding) as u32, label_height as u32),
    );
    label
        .into_styled(
            PrimitiveStyleBuilder::new()
                .fill_color(BinaryColor::Off)
                .stroke_color(BinaryColor::On)
                .stroke_width(1)
                .build(),
        )
        .draw(display)?;
    // Baselines: the font's ascent is most of its line height.
    let ascent = line_height * 4 / 5;
    for (index, line) in lines.iter().enumerate() {
        let x = label.top_left.x + padding + (text_width - style.text_width(line)) / 2;
        let y = label.top_left.y + padding + line_height * index as i32 + ascent;
        Text::new(line, Point::new(x, y), style).draw(display)?;
    }
    Ok(())
}

/// The status bar and its label share one line — the label beside the bar's
/// right end rather than stacked on a line of its own below it, saving a
/// full text line's height per cell. Both are centered on the same
/// horizontal axis using the label's *ink* bounds rather than its full line
/// box (which reserves room for descenders no digit, '%', or all-caps label
/// here ever uses) — centering on the line box instead reads as the bar
/// sitting slightly high of the text. The look of both still comes from
/// `status` alone: a partially filled track + live percentage while reading,
/// a fully filled (solid) track + "COMPLETATO" once finished, and a dashed,
/// empty track + "NUOVO" for a book never opened — a dashed outline rather
/// than a 0%-full track, so "never started" reads differently at a glance
/// from "just started".
#[allow(clippy::too_many_arguments)]
fn draw_library_status_row(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    left: i32,
    right: i32,
    top: i32,
    row_height: i32,
    status: LibraryCellStatus,
    locale: Locale,
) -> Result<(), Infallible> {
    let label_style = state.display.detail_style();
    let label = match status {
        LibraryCellStatus::InProgress(percent) => format!("{}%", percent.min(100)),
        LibraryCellStatus::Completed => t(locale, "DONE", "COMPLETATO").to_string(),
        LibraryCellStatus::New => t(locale, "NEW", "NUOVO").to_string(),
    };
    let label_width = label_style.text_width(&label);
    let (ink_top, ink_bottom) = label_style.text_ink_bounds(&label);
    // The row's vertical center, in absolute display coordinates — both the
    // label's ink and the bar are centered on this same axis.
    let axis = top + row_height / 2;
    let baseline = axis - (ink_top + ink_bottom) / 2;
    let bar_top = axis - LIBRARY_BAR_HEIGHT / 2;

    // Inset from the text column's own margins: flush against `left`/`right`
    // (the sharp rectangle this replaced) reads as touching the cell border
    // on the left and crowding the label on the right once the bar got
    // pill-shaped rounded ends — see `LIBRARY_BAR_INSET`.
    let bar_left = left + LIBRARY_BAR_INSET;
    let bar_right = (right - label_width - LIBRARY_BAR_LABEL_GAP - LIBRARY_BAR_INSET).max(bar_left);
    let bar_width = (bar_right - bar_left).max(0);
    // Pill-shaped ends, matching Home's Continue Reading progress bar
    // (`category::draw_continue_reading_progress_bar`) so the two screens'
    // bars read as the same element.
    let bar_radii = CornerRadii::new(Size::new(
        LIBRARY_BAR_HEIGHT as u32 / 2,
        LIBRARY_BAR_HEIGHT as u32 / 2,
    ));
    match status {
        LibraryCellStatus::InProgress(percent) => {
            RoundedRectangle::new(
                Rectangle::new(
                    Point::new(bar_left, bar_top),
                    Size::new(bar_width as u32, LIBRARY_BAR_HEIGHT as u32),
                ),
                bar_radii,
            )
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(display)?;
            let inner_width = (bar_width - 4).max(0);
            let fill_width = inner_width * i32::from(percent.min(100)) / 100;
            if fill_width > 0 {
                let fill_height = (LIBRARY_BAR_HEIGHT - 4).max(0);
                RoundedRectangle::new(
                    Rectangle::new(
                        Point::new(bar_left + 2, bar_top + 2),
                        Size::new(fill_width as u32, fill_height as u32),
                    ),
                    CornerRadii::new(Size::new(fill_height as u32 / 2, fill_height as u32 / 2)),
                )
                .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                .draw(display)?;
            }
        }
        LibraryCellStatus::Completed => {
            RoundedRectangle::new(
                Rectangle::new(
                    Point::new(bar_left, bar_top),
                    Size::new(bar_width as u32, LIBRARY_BAR_HEIGHT as u32),
                ),
                bar_radii,
            )
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;
        }
        LibraryCellStatus::New => {
            draw_dashed_rect(
                display,
                bar_left,
                bar_top,
                bar_right,
                bar_top + LIBRARY_BAR_HEIGHT,
            )?;
        }
    }

    Text::new(
        &label,
        Point::new(right - label_width, baseline),
        label_style,
    )
    .draw(display)?;
    Ok(())
}

/// Dash length and gap for [`draw_dashed_rect`]'s "New" status outline.
const LIBRARY_DASH_LEN: i32 = 4;
const LIBRARY_DASH_GAP: i32 = 3;

fn draw_dashed_hline(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    right: i32,
    y: i32,
) -> Result<(), Infallible> {
    let mut x = left;
    while x < right {
        let end = (x + LIBRARY_DASH_LEN).min(right);
        Rectangle::new(Point::new(x, y), Size::new((end - x).max(0) as u32, 1))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;
        x += LIBRARY_DASH_LEN + LIBRARY_DASH_GAP;
    }
    Ok(())
}

fn draw_dashed_vline(
    display: &mut OrientedFrameBuffer<'_>,
    x: i32,
    top: i32,
    bottom: i32,
) -> Result<(), Infallible> {
    let mut y = top;
    while y < bottom {
        let end = (y + LIBRARY_DASH_LEN).min(bottom);
        Rectangle::new(Point::new(x, y), Size::new(1, (end - y).max(0) as u32))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;
        y += LIBRARY_DASH_LEN + LIBRARY_DASH_GAP;
    }
    Ok(())
}

fn draw_dashed_rect(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
) -> Result<(), Infallible> {
    draw_dashed_hline(display, left, right, top)?;
    draw_dashed_hline(display, left, right, bottom - 1)?;
    draw_dashed_vline(display, left, top, bottom)?;
    draw_dashed_vline(display, right - 1, top, bottom)?;
    Ok(())
}

/// Center x of the scrollbar track, in the gutter reserved to the right of
/// the two-column grid (see `LIBRARY_CELL_WIDTH`'s doc comment).
const LIBRARY_SCROLLBAR_X: i32 = 470;
const LIBRARY_SCROLLBAR_THUMB_WIDTH: i32 = 6;
/// Thumb never shrinks below this, so a very long library's thumb stays
/// visible/grabbable-looking rather than shrinking to a sliver.
const LIBRARY_SCROLLBAR_MIN_THUMB: i32 = 28;
/// Room left below the scrollbar track, inside the same gutter column, for
/// the "more below" chevron — so it doesn't need its own space reserved
/// below `LIBRARY_GRID_BOTTOM` (the book grid runs to the full height; only
/// the gutter's track/chevron split within it).
const LIBRARY_SCROLLBAR_CHEVRON_RESERVE: i32 = 30;

/// Scroll affordance for the two-section grid: a thin track spanning the
/// full grid height plus a thumb sized to how much of the library is
/// currently on-panel and positioned by how far into it the current page is.
/// Drawn only when there's actually more content than fits on one screen —
/// otherwise its mere presence would falsely suggest more to scroll to.
fn draw_library_scrollbar(
    display: &mut OrientedFrameBuffer<'_>,
    track_top: i32,
    track_bottom: i32,
    offset: i32,
    visible: i32,
    total: i32,
) -> Result<(), Infallible> {
    let track_height = track_bottom - track_top;
    if total <= track_height {
        return Ok(());
    }
    Rectangle::new(
        Point::new(LIBRARY_SCROLLBAR_X, track_top),
        Size::new(1, track_height as u32),
    )
    .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
    .draw(display)?;

    let thumb_height = (visible.saturating_mul(track_height) / total)
        .max(LIBRARY_SCROLLBAR_MIN_THUMB)
        .min(track_height);
    let scrollable_track = (track_height - thumb_height).max(0);
    let scrollable_content = (total - visible).max(1);
    let thumb_top = track_top + offset.saturating_mul(scrollable_track) / scrollable_content;

    RoundedRectangle::new(
        Rectangle::new(
            Point::new(
                LIBRARY_SCROLLBAR_X - LIBRARY_SCROLLBAR_THUMB_WIDTH / 2,
                thumb_top,
            ),
            Size::new(LIBRARY_SCROLLBAR_THUMB_WIDTH as u32, thumb_height as u32),
        ),
        CornerRadii::new(Size::new(
            LIBRARY_SCROLLBAR_THUMB_WIDTH as u32 / 2,
            LIBRARY_SCROLLBAR_THUMB_WIDTH as u32 / 2,
        )),
    )
    .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
    .draw(display)?;
    Ok(())
}

/// Down-chevron drawn at the bottom of the scrollbar gutter, below the track
/// itself, only when the current page isn't the last one — the answer to
/// "can I still scroll down for more books?" that the plain scrollbar thumb
/// alone doesn't spell out.
fn draw_library_scroll_hint(display: &mut OrientedFrameBuffer<'_>) -> Result<(), Infallible> {
    draw_iconoir_icon(
        display,
        Point::new(LIBRARY_SCROLLBAR_X - 12, LIBRARY_GRID_BOTTOM - 24),
        &FastArrowDownBox::new(BinaryColor::On),
    )
}

/// Rows a list starting right under the header shows per page.
const LIST_ROWS_PER_PAGE: usize = 10;
/// Top of the first row of a list under a one-line title.
const TITLED_ROWS_TOP: i32 = FIRST_BASELINE + 24;
/// Rows a list under a one-line title shows per page.
const TITLED_ROWS_PER_PAGE: usize = 9;

/// Where a bookmark points, in words: the chapter and its page for an EPUB,
/// the page for a text file.
fn bookmark_row_label(reader: &ReaderUiState, bookmark: &ReaderLocation, locale: Locale) -> String {
    if let Some(chapter) = reader.bookmark_display_chapter_page(bookmark) {
        match locale {
            Locale::English => format!(
                "Chapter {}, page {}",
                chapter.chapter_number,
                chapter.page_text()
            ),
            Locale::Italian => format!(
                "Capitolo {}, pagina {}",
                chapter.chapter_number,
                chapter.page_text()
            ),
        }
    } else {
        format!(
            "{} {}",
            t(locale, "Page", "Pagina"),
            reader.bookmark_display_page(bookmark)
        )
    }
}

/// The page of bookmark rows that holds `selected`, each with how far into
/// the book it is. Returns `(page, pages)` for the footer.
fn draw_bookmark_rows(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    bookmarks: &[ReaderLocation],
    selected: usize,
    rows_top: i32,
    per_page: usize,
) -> Result<(usize, usize), Infallible> {
    let locale = state.regional.locale;
    let selected = selected.min(bookmarks.len().saturating_sub(1));
    let (first, end, page, pages) = page_window(selected, bookmarks.len(), per_page);
    for (row, bookmark) in bookmarks[first..end].iter().enumerate() {
        let percent = bookmark
            .reading_percent
            .map_or_else(String::new, |percent| format!("{}%", percent.min(100)));
        draw_list_row(
            display,
            state.display,
            rows_top + row as i32 * ROW_STEP,
            &bookmark_row_label(&state.reader, bookmark, locale),
            &percent,
            first + row == selected,
        )?;
    }
    Ok((page, pages))
}

/// Footer of a bookmark list: open with SELECT, delete by holding it.
fn bookmark_list_hint(locale: Locale) -> String {
    footer_hints(
        locale,
        &[
            (FooterKey::Select, t(locale, "OPEN", "APRI")),
            (FooterKey::Hold, t(locale, "DELETE", "ELIMINA")),
            (FooterKey::Boot, back_action(locale)),
        ],
    )
}

/// Bookmarks of the open book (`ScreenRoute::ReaderBookmarks`), a page of
/// rows at a time.
pub fn render_bookmarks(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    draw_header(display, state, t(locale, "BOOKMARKS", "SEGNALIBRI"))?;
    let bookmarks = state.reader.session_bookmarks();
    if bookmarks.is_empty() {
        let next = draw_section_title(
            display,
            preferences,
            FIRST_BASELINE,
            t(locale, "No bookmarks", "Nessun segnalibro"),
        )?;
        draw_paragraph(
            display,
            t(
                locale,
                "To add one, open Options from the reading page and choose Mark page.",
                "Per aggiungerne uno, apri le Opzioni dalla pagina di lettura e scegli Segna pagina.",
            ),
            CONTENT_LEFT,
            next,
            preferences.body_style(),
            CONTENT_WIDTH,
            4,
            6,
        )?;
        return draw_footer(display, state, &back_only(locale));
    }
    let position = draw_bookmark_rows(
        display,
        state,
        &bookmarks,
        state.reader.bookmarks_selected,
        FIRST_ROW_TOP,
        LIST_ROWS_PER_PAGE,
    )?;
    draw_footer_paged(display, state, &bookmark_list_hint(locale), Some(position))
}

/// Book title shown under the header on the book-actions overlay and its
/// bookmarks sub-screen — both operate on
/// `state.reader.book_actions_target`, which is only ever unset if a route
/// change raced the overlay closing (BOOT-button Back is instant; nothing
/// in this app's input loop can reach either route without it set first).
fn book_actions_target_title(state: &AppState) -> String {
    let locale = state.regional.locale;
    state.reader.book_actions_target.as_ref().map_or_else(
        || t(locale, "Book", "Libro").to_string(),
        |book| book.title.clone(),
    )
}

/// File size in the unit a reader expects, with the locale's decimal mark.
fn book_size_label(bytes: u64, locale: Locale) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        let tenths = bytes.saturating_mul(10) / (1024 * 1024);
        let mark = t(locale, ".", ",");
        format!("{}{mark}{} MB", tenths / 10, tenths % 10)
    }
}

/// Library long-press overlay (`ScreenRoute::LibraryBookActions`): the
/// actions for the book held on in the Library grid — mark it finished or
/// never read, list its bookmarks, or delete its file. Deleting takes a
/// second SELECT: the row then asks to confirm.
pub fn render_library_book_actions(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let reader = &state.reader;
    draw_header(display, state, t(locale, "BOOK OPTIONS", "OPZIONI LIBRO"))?;
    let next = draw_paragraph(
        display,
        &book_actions_target_title(state),
        CONTENT_LEFT,
        FIRST_BASELINE,
        preferences.heading_style(),
        CONTENT_WIDTH,
        2,
        2,
    )?;
    let info_baseline = next - 4;
    if let Some(book) = reader.book_actions_target.as_ref() {
        let status = match reader.book_actions_target_percent() {
            None | Some(0) => t(locale, "New", "Nuovo").to_string(),
            Some(percent) if percent >= 100 => t(locale, "Completed", "Completato").to_string(),
            Some(percent) => match locale {
                Locale::English => format!("{percent}% read"),
                Locale::Italian => format!("Letto al {percent}%"),
            },
        };
        let info = format!(
            "{} \u{00B7} {} \u{00B7} {status}",
            book.format.badge(),
            book_size_label(book.size_bytes, locale)
        );
        draw_text_fit(
            display,
            &info,
            Point::new(CONTENT_LEFT, info_baseline),
            preferences.detail_style(),
            CONTENT_WIDTH,
        )?;
    }

    let rows_top = info_baseline + 16;
    for (index, action) in LibraryBookAction::ALL.iter().enumerate() {
        let label = if *action == LibraryBookAction::Delete && reader.book_delete_armed {
            t(
                locale,
                "Confirm: delete the book",
                "Conferma: elimina il libro",
            )
        } else {
            action.label_i18n(locale)
        };
        draw_list_row(
            display,
            preferences,
            rows_top + index as i32 * ROW_STEP,
            label,
            "",
            reader.book_actions_selected == index,
        )?;
    }

    let note_baseline = rows_top + LibraryBookAction::ALL.len() as i32 * ROW_STEP + 24;
    let note = if let Some(error) = reader.book_actions_error.as_deref() {
        Some(match locale {
            Locale::English => format!("Could not delete the book: {error}"),
            Locale::Italian => format!("Impossibile eliminare il libro: {error}"),
        })
    } else if reader.book_delete_armed {
        Some(
            t(
                locale,
                "The file is removed from the SD card. This cannot be undone.",
                "Il file viene rimosso dalla scheda SD. L'operazione non si può annullare.",
            )
            .to_string(),
        )
    } else {
        None
    };
    if let Some(note) = note {
        draw_paragraph(
            display,
            &note,
            CONTENT_LEFT,
            note_baseline,
            preferences.body_style(),
            CONTENT_WIDTH,
            4,
            6,
        )?;
    }

    let hint = if reader.book_delete_armed {
        footer_hints(
            locale,
            &[
                (FooterKey::Select, t(locale, "DELETE", "ELIMINA")),
                (FooterKey::Boot, t(locale, "CANCEL", "ANNULLA")),
            ],
        )
    } else {
        select_and_back(locale, t(locale, "CONFIRM", "CONFERMA"))
    };
    draw_footer(display, state, &hint)
}

/// One book's bookmarks (`ScreenRoute::LibraryBookBookmarks`), reached from
/// [`render_library_book_actions`]: the same rows as [`render_bookmarks`]
/// under the book's title.
pub fn render_library_book_bookmarks(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    draw_header(display, state, t(locale, "BOOKMARKS", "SEGNALIBRI"))?;
    let next = draw_section_title(
        display,
        preferences,
        FIRST_BASELINE,
        &book_actions_target_title(state),
    )?;

    let bookmarks = state.reader.book_actions_bookmarks();
    if bookmarks.is_empty() {
        draw_paragraph(
            display,
            t(
                locale,
                "This book has no bookmarks. To add one, open it and choose Mark page in Options.",
                "Questo libro non ha segnalibri. Per aggiungerne uno, aprilo e scegli Segna pagina nelle Opzioni.",
            ),
            CONTENT_LEFT,
            next,
            preferences.body_style(),
            CONTENT_WIDTH,
            4,
            6,
        )?;
        return draw_footer(display, state, &back_only(locale));
    }
    let position = draw_bookmark_rows(
        display,
        state,
        &bookmarks,
        state.reader.book_bookmarks_selected,
        TITLED_ROWS_TOP,
        TITLED_ROWS_PER_PAGE,
    )?;
    draw_footer_paged(display, state, &bookmark_list_hint(locale), Some(position))
}

/// Book opening (`ScreenRoute::ReaderLoading`): the title, the stage and a
/// progress bar. The reason is shown only when the book could not be
/// opened; BOOT cancels at any time.
pub fn render_loading(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let body = preferences.body_style();
    draw_header(display, state, t(locale, "OPENING BOOK", "APERTURA"))?;
    let loading = state.reader.loading.as_ref();
    let title = loading.map_or(t(locale, "Book", "Libro"), |value| {
        value.book.title.as_str()
    });
    let stage = loading.map_or(ReaderLoadingStage::OpeningFile, |value| value.stage);
    let failed = matches!(
        stage,
        ReaderLoadingStage::Failed | ReaderLoadingStage::UnsupportedEpub
    );

    let next = draw_paragraph(
        display,
        title,
        CONTENT_LEFT,
        FIRST_BASELINE,
        preferences.heading_style(),
        CONTENT_WIDTH,
        3,
        2,
    )?;
    let stage_baseline = next + 12;
    draw_text_fit(
        display,
        stage.label_i18n(locale),
        Point::new(CONTENT_LEFT, stage_baseline),
        body,
        CONTENT_WIDTH,
    )?;
    let bar_top = stage_baseline + 18;
    draw_progress(display, bar_top, stage.progress())?;

    if failed {
        if let Some(reason) = loading
            .map(|value| value.message.trim())
            .filter(|reason| !reason.is_empty())
        {
            draw_paragraph(
                display,
                reason,
                CONTENT_LEFT,
                bar_top + LOADING_BAR_HEIGHT + 36,
                body,
                CONTENT_WIDTH,
                6,
                6,
            )?;
        }
        return draw_footer(display, state, &back_only(locale));
    }
    draw_footer(
        display,
        state,
        &footer_hints(locale, &[(FooterKey::Boot, t(locale, "CANCEL", "ANNULLA"))]),
    )
}

/// Top margin above the reading-progress bar, in logical pixels.
const PROGRESS_TOP: i32 = 12;
/// Height of the reading-progress track, in logical pixels.
const PROGRESS_HEIGHT: i32 = 10;
/// Gap between the reading-progress bar and the first line of book text.
const PROGRESS_TO_CONTENT_GAP: i32 = 14;

pub fn render_page(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let Some(session) = state.reader.session.as_ref() else {
        return render_continue_reading(display, state);
    };
    let locale = state.regional.locale;
    let size = display.orientation().logical_size();
    let width = size.width as i32;
    let height = size.height as i32;
    // The session's own layout (not the live preferences) decides full
    // screen, so the geometry always matches what the page was paginated
    // against.
    let full_screen = session.layout.full_screen;
    let (body, footer_line) = reader_body_geometry(width, height, full_screen);
    let body_style = reader_body_style(
        state.reader.preferences.book_font,
        state.reader.preferences.font_size,
        state.reader.preferences.theme,
    );

    let bookmarked = state.reader.current_page_is_bookmarked();
    if !full_screen {
        draw_reading_progress(display, state, session, width, bookmarked)?;
    }

    if let Some(page) = session.current_cached_page() {
        let line_step = i32::from(body_style.line_height()) + 2;
        let first_baseline = body.text.top + i32::from(body_style.line_height());
        report_corrupted_page_text(page);
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
            if let Some(image) = &line.image {
                let slot_top = baseline - i32::from(body_style.line_height());
                let slot_height = image.slot_span as i32 * line_step;
                draw_reader_inline_image(
                    display,
                    state,
                    session,
                    image,
                    &body,
                    body_style,
                    slot_top,
                    slot_height,
                )?;
                continue;
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
            t(locale, "Preparing page...", "Preparazione pagina..."),
            Point::new(body.text.left, baseline),
            body_style,
        )
        .draw_clipped(display, body.text)?;
    }

    if !full_screen {
        draw_reader_footer(display, state, width, height, footer_line)?;
    }
    if bookmarked {
        draw_bookmark_ribbon(display, width)?;
    }

    // HighContrast is a night mode: the page is drawn exactly as Classic,
    // then the whole panel is flipped to white-on-black so the e-paper
    // reflects far less light when reading in the dark.
    if state.reader.preferences.theme == ReadingTheme::HighContrast {
        display.invert_all();
    }
    Ok(())
}

/// Width of the bookmark ribbon on a marked page.
const BOOKMARK_RIBBON_WIDTH: i32 = 8;
/// Height of the bookmark ribbon, notch included.
const BOOKMARK_RIBBON_HEIGHT: i32 = 22;
/// Gap between the ribbon and the panel's right edge: inside the page
/// margin, clear of the book text.
const BOOKMARK_RIBBON_RIGHT_GAP: i32 = 6;
/// Room the progress label leaves between itself and the ribbon.
const BOOKMARK_RIBBON_LABEL_GAP: i32 = 4;

/// A ribbon hanging from the top right corner of a page that has a
/// bookmark, so the mark shows while reading, full screen included.
fn draw_bookmark_ribbon(
    display: &mut OrientedFrameBuffer<'_>,
    width: i32,
) -> Result<(), Infallible> {
    let left = width - BOOKMARK_RIBBON_RIGHT_GAP - BOOKMARK_RIBBON_WIDTH;
    Rectangle::new(
        Point::new(left, 0),
        Size::new(BOOKMARK_RIBBON_WIDTH as u32, BOOKMARK_RIBBON_HEIGHT as u32),
    )
    .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
    .draw(display)?;
    // The swallowtail notch at the ribbon's end.
    Triangle::new(
        Point::new(left, BOOKMARK_RIBBON_HEIGHT),
        Point::new(left + BOOKMARK_RIBBON_WIDTH - 1, BOOKMARK_RIBBON_HEIGHT),
        Point::new(
            left + BOOKMARK_RIBBON_WIDTH / 2,
            BOOKMARK_RIBBON_HEIGHT - BOOKMARK_RIBBON_WIDTH / 2,
        ),
    )
    .into_styled(PrimitiveStyle::with_fill(BinaryColor::Off))
    .draw(display)
}

/// Diagnostic for field reports of pages that rendered fine once and later
/// show up as `?`: pagination only ever emits printable ASCII plus the
/// Italian accents the reader fonts carry, so any other character in an
/// already-paginated line means that page's text changed after it was
/// built, and the font draws each such character as `?`.
fn report_corrupted_page_text(page: &crate::reader::ReaderCachedPage) {
    let unexpected = |character: char| {
        !(character == ' ' || character.is_ascii_graphic() || "àèéìòùÀÈÉÌÒÙ".contains(character))
    };
    for (index, line) in page.lines.iter().enumerate() {
        let count = line.text.chars().filter(|value| unexpected(*value)).count();
        if count > 0 {
            let samples: Vec<String> = line
                .text
                .chars()
                .filter(|value| unexpected(*value))
                .take(8)
                .map(|value| format!("U+{:04X}", u32::from(value)))
                .collect();
            log::error!(
                "rustmix-wave=reader-page-text-corrupted page={} byte-offset={} line={index} count={count} samples={}",
                page.page_index,
                page.byte_offset,
                samples.join(",")
            );
            return;
        }
    }
}

/// Look up (or, on a cache miss, synchronously decode+cache) one inline
/// EPUB image and blit it into its reserved slot span, centered within the
/// box, aspect-ratio-preserving rather than stretched (see
/// `EpubImageCache::generate_bitmap`'s own doc comment). A cache miss costs
/// one real ZIP+JPEG/PNG decode the first time a given page is shown; every
/// redraw after that is a plain SD-cache read. This runs entirely off
/// `&AppState` -- the SD cache file is the persistence layer, so there is
/// nothing to write back into `AppState` for next time, unlike the Library
/// screen's `library_thumbnails` map (which exists only to avoid a redundant
/// SD read on every frame for a screen that redraws far more often than the
/// Reader turns pages).
fn draw_reader_inline_image(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    session: &crate::reader::ReaderSession,
    image: &crate::reader::ReaderPageImage,
    body: &ReaderBodyGeometry,
    body_style: UiTextStyle,
    slot_top: i32,
    slot_height: i32,
) -> Result<(), Infallible> {
    // Pagination chose this box (see `crate::reader::inline_image_slots`) and
    // the Reader's image prewarm generates the cache entry for exactly it, so
    // a page the prewarm already reached is a plain SD-cache hit here.
    let max_width = image.box_width;
    let max_height = image.box_height;
    let cache = crate::cover_cache::EpubImageCache::new(state.reader.cache_directory());
    let bitmap = cache
        .load_cached_bitmap(&session.book, &image.href, max_width, max_height)
        .unwrap_or_else(|| {
            cache.generate_bitmap(&session.book, &image.href, max_width, max_height)
        });

    if bitmap.placeholder {
        return draw_inline_image_placeholder(
            display,
            body,
            body_style,
            slot_top,
            slot_height,
            &image.alt,
        );
    }

    let left = body.text.left + (body.text.width() - i32::from(bitmap.width)).max(0) / 2;
    let top = slot_top + (slot_height - i32::from(bitmap.height)).max(0) / 2;
    display.draw_packed_bitmap_opaque(
        Point::new(left, top),
        bitmap.width,
        bitmap.height,
        &bitmap.bits,
    );
    Ok(())
}

/// Bordered box shown when an inline image fails to extract or decode
/// (corrupt file, unsupported format, oversized source -- see
/// `MAX_PNG_DECODED_BUFFER_BYTES`), with its `alt` text centered inside when
/// present. Matches the Library screen's own placeholder philosophy
/// (`draw_library_cell`'s "not generated yet" outline) rather than silently
/// leaving blank space where an illustration should be.
fn draw_inline_image_placeholder(
    display: &mut OrientedFrameBuffer<'_>,
    body: &ReaderBodyGeometry,
    body_style: UiTextStyle,
    slot_top: i32,
    slot_height: i32,
    alt: &str,
) -> Result<(), Infallible> {
    let box_bounds = TextBounds::new(
        body.text.left,
        slot_top,
        body.text.left + body.text.width(),
        slot_top + slot_height,
    );
    Rectangle::new(
        Point::new(box_bounds.left, box_bounds.top),
        Size::new(
            box_bounds.right.saturating_sub(box_bounds.left).max(0) as u32,
            slot_height.max(0) as u32,
        ),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
    .draw(display)?;
    if !alt.is_empty() {
        let label = truncate_to_width(body_style, alt, box_bounds.width() - 16);
        let label_width = body_style.text_width(&label);
        let left = box_bounds.left + (box_bounds.right - box_bounds.left - label_width).max(0) / 2;
        let baseline = slot_top + slot_height / 2 + i32::from(body_style.line_height()) / 2;
        Text::new(&label, Point::new(left, baseline), body_style)
            .draw_clipped(display, box_bounds)?;
    }
    Ok(())
}

/// Reading-progress row that replaces the Reader page's old title bar: a
/// rounded pill track spanning the full width, filled to the current
/// position, with the chapter and percentage printed at its right end.
/// Keeping this the only element above the book text reclaims the
/// header/title rows for content.
fn draw_reading_progress(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    session: &crate::reader::ReaderSession,
    width: i32,
    bookmarked: bool,
) -> Result<(), Infallible> {
    let style = state.display.body_style();
    let percent_label = session.reading_percent_label();
    // "Cap." (chapter) reads the same in English and Italian chrome, so this
    // label does not need a locale-branched format!() -- unlike the rest of
    // this file's UI strings.
    let label = session.current_epub_chapter_page_label().map_or_else(
        || percent_label.clone(),
        |chapter| format!("Cap. {} - {percent_label}", chapter.chapter_number),
    );
    let label_width = style.text_width(&label);
    let track_left = 14;
    // A marked page keeps the corner clear for its ribbon.
    let ribbon_room = if bookmarked {
        BOOKMARK_RIBBON_WIDTH + BOOKMARK_RIBBON_LABEL_GAP
    } else {
        0
    };
    let track_right = (width - 14 - ribbon_room - 10 - label_width).max(track_left + 4);
    let track_radii = CornerRadii::new(Size::new(
        PROGRESS_HEIGHT as u32 / 2,
        PROGRESS_HEIGHT as u32 / 2,
    ));

    RoundedRectangle::new(
        Rectangle::new(
            Point::new(track_left, PROGRESS_TOP),
            Size::new((track_right - track_left) as u32, PROGRESS_HEIGHT as u32),
        ),
        track_radii,
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
    .draw(display)?;

    if let Some(percent) = session.reading_percent() {
        let inner_width = (track_right - track_left - 4).max(0);
        let fill_width = inner_width * i32::from(percent.min(100)) / 100;
        if fill_width > 0 {
            let fill_height = (PROGRESS_HEIGHT - 4).max(0);
            RoundedRectangle::new(
                Rectangle::new(
                    Point::new(track_left + 2, PROGRESS_TOP + 2),
                    Size::new(fill_width as u32, fill_height as u32),
                ),
                CornerRadii::new(Size::new(fill_height as u32 / 2, fill_height as u32 / 2)),
            )
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;
        }
    }

    Text::new(
        &label,
        Point::new(track_right + 10, PROGRESS_TOP + PROGRESS_HEIGHT),
        style,
    )
    .draw(display)?;
    Ok(())
}

/// In-page dictionary lookup mode, drawn on top of the already-rendered book
/// text: a rounded black pill behind the selected line with its text
/// re-drawn white, the same pill behind just the selected word once a line
/// is confirmed, and a compact definition panel once a word is confirmed. A
/// quick SELECT enters the mode and a held SELECT exits it; see
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
        ReaderDictionaryMode::LineSelect { line_index } => (*line_index, None, None),
        ReaderDictionaryMode::WordSelect {
            line_index,
            word_index,
        } => (*line_index, Some(*word_index), None),
        ReaderDictionaryMode::Definition {
            line_index,
            word_index,
            word,
            message,
        } => (
            *line_index,
            Some(*word_index),
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

    let (pill_left, pill_right) = match word_index {
        None => (
            body.text.left - DICTIONARY_PILL_PAD_X,
            body.text.right + DICTIONARY_PILL_PAD_X,
        ),
        Some(word_index) => {
            let Some(&(start, end)) = eligible_word_spans(&rendered).get(word_index) else {
                return Ok(());
            };
            let word_left = left + body_style.text_width(&rendered[..start]);
            let word_right = word_left + body_style.text_width(&rendered[start..end]).max(1);
            (
                word_left - DICTIONARY_PILL_PAD_X,
                word_right + DICTIONARY_PILL_PAD_X,
            )
        }
    };
    let (pill_top, pill_height) = dictionary_pill_band(body_style, baseline, line_step);
    let radius = if word_index.is_some() {
        pill_height / 2
    } else {
        DICTIONARY_LINE_PILL_RADIUS.min(pill_height / 2)
    };
    RoundedRectangle::new(
        Rectangle::new(
            Point::new(pill_left, pill_top),
            Size::new((pill_right - pill_left) as u32, pill_height as u32),
        ),
        CornerRadii::new(Size::new(radius as u32, radius as u32)),
    )
    .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
    .draw(display)?;

    // Re-draw the whole line white, clipped to the pill: exactly the glyphs
    // (or glyph fragments, e.g. a trailing comma the padding overlaps) that
    // the pill just covered come back inverted, and nothing outside it.
    let clip = TextBounds::new(
        pill_left.max(body.text.left),
        pill_top,
        pill_right.min(body.text.right),
        pill_top + pill_height,
    );
    Text::new(
        rendered.as_str(),
        Point::new(left, baseline),
        body_style.with_color(BinaryColor::Off),
    )
    .draw_clipped(display, clip)?;

    if let Some((word, definition)) = definition {
        let definition = localized_dictionary_message(definition, state.regional.locale);
        draw_dictionary_definition_panel(display, state, body, word, &definition)?;
    }
    Ok(())
}

/// The Reader's own dictionary status messages in the user's language. A
/// definition comes from the dictionary itself and is shown as it is.
fn localized_dictionary_message(message: &str, locale: Locale) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    if locale == Locale::English {
        return Cow::Borrowed(message);
    }
    if message == "Word not found in dictionary." {
        return Cow::Borrowed("Parola non trovata nel dizionario.");
    }
    match message.strip_prefix("Dictionary: ") {
        Some(reason) => Cow::Owned(format!("Dizionario non disponibile: {reason}")),
        None => Cow::Borrowed(message),
    }
}

/// Horizontal padding between the selected text and the pill's rounded ends.
const DICTIONARY_PILL_PAD_X: i32 = 6;
/// Vertical padding between the font's ascender/descender ink and the pill.
const DICTIONARY_PILL_PAD_Y: i32 = 3;
/// Corner radius of the full-width line pill (the word pill is fully round).
const DICTIONARY_LINE_PILL_RADIUS: i32 = 10;
/// Sample covering the strike's ascenders and descenders, so every line's
/// pill has the same height and sits centered on the text regardless of
/// which letters that particular line happens to contain.
const DICTIONARY_PILL_INK_SAMPLE: &str = "Hbdfhklgjpqy";

/// `(top, height)` of the selection pill for the line at `baseline`:
/// centered on the font's actual glyph ink rather than on the line pitch
/// (whose extra leading sits above the ascenders and made the old fill look
/// pushed upward), and never taller than one line step so it cannot cover
/// the neighbouring lines.
fn dictionary_pill_band(style: UiTextStyle, baseline: i32, line_step: i32) -> (i32, i32) {
    let (ink_top, ink_bottom) = style.text_ink_bounds(DICTIONARY_PILL_INK_SAMPLE);
    let height = (ink_bottom - ink_top + 2 * DICTIONARY_PILL_PAD_Y).min(line_step);
    let center = baseline + (ink_top + ink_bottom) / 2;
    (center - height / 2, height)
}

/// Word + definition panel shown once a word is confirmed, drawn over the
/// bottom of the reading body so it never collides with the progress bar or
/// footer. The panel grows upward to fit the whole definition; text is never
/// truncated.
fn draw_dictionary_definition_panel(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    body: &ReaderBodyGeometry,
    word: &str,
    message: &str,
) -> Result<(), Infallible> {
    let layout = definition_panel_layout(
        state.display.heading_style(),
        [state.display.body_style(), state.display.detail_style()],
        body,
        word,
        message,
    );
    let panel_top = body.text.bottom - layout.height;
    let panel = Rectangle::new(
        Point::new(body.frame.left, panel_top),
        Size::new(body.frame.width() as u32, layout.height as u32),
    );
    panel
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::Off))
        .draw(display)?;
    panel
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2))
        .draw(display)?;

    let text_left = body.frame.left + DEFINITION_PANEL_PADDING;
    let mut baseline = panel_top + DEFINITION_PANEL_PADDING;
    for line in &layout.heading_lines {
        baseline += layout.heading_step;
        Text::new(line, Point::new(text_left, baseline), layout.heading).draw(display)?;
    }
    baseline += DEFINITION_PANEL_HEADING_GAP;
    for line in &layout.lines {
        baseline += layout.line_step;
        Text::new(line, Point::new(text_left, baseline), layout.text).draw(display)?;
    }
    Ok(())
}

/// Inner padding between the panel border and its text.
const DEFINITION_PANEL_PADDING: i32 = 12;
/// Extra space between the word heading and the first definition line.
const DEFINITION_PANEL_HEADING_GAP: i32 = 6;
/// Extra leading between wrapped definition lines.
const DEFINITION_PANEL_LINE_GAP: i32 = 3;

struct DefinitionPanelLayout {
    heading: UiTextStyle,
    heading_lines: Vec<String>,
    heading_step: i32,
    text: UiTextStyle,
    lines: Vec<String>,
    line_step: i32,
    height: i32,
}

/// Picks the largest of `text_styles` (largest first) whose fully wrapped
/// definition fits the reading body; the last style is used regardless, and
/// its panel may then cover the whole body. A definition of the length the
/// dictionary produces always fits whole; only a text longer than the body
/// can hold is cut, with an ellipsis.
fn definition_panel_layout(
    heading: UiTextStyle,
    text_styles: [UiTextStyle; 2],
    body: &ReaderBodyGeometry,
    word: &str,
    message: &str,
) -> DefinitionPanelLayout {
    let max_width = body.frame.width() - 2 * DEFINITION_PANEL_PADDING;
    let max_height = body.text.bottom - body.frame.top;
    let heading_lines = wrap_definition_to_width(heading, word, max_width);
    let heading_step = i32::from(heading.line_height());
    let mut chosen = None;
    for text in text_styles {
        let lines = wrap_definition_to_width(text, message, max_width);
        let line_step = i32::from(text.line_height()) + DEFINITION_PANEL_LINE_GAP;
        let height = 2 * DEFINITION_PANEL_PADDING
            + heading_lines.len() as i32 * heading_step
            + DEFINITION_PANEL_HEADING_GAP
            + lines.len() as i32 * line_step;
        let fits = height <= max_height;
        chosen = Some((text, lines, line_step, height));
        if fits {
            break;
        }
    }
    let (text, mut lines, line_step, _) = chosen.expect("two candidate styles");
    // A text too long for the whole body even in the smaller style is cut
    // short there, never drawn through the footer and off the panel.
    let fixed_height = 2 * DEFINITION_PANEL_PADDING
        + heading_lines.len() as i32 * heading_step
        + DEFINITION_PANEL_HEADING_GAP;
    let room = ((max_height - fixed_height) / line_step).max(1) as usize;
    if lines.len() > room {
        lines.truncate(room);
        if let Some(last) = lines.last_mut() {
            *last = truncate_to_width(text, &format!("{last}{ELLIPSIS}"), max_width);
        }
    }
    let height = fixed_height + lines.len() as i32 * line_step;
    DefinitionPanelLayout {
        heading,
        heading_lines,
        heading_step,
        text,
        lines,
        line_step,
        height: height.min(max_height),
    }
}

/// Greedy pixel-width word wrap with no line cap. A single word wider than
/// the panel is broken across lines instead of overflowing the border.
fn wrap_definition_to_width(style: UiTextStyle, text: &str, max_width: i32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if style.text_width(&candidate) <= max_width {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        for character in word.chars() {
            current.push(character);
            if style.text_width(&current) > max_width && current.chars().count() > 1 {
                current.pop();
                lines.push(std::mem::take(&mut current));
                current.push(character);
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Reader page footer: a dot glyph for the options/select shortcut and its
/// contextual label on the left, and the clock
/// plus battery icon — no percentage text — on the right, matching the clock
/// every other screen's footer now shows. Wi-Fi and the date are not shown
/// while reading.
fn draw_reader_footer(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    width: i32,
    height: i32,
    footer_line: i32,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let style = state.display.body_style();
    let color = BinaryColor::On;
    let baseline = height - 18;

    Rectangle::new(
        Point::new(14, footer_line),
        Size::new((width - 28) as u32, 1),
    )
    .into_styled(PrimitiveStyle::with_fill(color))
    .draw(display)?;

    let (select_label, hold_label) = match &state.reader.dictionary_mode {
        ReaderDictionaryMode::Off => (
            t(locale, "Dictionary", "Dizionario"),
            t(locale, "Options", "Opzioni"),
        ),
        ReaderDictionaryMode::LineSelect { .. } => (
            t(locale, "Pick line", "Scegli riga"),
            t(locale, "Exit", "Esci"),
        ),
        ReaderDictionaryMode::WordSelect { .. } => {
            (t(locale, "Look up", "Cerca"), t(locale, "Exit", "Esci"))
        }
        ReaderDictionaryMode::Definition { .. } => (
            t(locale, "Next word", "Prossima parola"),
            t(locale, "Exit", "Esci"),
        ),
    };

    let mut cursor_x = 18;
    draw_dot(display, cursor_x, baseline, color)?;
    cursor_x += DOT_SIZE + ICON_TEXT_GAP;
    let cursor = Text::new(select_label, Point::new(cursor_x, baseline), style).draw(display)?;
    // The hold hint is the same dot stretched into a pill that carries the
    // hold time ("(2s) Opzioni") instead of a "Hold:" word.
    cursor_x = cursor.x + FOOTER_GROUP_GAP;
    cursor_x = draw_hold_pill(display, state, cursor_x, baseline, color)?;
    cursor_x += ICON_TEXT_GAP;
    Text::new(hold_label, Point::new(cursor_x, baseline), style).draw(display)?;

    // The battery glyph's ink sits centered in its square icon box, so
    // center that box on the clock digits' actual ink, not the baseline.
    let (digit_top, digit_bottom) = style.text_ink_bounds("0");
    let digit_center = baseline + (digit_top + digit_bottom) / 2;
    let mut right_x = width - 18 - BATTERY_SIZE.width as i32;
    draw_battery_icon(
        display,
        Point::new(right_x, digit_center - BATTERY_SIZE.height as i32 / 2),
        state.battery_percent(),
        state.battery_charging(),
        color,
    )?;

    let time_label = state.status_time_label();
    right_x -= 10 + style.text_width(&time_label);
    Text::new(&time_label, Point::new(right_x, baseline), style).draw(display)?;

    Ok(())
}

/// Diameter of the round "options" glyph in the footer.
const DOT_SIZE: i32 = 12;
/// Gap between a footer glyph and the label that follows it.
const ICON_TEXT_GAP: i32 = 8;
/// Gap between one footer hint group and the next.
const FOOTER_GROUP_GAP: i32 = 24;

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

/// Hold time printed inside the footer's hold-hint pill; matches
/// `buttons::READER_SELECT_LONG_PRESS_MS`.
const HOLD_PILL_LABEL: &str = "2s";
/// Horizontal padding between the pill's rounded ends and its label.
const HOLD_PILL_PAD_X: i32 = 4;

/// Footer hold hint: a filled pill exactly as tall as [`draw_dot`]'s dot
/// (same top and bottom), widened to fit [`HOLD_PILL_LABEL`] in the
/// smallest UI tier, drawn in inverted ink and centered on its actual
/// glyph ink. Returns the x just past the pill.
fn draw_hold_pill(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    left: i32,
    baseline: i32,
    color: BinaryColor,
) -> Result<i32, Infallible> {
    let style = state.display.detail_style();
    let label_width = style.text_width(HOLD_PILL_LABEL);
    let width = (label_width + 2 * HOLD_PILL_PAD_X).max(DOT_SIZE);
    let top = baseline - DOT_SIZE;
    let radius = DOT_SIZE as u32 / 2;
    RoundedRectangle::new(
        Rectangle::new(
            Point::new(left, top),
            Size::new(width as u32, DOT_SIZE as u32),
        ),
        CornerRadii::new(Size::new(radius, radius)),
    )
    .into_styled(PrimitiveStyle::with_fill(color))
    .draw(display)?;

    let (ink_top, ink_bottom) = style.text_ink_bounds(HOLD_PILL_LABEL);
    let label_baseline = top + DOT_SIZE / 2 - (ink_top + ink_bottom) / 2;
    Text::new(
        HOLD_PILL_LABEL,
        Point::new(left + (width - label_width) / 2, label_baseline),
        style.with_color(color.invert()),
    )
    .draw(display)?;
    Ok(left + width)
}

/// Top edge of the book text in full-screen mode (no progress bar above it).
const FULL_SCREEN_TEXT_TOP: i32 = 20;
/// Gap kept below the book text in full-screen mode (no footer below it).
const FULL_SCREEN_TEXT_BOTTOM_GAP: i32 = 20;

/// Reader page body viewport plus the footer rule's y, for normal reading
/// (progress bar above, hint/clock/battery footer below) or full screen
/// (neither: the text runs from `FULL_SCREEN_TEXT_TOP` to
/// `FULL_SCREEN_TEXT_BOTTOM_GAP` above the panel edge). The single source of
/// truth for both `render_page` and the `lines_per_page` calibration test.
fn reader_body_geometry(width: i32, height: i32, full_screen: bool) -> (ReaderBodyGeometry, i32) {
    let (content_top, footer_line) = if full_screen {
        // `ReaderBodyGeometry::new` keeps the text 12px above `footer_line`.
        (
            FULL_SCREEN_TEXT_TOP,
            height - FULL_SCREEN_TEXT_BOTTOM_GAP + 12,
        )
    } else {
        (
            PROGRESS_TOP + PROGRESS_HEIGHT + PROGRESS_TO_CONTENT_GAP,
            height - 54,
        )
    };
    (
        ReaderBodyGeometry::new(width, content_top, footer_line),
        footer_line,
    )
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
}

impl ReaderBodyGeometry {
    /// Shared Reader body rectangle used by Classic and High Contrast. High
    /// Contrast only inverts the finished frame, so switching themes never
    /// changes TXT pagination or cache fingerprints. The left
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

/// Left edge of the Reader Options tile grid (the shell's shared content
/// inset).
const OPTIONS_GRID_LEFT: i32 = 22;
/// Top edge of the Reader Options tile grid, just below the header.
const OPTIONS_GRID_TOP: i32 = 70;
/// Two columns of tiles filling the shared 436px content width.
const OPTIONS_GRID_COLUMNS: usize = 2;
const OPTIONS_TILE_SIZE: Size = Size::new(
    ((436 - TILE_GAP_X) / OPTIONS_GRID_COLUMNS as i32) as u32,
    COMPACT_TILE_SIZE.height,
);

/// Reader Options: a 2x2 grid of the same rounded icon tiles the Home
/// dashboard uses (thick border + corner dot on the selected one).
pub fn render_options(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    draw_header(display, state, t(locale, "OPTIONS", "OPZIONI"))?;
    let bookmarked = state.reader.current_page_is_bookmarked();
    for (index, option) in ReaderOption::ALL.iter().copied().enumerate() {
        let column = (index % OPTIONS_GRID_COLUMNS) as i32;
        let row = (index / OPTIONS_GRID_COLUMNS) as i32;
        let top_left = Point::new(
            OPTIONS_GRID_LEFT + column * (OPTIONS_TILE_SIZE.width as i32 + TILE_GAP_X),
            OPTIONS_GRID_TOP + row * (OPTIONS_TILE_SIZE.height as i32 + TILE_GAP_Y),
        );
        let label = option.tile_label_i18n(locale, bookmarked);
        let selected = state.reader.options_selected == index;
        let color = BinaryColor::On;
        let preferences = state.display;
        match option {
            ReaderOption::TableOfContents => draw_icon_tile(
                display,
                top_left,
                OPTIONS_TILE_SIZE,
                label,
                &List::new(color),
                selected,
                preferences,
            )?,
            ReaderOption::Bookmarks => draw_icon_tile(
                display,
                top_left,
                OPTIONS_TILE_SIZE,
                label,
                &BookmarkBook::new(color),
                selected,
                preferences,
            )?,
            ReaderOption::Bookmark => draw_icon_tile(
                display,
                top_left,
                OPTIONS_TILE_SIZE,
                label,
                &BookmarkEmpty::new(color),
                selected,
                preferences,
            )?,
            ReaderOption::GoTo => draw_icon_tile(
                display,
                top_left,
                OPTIONS_TILE_SIZE,
                label,
                &Percentage::new(color),
                selected,
                preferences,
            )?,
            ReaderOption::ReadingPreferences => draw_icon_tile(
                display,
                top_left,
                OPTIONS_TILE_SIZE,
                label,
                &SettingsIcon::new(color),
                selected,
                preferences,
            )?,
        }
    }
    draw_footer(
        display,
        state,
        &select_and_back(locale, t(locale, "CHOOSE", "SCEGLI")),
    )
}

/// Reading Preferences screen: the flat list of rows, or (once SELECT opens
/// one) that row's editor. See `ReaderUiState::preference_edit`.
pub fn render_preferences(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    match state.reader.preference_edit {
        Some(candidate) => render_preference_editor(display, state, candidate),
        None => render_preference_list(display, state),
    }
}

fn render_preference_list(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    draw_header(display, state, t(locale, "PREFERENCES", "PREFERENZE"))?;
    for (index, preference) in ReadingPreference::ALL.iter().copied().enumerate() {
        let value = match preference {
            ReadingPreference::ReadingTheme => state.reader.preferences.theme.label_i18n(locale),
            ReadingPreference::Orientation => {
                state.reader.preferences.orientation.label_i18n(locale)
            }
            ReadingPreference::BookFontSize => {
                state.reader.preferences.font_size.label_i18n(locale)
            }
            ReadingPreference::BookFont => state.reader.preferences.book_font.label_i18n(locale),
            ReadingPreference::ParagraphAlignment => state
                .reader
                .preferences
                .paragraph_alignment
                .label_i18n(locale),
            ReadingPreference::ShowProgress if state.reader.preferences.show_progress => {
                t(locale, "On", "Attivo")
            }
            ReadingPreference::ShowProgress => t(locale, "Off", "Non attivo"),
            ReadingPreference::FullScreen if state.reader.preferences.full_screen => {
                t(locale, "On", "Attivo")
            }
            ReadingPreference::FullScreen => t(locale, "Off", "Non attivo"),
        };
        draw_list_row(
            display,
            state.display,
            FIRST_ROW_TOP + index as i32 * ROW_STEP,
            preference.label_i18n(locale),
            value,
            state.reader.preferences_selected == index,
        )?;
    }
    draw_footer(
        display,
        state,
        &select_and_back(locale, t(locale, "EDIT", "MODIFICA")),
    )
}

/// Sample paragraph shown while previewing Paragraph Alignment: long enough
/// to wrap at every book font size, so Justified visibly stretches its
/// non-final lines.
const ALIGNMENT_SAMPLE_EN: &str =
    "The quick brown fox jumps over the lazy dog while reading is a quiet pleasure.";
/// Italian counterpart of [`ALIGNMENT_SAMPLE_EN`], of a similar length.
const ALIGNMENT_SAMPLE_IT: &str =
    "La volpe marrone salta veloce sopra il cane pigro mentre la lettura è un piacere tranquillo.";
/// Most lines of the alignment sample shown.
const ALIGNMENT_SAMPLE_LINES: usize = 5;

/// One row's editor: SELECT on the list opens this with `candidate` seeded
/// from the current `preferences`; UP/DOWN browse `candidate` further
/// (`state.reader` is only ever consulted for `selected_preference()` here —
/// the value being previewed is always `candidate`, never `state.reader.preferences`).
fn render_preference_editor(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    candidate: ReaderPreferences,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preference = state.reader.selected_preference();
    draw_header(display, state, preference.header_label_i18n(locale))?;
    match preference {
        ReadingPreference::Orientation => render_orientation_editor(display, state, candidate)?,
        ReadingPreference::ParagraphAlignment => {
            render_alignment_editor(display, state, candidate)?
        }
        ReadingPreference::BookFontSize => render_font_size_editor(display, state, candidate)?,
        ReadingPreference::BookFont => render_font_editor(display, state, candidate)?,
        ReadingPreference::ReadingTheme => render_theme_editor(display, state, candidate)?,
        ReadingPreference::ShowProgress => {
            render_toggle_editor(display, state, candidate.show_progress)?
        }
        ReadingPreference::FullScreen => {
            render_toggle_editor(display, state, candidate.full_screen)?
        }
    }
    draw_footer(
        display,
        state,
        &footer_hints(
            locale,
            &[
                (FooterKey::Select, t(locale, "CONFIRM", "CONFERMA")),
                (FooterKey::Boot, t(locale, "CANCEL", "ANNULLA")),
            ],
        ),
    )
}

/// Icon-option rows (Orientation, Paragraph Alignment): the shared list row
/// with a small `size24px` glyph before the label.
const ICON_ROW_ICON_SIZE: i32 = 24;
/// Gap between an icon-option row's glyph and its label.
const ICON_ROW_ICON_GAP: i32 = 14;

/// Draw one icon+label option row. `icon` is a different concrete
/// `embedded-iconoir` type per call site, so callers pass it in already
/// constructed rather than this function selecting it generically.
fn draw_icon_option_row<I>(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    top: i32,
    selected: bool,
    icon: &I,
    label: &str,
) -> Result<(), Infallible>
where
    I: embedded_graphics::image::ImageDrawable<Color = BinaryColor>,
{
    let body = state.display.body_style();
    draw_row_frame(display, top, ROW_HEIGHT, selected)?;
    let icon_left = CONTENT_LEFT + ROW_PAD_X;
    draw_iconoir_icon(
        display,
        Point::new(icon_left, top + (ROW_HEIGHT - ICON_ROW_ICON_SIZE) / 2),
        icon,
    )?;
    let label_left = icon_left + ICON_ROW_ICON_SIZE + ICON_ROW_ICON_GAP;
    draw_text_fit(
        display,
        label,
        Point::new(label_left, centered_baseline(body, top, ROW_HEIGHT)),
        body,
        CONTENT_RIGHT - ROW_PAD_X - label_left,
    )
}

/// Top of an editor's first candidate row.
const EDITOR_ROW_TOP: i32 = FIRST_ROW_TOP;
/// Distance from one candidate row's top to the next.
const EDITOR_ROW_STEP: i32 = ROW_STEP;

fn render_orientation_editor(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    candidate: ReaderPreferences,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    for (index, orientation) in [ReaderOrientation::Portrait, ReaderOrientation::Landscape]
        .into_iter()
        .enumerate()
    {
        let top = EDITOR_ROW_TOP + index as i32 * EDITOR_ROW_STEP;
        let selected = orientation == candidate.orientation;
        match orientation {
            ReaderOrientation::Portrait => draw_icon_option_row(
                display,
                state,
                top,
                selected,
                &FastArrowDownBox::new(BinaryColor::On),
                orientation.label_i18n(locale),
            )?,
            ReaderOrientation::Landscape => draw_icon_option_row(
                display,
                state,
                top,
                selected,
                &FastArrowRightBox::new(BinaryColor::On),
                orientation.label_i18n(locale),
            )?,
        }
    }
    Ok(())
}

/// Two-row On/Off editor shared by Show Progress and Full Screen: neither
/// preference has a multi-value list worth an icon, so this just highlights
/// whichever row matches `enabled` the way the icon editors highlight the
/// row matching `candidate`.
fn render_toggle_editor(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    enabled: bool,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    for (index, value) in [true, false].into_iter().enumerate() {
        let top = EDITOR_ROW_TOP + index as i32 * EDITOR_ROW_STEP;
        let label = if value {
            t(locale, "On", "Attivo")
        } else {
            t(locale, "Off", "Non attivo")
        };
        draw_list_row(display, state.display, top, label, "", value == enabled)?;
    }
    Ok(())
}

fn render_alignment_editor(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    candidate: ReaderPreferences,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let alignments = [
        ParagraphAlignment::Left,
        ParagraphAlignment::Center,
        ParagraphAlignment::Right,
        ParagraphAlignment::Justified,
    ];
    for (index, alignment) in alignments.into_iter().enumerate() {
        let top = EDITOR_ROW_TOP + index as i32 * EDITOR_ROW_STEP;
        let selected = alignment == candidate.paragraph_alignment;
        match alignment {
            ParagraphAlignment::Left => draw_icon_option_row(
                display,
                state,
                top,
                selected,
                &AlignLeft::new(BinaryColor::On),
                alignment.label_i18n(locale),
            )?,
            ParagraphAlignment::Center => draw_icon_option_row(
                display,
                state,
                top,
                selected,
                &AlignCenter::new(BinaryColor::On),
                alignment.label_i18n(locale),
            )?,
            ParagraphAlignment::Right => draw_icon_option_row(
                display,
                state,
                top,
                selected,
                &AlignRight::new(BinaryColor::On),
                alignment.label_i18n(locale),
            )?,
            ParagraphAlignment::Justified => draw_icon_option_row(
                display,
                state,
                top,
                selected,
                &AlignJustify::new(BinaryColor::On),
                alignment.label_i18n(locale),
            )?,
        }
    }

    // The sample wraps to the page's own text width in the candidate book
    // font, so it shows what the alignment does to real lines.
    let body_style = reader_body_style(candidate.book_font, candidate.font_size, candidate.theme);
    let sample_top = EDITOR_ROW_TOP + alignments.len() as i32 * EDITOR_ROW_STEP + 14;
    let bounds = TextBounds::new(
        READER_BODY_MARGIN_PX,
        sample_top,
        SCREEN_WIDTH - READER_BODY_MARGIN_PX,
        CONTENT_BOTTOM,
    );
    let line_step = i32::from(body_style.line_height()) + 2;
    let lines = wrap_to_width(
        body_style,
        t(locale, ALIGNMENT_SAMPLE_EN, ALIGNMENT_SAMPLE_IT),
        bounds.width(),
        ALIGNMENT_SAMPLE_LINES,
    );
    for (index, line) in lines.iter().enumerate() {
        let paragraph_end = index + 1 == lines.len();
        let (rendered, left) = aligned_reader_line(
            line,
            paragraph_end,
            candidate.paragraph_alignment,
            body_style,
            bounds,
        );
        let baseline = bounds.top + i32::from(body_style.line_height()) + index as i32 * line_step;
        Text::new(rendered.as_str(), Point::new(left, baseline), body_style)
            .draw_clipped(display, bounds)?;
    }
    Ok(())
}

/// Padding above the label and below the specimen of a specimen row.
const SPECIMEN_PAD_Y: i32 = 12;
/// Gap between a specimen row's label and its specimen.
const SPECIMEN_LABEL_GAP: i32 = 8;
/// Letters with an ascender and a descender, to measure a line's ink.
const SPECIMEN_INK_SAMPLE: &str = "Hg";

/// One text-sample row per candidate, used by Book Font Size, Book Font and
/// Reading Theme: a real specimen rendered with `reader_body_style`, not a
/// mockup, so what's previewed is exactly what the page will look like. The
/// row is as tall as its label and its specimen need, so a large specimen
/// never runs into the label above it. Returns the row's height.
fn draw_specimen_row(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    top: i32,
    selected: bool,
    label: &str,
    specimen_style: UiTextStyle,
) -> Result<i32, Infallible> {
    let locale = state.regional.locale;
    let ui_body = state.display.body_style();
    let (label_top, label_bottom) = ui_body.text_ink_bounds(SPECIMEN_INK_SAMPLE);
    let (specimen_top, specimen_bottom) = specimen_style.text_ink_bounds(SPECIMEN_INK_SAMPLE);
    let label_baseline = top + SPECIMEN_PAD_Y - label_top;
    let specimen_baseline = label_baseline + label_bottom + SPECIMEN_LABEL_GAP - specimen_top;
    let height = specimen_baseline + specimen_bottom + SPECIMEN_PAD_Y - top;
    draw_row_frame(display, top, height, selected)?;
    let left = CONTENT_LEFT + ROW_PAD_X;
    let width = CONTENT_WIDTH - 2 * ROW_PAD_X;
    draw_text_fit(
        display,
        label,
        Point::new(left, label_baseline),
        ui_body,
        width,
    )?;
    draw_text_fit(
        display,
        t(locale, "Aa Reading sample", "Aa Esempio di lettura"),
        Point::new(left, specimen_baseline),
        specimen_style,
        width,
    )?;
    Ok(height)
}

fn render_font_size_editor(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    candidate: ReaderPreferences,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let sizes = [
        BookFontSize::Large,
        BookFontSize::XLarge,
        BookFontSize::XXLarge,
        BookFontSize::XXXLarge,
    ];
    let mut top = EDITOR_ROW_TOP;
    for size in sizes {
        let specimen_style = reader_body_style(candidate.book_font, size, candidate.theme);
        let height = draw_specimen_row(
            display,
            state,
            top,
            size == candidate.font_size,
            size.label_i18n(locale),
            specimen_style,
        )?;
        top += height + ROW_GAP;
    }
    Ok(())
}

fn render_font_editor(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    candidate: ReaderPreferences,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let fonts = [BookFont::Literata, BookFont::AtkinsonHyperlegible];
    let mut top = EDITOR_ROW_TOP;
    for font in fonts {
        let specimen_style = reader_body_style(font, candidate.font_size, candidate.theme);
        let height = draw_specimen_row(
            display,
            state,
            top,
            font == candidate.book_font,
            font.label_i18n(locale),
            specimen_style,
        )?;
        top += height + ROW_GAP;
    }
    Ok(())
}

/// Inset of the High Contrast preview's inverted area from its row's edge:
/// enough to stay inside the rounded border, even the thick selected one.
const THEME_PREVIEW_INSET: i32 = 8;

fn render_theme_editor(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    candidate: ReaderPreferences,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let themes = [ReadingTheme::Classic, ReadingTheme::HighContrast];
    let mut top = EDITOR_ROW_TOP;
    for theme in themes {
        let specimen_style = reader_body_style(candidate.book_font, candidate.font_size, theme);
        let height = draw_specimen_row(
            display,
            state,
            top,
            theme == candidate.theme,
            theme.label_i18n(locale),
            specimen_style,
        )?;
        // Same inversion `render_page` applies for HighContrast, limited to
        // the inside of the row's selection border so the preview matches.
        if theme == ReadingTheme::HighContrast {
            display.invert_logical_rect(&Rectangle::new(
                Point::new(
                    CONTENT_LEFT + THEME_PREVIEW_INSET,
                    top + THEME_PREVIEW_INSET,
                ),
                Size::new(
                    (CONTENT_WIDTH - 2 * THEME_PREVIEW_INSET) as u32,
                    (height - 2 * THEME_PREVIEW_INSET) as u32,
                ),
            ));
        }
        top += height + ROW_GAP;
    }
    Ok(())
}

/// Height of the card that holds the "Go to" percentage.
const GOTO_CARD_HEIGHT: i32 = 150;

/// "Go to" (`ScreenRoute::ReaderGoTo`): a percentage of the book, moved
/// with the rocker in steps, with the chapter it falls in.
pub fn render_goto(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let body = preferences.body_style();
    draw_header(display, state, t(locale, "GO TO", "VAI A"))?;

    let card_top = FIRST_ROW_TOP;
    draw_row_frame(display, card_top, GOTO_CARD_HEIGHT, true)?;
    let large = preferences.large_style();
    draw_text_centered(
        display,
        &format!("{}%", state.reader.goto_percent),
        CONTENT_LEFT,
        CONTENT_WIDTH,
        centered_baseline(large, card_top, GOTO_CARD_HEIGHT),
        large,
    )?;

    let mut baseline = card_top + GOTO_CARD_HEIGHT + 36;
    if let Some(chapter) = state.reader.goto_chapter_label() {
        baseline = draw_paragraph(
            display,
            chapter,
            CONTENT_LEFT,
            baseline,
            preferences.heading_style(),
            CONTENT_WIDTH,
            2,
            2,
        )? + 10;
    }
    let step = crate::reader::READER_GOTO_STEP_PERCENT;
    let hint = match locale {
        Locale::English => {
            format!("UP and DOWN move by {step}%. 0% is the first page, 100% the last.")
        }
        Locale::Italian => format!(
            "SU e GI\u{00D9} spostano del {step}%. 0% \u{00E8} la prima pagina, 100% l'ultima."
        ),
    };
    draw_paragraph(
        display,
        &hint,
        CONTENT_LEFT,
        baseline,
        body,
        CONTENT_WIDTH,
        4,
        6,
    )?;
    draw_footer(
        display,
        state,
        &select_and_back(locale, t(locale, "GO", "VAI")),
    )
}

/// Table of contents (`ScreenRoute::ReaderToc`), a page of rows at a time,
/// with the chapter being read marked.
pub fn render_toc(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    draw_header(display, state, t(locale, "CONTENTS", "INDICE"))?;
    let toc = state.reader.toc_entries();
    if toc.is_empty() {
        let next = draw_section_title(
            display,
            preferences,
            FIRST_BASELINE,
            t(locale, "No table of contents", "Nessun indice"),
        )?;
        draw_paragraph(
            display,
            t(
                locale,
                "This book has no table of contents. Plain text files never have one; EPUB books show theirs here.",
                "Questo libro non ha un indice. I file di testo non lo prevedono; i libri EPUB mostrano qui il loro.",
            ),
            CONTENT_LEFT,
            next,
            preferences.body_style(),
            CONTENT_WIDTH,
            5,
            6,
        )?;
        return draw_footer(display, state, &back_only(locale));
    }

    let current = state.reader.current_toc_index();
    let selected = state.reader.toc_selected.min(toc.len() - 1);
    let (first, end, page, pages) = page_window(selected, toc.len(), LIST_ROWS_PER_PAGE);
    for (row, entry) in toc[first..end].iter().enumerate() {
        let index = first + row;
        let label = entry.label.trim();
        let fallback = format!("{} {}", t(locale, "Chapter", "Capitolo"), index + 1);
        draw_list_row(
            display,
            preferences,
            FIRST_ROW_TOP + row as i32 * ROW_STEP,
            if label.is_empty() { &fallback } else { label },
            if current == Some(index) {
                t(locale, "here", "qui")
            } else {
                ""
            },
            index == selected,
        )?;
    }
    draw_footer_paged(
        display,
        state,
        &select_and_back(locale, t(locale, "GO", "VAI")),
        Some((page, pages)),
    )
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

/// Height of the book-opening progress bar.
const LOADING_BAR_HEIGHT: i32 = 28;

/// The book-opening progress bar, across the content width.
fn draw_progress(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    percent: u8,
) -> Result<(), Infallible> {
    RoundedRectangle::new(
        Rectangle::new(
            Point::new(CONTENT_LEFT, top),
            Size::new(CONTENT_WIDTH as u32, LOADING_BAR_HEIGHT as u32),
        ),
        CornerRadii::new(Size::new(10, 10)),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2))
    .draw(display)?;
    let fill = (CONTENT_WIDTH - 12) * i32::from(percent.min(100)) / 100;
    if fill > 0 {
        RoundedRectangle::new(
            Rectangle::new(
                Point::new(CONTENT_LEFT + 6, top + 6),
                Size::new(fill as u32, (LOADING_BAR_HEIGHT - 12) as u32),
            ),
            CornerRadii::new(Size::new(5, 5)),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(display)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        aligned_reader_line, bookmark_row_label, definition_panel_layout, library_grid_entries,
        library_visible_books, reader_body_style, render_bookmarks, render_continue_reading,
        render_library, render_library_book_actions, render_library_book_bookmarks, render_loading,
        render_options, render_preferences, render_toc, LibraryCellStatus, ReaderBodyGeometry,
        DEFINITION_PANEL_LINE_GAP, DEFINITION_PANEL_PADDING, PROGRESS_HEIGHT, PROGRESS_TOP,
        PROGRESS_TO_CONTENT_GAP,
    };
    use crate::{
        app::AppState,
        cover_cache::{CachedThumbnail, THUMB_HEIGHT, THUMB_WIDTH},
        framebuffer::FrameBuffer,
        orientation::OrientedFrameBuffer,
        reader::{
            BookFont, BookFontSize, BookFormat, ParagraphAlignment, PendingReaderOpen, ReaderBook,
            ReaderChapterPageLabel, ReaderLoadingStage, ReaderLocation, ReaderOrientation,
            ReaderPreferences, ReadingPreference, ReadingTheme,
        },
        regional::Locale,
    };

    #[test]
    fn definition_panel_fits_longest_definition_without_dropping_text() {
        use crate::app::{
            display::UiFontSize,
            typography::{style_for, UiTextRole},
        };
        use embedded_graphics::pixelcolor::BinaryColor;

        // Longest explained definition the engine produces (290 chars).
        let message = "terza persona plurale del congiuntivo imperfetto di accigliare ->             ACCIGLIARE: corrugare la fronte avvicinando le sopracciglia in segno di             preoccupazione, disappunto o concentrazione / assumere un'espressione             severa e accigliata, rabbuiarsi in volto per un pensiero molesto o un...";
        assert!(message.chars().count() >= 280);
        let content_top = PROGRESS_TOP + PROGRESS_HEIGHT + PROGRESS_TO_CONTENT_GAP;
        for (width, height) in [(480, 800), (800, 480)] {
            let body = ReaderBodyGeometry::new(width, content_top, height - 54);
            for size in [UiFontSize::Compact, UiFontSize::Standard, UiFontSize::Large] {
                let style = |role| style_for(size, role, BinaryColor::On);
                let body_text = style(UiTextRole::Body);
                let layout = definition_panel_layout(
                    style(UiTextRole::Heading),
                    [body_text, style(UiTextRole::Detail)],
                    &body,
                    "ACCIGLIASSERO",
                    message,
                );
                let context = format!("{width}x{height} {size:?}");
                // Every word survives the wrap, in order.
                assert_eq!(
                    layout.lines.join(" "),
                    message.split_whitespace().collect::<Vec<_>>().join(" "),
                    "{context}"
                );
                let max_width = body.frame.width() - 2 * DEFINITION_PANEL_PADDING;
                for line in &layout.lines {
                    assert!(
                        layout.text.text_width(line) <= max_width,
                        "{context}: {line}"
                    );
                }
                assert!(
                    layout.height <= body.text.bottom - body.frame.top,
                    "{context}"
                );
                // The larger body strike is used, not the old detail one.
                assert_eq!(
                    layout.line_step,
                    i32::from(body_text.line_height()) + DEFINITION_PANEL_LINE_GAP,
                    "{context}"
                );
            }
        }
    }

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
    fn bookmark_rows_say_the_chapter_and_page_in_words() {
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
            bookmark_row_label(&reader, &bookmark, Locale::English),
            "Chapter 4, page 3/12"
        );
        assert_eq!(
            bookmark_row_label(&reader, &bookmark, Locale::Italian),
            "Capitolo 4, pagina 3/12"
        );
        let text_bookmark = ReaderLocation {
            format: BookFormat::Text,
            epub_chapter: None,
            ..bookmark
        };
        assert_eq!(
            bookmark_row_label(&reader, &text_bookmark, Locale::Italian),
            "Pagina 12"
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
        render_library_book_actions(&mut display, &state).unwrap();
        state.reader.open_book_actions(ReaderBook {
            path: "a.txt".into(),
            title: "A".into(),
            format: BookFormat::Text,
            size_bytes: 1,
            modified_seconds: 0,
        });
        render_library_book_actions(&mut display, &state).unwrap();
        render_library_book_bookmarks(&mut display, &state).unwrap();
        render_options(&mut display, &state).unwrap();
        render_preferences(&mut display, &state).unwrap();
        for (index, _preference) in ReadingPreference::ALL.iter().enumerate() {
            state.reader.preferences_selected = index;
            state.reader.open_preference_editor();
            render_preferences(&mut display, &state).unwrap();
        }
        state.reader.preference_edit = None;
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
        assert!(visible_at_end.iter().any(|book| book.path == "book19.epub"));
        assert!(visible_at_end.len() < state.reader.books.len());
    }

    #[test]
    fn library_visible_books_is_empty_for_an_empty_library() {
        let state = AppState::default();
        assert!(library_visible_books(&state).is_empty());
    }

    #[test]
    fn wrap_to_width_breaks_at_spaces_and_ends_with_an_ellipsis() {
        let style = AppState::default().display.heading_style();
        let width = style.text_width("Il nome del");
        assert_eq!(
            super::wrap_to_width(style, "Il nome del vento", width, 5),
            vec!["Il nome del", "vento"]
        );
        let cut = super::wrap_to_width(style, "Il nome del vento e altre storie", width, 2);
        assert_eq!(cut.len(), 2);
        assert!(cut[1].ends_with('…'), "{cut:?}");
        assert!(cut.iter().all(|line| style.text_width(line) <= width));
        // A word wider than a line is split inside it.
        let word = "Precipitevolissimevolmente";
        let split = super::wrap_to_width(style, word, width, 5);
        assert!(split.len() > 1);
        assert!(split.iter().all(|line| style.text_width(line) <= width));
        assert_eq!(split.concat(), word);
        assert!(super::wrap_to_width(style, "  ", width, 5).is_empty());
    }

    #[test]
    fn placeholder_covers_carry_the_title_real_covers_do_not() {
        fn render(state: &AppState) -> Vec<u8> {
            let mut frame = FrameBuffer::new_white();
            let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
            render_library(&mut display, state).unwrap();
            frame.as_bytes().to_vec()
        }
        let mut state = AppState::default();
        state.reader.books = vec![epub_book(0)];
        // No thumbnail yet, then the generic placeholder: the title shows.
        let before = render(&state);
        state.reader.books[0].title = "Un altro titolo".into();
        assert_ne!(render(&state), before);
        let placeholder = crate::cover_cache::CachedThumbnail {
            width: THUMB_WIDTH,
            height: THUMB_HEIGHT,
            bits: vec![0u8; (THUMB_WIDTH as usize / 8) * THUMB_HEIGHT as usize],
            placeholder: true,
        };
        state
            .reader
            .library_thumbnails
            .insert("book0.epub".into(), placeholder.clone());
        let titled = render(&state);
        state.reader.books[0].title = "Book 0".into();
        assert_ne!(render(&state), titled);
        // A real cover already shows its title.
        state.reader.library_thumbnails.insert(
            "book0.epub".into(),
            crate::cover_cache::CachedThumbnail {
                placeholder: false,
                ..placeholder
            },
        );
        let cover = render(&state);
        state.reader.books[0].title = "Un altro titolo".into();
        assert_eq!(render(&state), cover);
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

    /// For every real on-device orientation/font/size combination, the last
    /// line `ReaderPreferences::layout()` says fits on a page
    /// (`lines_per_page`) must actually clear `render_page`'s own
    /// `if baseline >= body.text.bottom { break; }` guard. If a calibrated
    /// `lines_per_page` and the render geometry ever disagree by even one
    /// pixel, that guard silently drops the page's last line -- paginated,
    /// but never drawn -- which reads exactly like "a line goes missing
    /// between page turns" despite the underlying text data being complete.
    /// Portrait/XLarge caught exactly this (line 20 landed baseline-on-edge)
    /// before `layout()` was corrected to 19; this guards against that class
    /// of drift recurring for any future font/size addition.
    #[test]
    fn every_reader_layout_clears_its_own_render_clip_guard() {
        let orientations = [ReaderOrientation::Portrait, ReaderOrientation::Landscape];
        let sizes = [
            BookFontSize::Large,
            BookFontSize::XLarge,
            BookFontSize::XXLarge,
            BookFontSize::XXXLarge,
        ];
        let fonts = [BookFont::Literata, BookFont::AtkinsonHyperlegible];

        let mut failures = Vec::new();
        for full_screen in [false, true] {
            for &orientation in &orientations {
                for &font_size in &sizes {
                    for &book_font in &fonts {
                        let preferences = ReaderPreferences {
                            theme: ReadingTheme::Classic,
                            orientation,
                            font_size,
                            book_font,
                            paragraph_alignment: ParagraphAlignment::Left,
                            show_progress: true,
                            full_screen,
                        };
                        let layout = preferences.layout();
                        let display_orientation = match orientation {
                            ReaderOrientation::Portrait => {
                                crate::orientation::DisplayOrientation::Portrait
                            }
                            ReaderOrientation::Landscape => {
                                crate::orientation::DisplayOrientation::Landscape
                            }
                        };
                        let size = display_orientation.logical_size();
                        let width = size.width as i32;
                        let height = size.height as i32;
                        let (body, _) = super::reader_body_geometry(width, height, full_screen);
                        let body_style =
                            reader_body_style(book_font, font_size, ReadingTheme::Classic);
                        let line_step = i32::from(body_style.line_height()) + 2;
                        let first_baseline = body.text.top + i32::from(body_style.line_height());
                        let last_index = layout.lines_per_page - 1;
                        let last_baseline = first_baseline + last_index as i32 * line_step;
                        if last_baseline >= body.text.bottom {
                            failures.push(format!(
                            "full_screen={full_screen} orientation={orientation:?} font_size={font_size:?} book_font={book_font:?}: \
                             lines_per_page={} last_baseline={last_baseline} body.text.bottom={} \
                             (line_height={})",
                            layout.lines_per_page,
                            body.text.bottom,
                            body_style.line_height(),
                        ));
                        }
                    }
                }
            }
        }
        assert!(
            failures.is_empty(),
            "the following configurations drop their last calibrated line at render time:\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn marking_a_book_completed_moves_it_from_new_to_completed_in_recent() {
        let mut state = AppState::default();
        state.reader.books = vec![epub_book(0)];

        let (entries, in_progress_count) = library_grid_entries(&state.reader);
        assert_eq!(in_progress_count, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, LibraryCellStatus::New);

        state.reader.open_book_actions(entries[0].book.clone());
        assert!(state.reader.mark_book_actions_target_completed());

        let (entries, in_progress_count) = library_grid_entries(&state.reader);
        assert_eq!(in_progress_count, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, LibraryCellStatus::Completed);
    }

    #[test]
    fn a_book_stuck_at_zero_percent_is_new_not_reading_now() {
        let mut state = AppState::default();
        state.reader.books = vec![epub_book(0)];
        state.reader.recent = vec![ReaderLocation {
            path: "book0.epub".into(),
            title: "Book 0".into(),
            format: BookFormat::Epub,
            size_bytes: 10,
            modified_seconds: 1,
            page_index: 0,
            byte_offset: 0,
            epub_chapter: None,
            reading_percent: Some(0),
        }];

        let (entries, in_progress_count) = library_grid_entries(&state.reader);
        assert_eq!(in_progress_count, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, LibraryCellStatus::New);
    }

    /// Every Reader Options tile title fits inside its tile, with a margin
    /// clear of the rounded border, at every UI font profile.
    #[test]
    fn reader_option_tile_labels_fit_their_tiles() {
        use crate::app::display::{DisplayPreferences, UiFontSize};
        use crate::reader::ReaderOption;
        use crate::regional::Locale;

        let max_width = super::OPTIONS_TILE_SIZE.width as i32 - 2 * 16;
        for font_size in [UiFontSize::Compact, UiFontSize::Standard, UiFontSize::Large] {
            let preferences = DisplayPreferences {
                font_size,
                ..DisplayPreferences::default()
            };
            let heading = preferences.heading_style();
            for option in ReaderOption::ALL {
                for locale in [Locale::English, Locale::Italian] {
                    for bookmarked in [false, true] {
                        let label = option.tile_label_i18n(locale, bookmarked);
                        assert!(
                            heading.text_width(label) <= max_width,
                            "{label:?} too wide at {font_size:?}"
                        );
                    }
                }
            }
        }
    }
}
