//! SDMMC read-only file-browser screen.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::app::{i18n::t, typography::Text};

use crate::{
    app::{
        state::AppState,
        widgets::{footer::draw_footer, header::draw_header},
    },
    orientation::OrientedFrameBuffer,
    storage::{FilePreview, StorageEntryKind},
};

/// Draw the read-only SDMMC browser or the bounded text-preview panel.
pub fn render_files(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let storage = &state.storage;
    if let Some(preview) = &storage.preview {
        return render_preview(display, state, preview);
    }

    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();

    draw_header(display, state, t(locale, "FILES", "FILE"))?;

    Text::new(
        t(locale, "Directory", "Cartella"),
        Point::new(22, 106),
        heading,
    )
    .draw(display)?;
    if let Some(error) = &storage.error {
        Text::new(&truncate_label(error, 68), Point::new(22, 134), body).draw(display)?;
    } else if storage.scan.retained_entries == 0 {
        Text::new(
            t(
                locale,
                "No files or directories found on this SD card.",
                "Nessun file o cartella trovato su questa scheda SD.",
            ),
            Point::new(22, 134),
            body,
        )
        .draw(display)?;
    } else {
        Text::new(
            t(
                locale,
                "Directories first, then files. No write operations.",
                "Prima le cartelle, poi i file. Nessuna operazione di scrittura.",
            ),
            Point::new(22, 134),
            body,
        )
        .draw(display)?;
    }

    let selected_on_page = storage.selected_on_page();
    for (index, entry) in storage.visible_entries().iter().enumerate() {
        let top = 164 + (index as i32 * 66);
        let selected = index == selected_on_page;
        let outline = if selected {
            PrimitiveStyle::with_stroke(BinaryColor::On, 3)
        } else {
            PrimitiveStyle::with_stroke(BinaryColor::On, 1)
        };
        Rectangle::new(Point::new(22, top), Size::new(436, 54))
            .into_styled(outline)
            .draw(display)?;
        Text::new(
            if selected { ">" } else { " " },
            Point::new(36, top + 32),
            heading,
        )
        .draw(display)?;
        // The synthetic rows carry an English name; show them localized.
        let name = match entry.kind {
            StorageEntryKind::BackToHome => t(locale, "Back to Home", "Torna alla Home"),
            StorageEntryKind::RetryScan => t(locale, "Retry SD scan", "Rileggi la scheda SD"),
            _ => entry.name.as_str(),
        };
        Text::new(&truncate_label(name, 29), Point::new(62, top + 23), heading).draw(display)?;
        Text::new(
            entry.kind.badge_i18n(locale),
            Point::new(62, top + 43),
            detail,
        )
        .draw(display)?;
        let size_label = if entry.kind == StorageEntryKind::File {
            entry.size_label()
        } else {
            entry.kind.badge_i18n(locale).to_string()
        };
        Text::new(&size_label, Point::new(382, top + 32), detail).draw(display)?;
    }

    Ok(())
}

fn render_preview(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    preview: &FilePreview,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();

    draw_header(display, state, t(locale, "FILE PREVIEW", "ANTEPRIMA"))?;
    Text::new(
        &truncate_label(&preview.name, 52),
        Point::new(22, 106),
        heading,
    )
    .draw(display)?;
    Text::new(
        if preview.binary {
            t(
                locale,
                "Binary content is intentionally not rendered.",
                "Il contenuto binario non viene visualizzato di proposito.",
            )
        } else if preview.truncated {
            t(
                locale,
                "Preview capped at 384 bytes. File remains unchanged.",
                "Anteprima limitata a 384 byte. Il file resta invariato.",
            )
        } else {
            t(
                locale,
                "Text preview. File remains unchanged.",
                "Anteprima testo. Il file resta invariato.",
            )
        },
        Point::new(22, 136),
        body,
    )
    .draw(display)?;

    Rectangle::new(Point::new(22, 164), Size::new(436, 498))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;
    for (index, line) in preview.display_lines(18, 52).iter().enumerate() {
        Text::new(line, Point::new(34, 192 + (index as i32 * 24)), detail).draw(display)?;
    }

    draw_footer(display, state, t(locale, "SELECT CLOSE", "SELECT CHIUDI"))?;
    Ok(())
}

fn truncate_label(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.into();
    }
    let mut output: String = value.chars().take(max_chars.saturating_sub(3)).collect();
    output.push_str("...");
    output
}
