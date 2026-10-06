//! SDMMC file-browser screen: folders, a text preview, deleting a file.

use core::convert::Infallible;

use embedded_graphics::prelude::Point;

use crate::{
    app::{
        i18n::t,
        state::AppState,
        typography::Text,
        widgets::{
            footer::{
                back_action, draw_footer, draw_footer_paged, footer_hints, select_and_back,
                FooterKey,
            },
            header::draw_header,
            layout::{CONTENT_LEFT, CONTENT_WIDTH, FIRST_BASELINE},
            list::{draw_list_row, draw_row_frame, ROW_PAD_X, ROW_STEP},
            text::{draw_paragraph, draw_text_fit, truncate_start_to_width, truncate_to_width},
        },
    },
    orientation::OrientedFrameBuffer,
    storage::{FilePreview, StorageEntryKind, StorageNotice, STORAGE_PAGE_SIZE},
};

/// Baseline of the current-folder line under the header.
const PATH_BASELINE: i32 = FIRST_BASELINE - 12;
/// Top of the first entry row.
const LIST_TOP: i32 = PATH_BASELINE + 16;

/// Draw the SDMMC browser or the bounded text-preview panel.
pub fn render_files(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let storage = &state.storage;
    if let Some(preview) = &storage.preview {
        return render_preview(display, state, preview);
    }
    let body = preferences.body_style();

    draw_header(display, state, t(locale, "FILES", "FILE"))?;

    // Where the list is: the end of a long path matters most.
    Text::new(
        &truncate_start_to_width(body, &storage.current_path, CONTENT_WIDTH),
        Point::new(CONTENT_LEFT, PATH_BASELINE),
        body,
    )
    .draw(display)?;

    let selected_on_page = storage.selected_on_page();
    let visible = storage.visible_entries();
    let delete_question = t(locale, "delete?", "eliminare?");
    for (index, entry) in visible.iter().enumerate() {
        let selected = index == selected_on_page;
        let (name, note): (&str, String) = match entry.kind {
            // The synthetic row carries an English name; show it localized.
            StorageEntryKind::RetryScan => (
                t(locale, "Read the card again", "Rileggi la scheda"),
                String::new(),
            ),
            StorageEntryKind::Directory => (
                entry.name.as_str(),
                t(locale, "folder", "cartella").to_string(),
            ),
            StorageEntryKind::File if selected && storage.pending_delete.is_some() => {
                (entry.name.as_str(), delete_question.to_string())
            }
            StorageEntryKind::File => (entry.name.as_str(), entry.size_label()),
        };
        draw_list_row(
            display,
            preferences,
            LIST_TOP + index as i32 * ROW_STEP,
            name,
            &note,
            selected,
        )?;
    }

    // Under the rows: how a delete went, what went wrong, or that there is
    // nothing to show.
    let message = if let Some(notice) = &storage.notice {
        Some(match notice {
            StorageNotice::Deleted(name) => {
                format!("{} {name}", t(locale, "Deleted:", "Eliminato:"))
            }
            StorageNotice::DeleteFailed(name, error) => format!(
                "{} {name} ({error})",
                t(locale, "Could not delete", "Impossibile eliminare")
            ),
        })
    } else if !storage.mounted {
        Some(
            t(
                locale,
                "No memory card found. Insert a FAT-formatted microSD and restart the device.",
                "Scheda di memoria non trovata. Inserisci una microSD formattata FAT e riavvia il dispositivo.",
            )
            .to_string(),
        )
    } else if let Some(error) = &storage.error {
        Some(error.clone())
    } else if storage.scan.retained_entries == 0 {
        Some(
            if storage.at_root {
                t(
                    locale,
                    "No files or folders on this memory card.",
                    "Nessun file o cartella su questa scheda di memoria.",
                )
            } else {
                t(
                    locale,
                    "This folder is empty.",
                    "Questa cartella \u{00E8} vuota.",
                )
            }
            .to_string(),
        )
    } else {
        None
    };
    if let Some(message) = message {
        draw_paragraph(
            display,
            &message,
            CONTENT_LEFT,
            LIST_TOP + visible.len() as i32 * ROW_STEP + 26,
            body,
            CONTENT_WIDTH,
            3,
            6,
        )?;
    }

    // BOOT goes up a folder and, from the top one, back to Home.
    let boot = if storage.at_root {
        back_action(locale)
    } else {
        t(locale, "UP", "SU")
    };
    let hint = if storage.pending_delete.is_some() {
        footer_hints(
            locale,
            &[
                (FooterKey::Select, t(locale, "DELETE", "ELIMINA")),
                (FooterKey::Boot, t(locale, "CANCEL", "ANNULLA")),
            ],
        )
    } else {
        match storage.selected_entry().map(|entry| entry.kind) {
            Some(StorageEntryKind::File) => footer_hints(
                locale,
                &[
                    (FooterKey::Select, t(locale, "OPEN", "APRI")),
                    (FooterKey::Hold, t(locale, "DELETE", "ELIMINA")),
                    (FooterKey::Boot, boot),
                ],
            ),
            Some(StorageEntryKind::RetryScan) => footer_hints(
                locale,
                &[
                    (FooterKey::Select, t(locale, "RETRY", "RIPROVA")),
                    (FooterKey::Boot, boot),
                ],
            ),
            Some(StorageEntryKind::Directory) => footer_hints(
                locale,
                &[
                    (FooterKey::Select, t(locale, "OPEN", "APRI")),
                    (FooterKey::Boot, boot),
                ],
            ),
            None => footer_hints(locale, &[(FooterKey::Boot, boot)]),
        }
    };
    let pages = storage.entries.len().max(1).div_ceil(STORAGE_PAGE_SIZE);
    let page = storage.page_start / STORAGE_PAGE_SIZE + 1;
    draw_footer_paged(display, state, &hint, Some((page, pages)))
}

/// Characters a preview line may hold before it is cut to the frame width.
const PREVIEW_LINE_CHARS: usize = 72;
/// Lines of the preview frame.
const PREVIEW_LINES: usize = 20;

fn render_preview(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    preview: &FilePreview,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let body = preferences.body_style();

    draw_header(display, state, t(locale, "FILE PREVIEW", "ANTEPRIMA"))?;
    draw_text_fit(
        display,
        &preview.name,
        Point::new(CONTENT_LEFT, PATH_BASELINE),
        body,
        CONTENT_WIDTH,
    )?;

    let frame_top = LIST_TOP;
    let line_step = i32::from(body.line_height()) + 6;
    let frame_height = PREVIEW_LINES as i32 * line_step + 22;
    draw_row_frame(display, frame_top, frame_height, false)?;
    let text_left = CONTENT_LEFT + ROW_PAD_X;
    let text_width = CONTENT_WIDTH - 2 * ROW_PAD_X;
    if preview.binary {
        draw_paragraph(
            display,
            t(
                locale,
                "This file is not text: there is nothing to preview.",
                "Questo file non \u{00E8} testo: non c'\u{00E8} nulla da mostrare in anteprima.",
            ),
            text_left,
            frame_top + 30,
            body,
            text_width,
            4,
            6,
        )?;
    } else {
        for (index, line) in preview
            .display_lines(PREVIEW_LINES, PREVIEW_LINE_CHARS)
            .iter()
            .enumerate()
        {
            Text::new(
                &truncate_to_width(body, line, text_width),
                Point::new(text_left, frame_top + 28 + index as i32 * line_step),
                body,
            )
            .draw(display)?;
        }
    }
    if preview.truncated {
        draw_paragraph(
            display,
            t(
                locale,
                "Only the beginning of the file is shown.",
                "\u{00C8} mostrato solo l'inizio del file.",
            ),
            CONTENT_LEFT,
            frame_top + frame_height + 28,
            body,
            CONTENT_WIDTH,
            2,
            6,
        )?;
    }

    draw_footer(
        display,
        state,
        &select_and_back(locale, t(locale, "CLOSE", "CHIUDI")),
    )
}
