//! Global Power-key display-maintenance menu.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::{
    app::{
        i18n::t,
        state::AppState,
        typography::{Text, UiTextStyle},
        widgets::{footer::draw_footer, header::draw_header},
    },
    orientation::OrientedFrameBuffer,
};

pub fn render_power_key_menu(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();

    draw_header(display, state, t(locale, "POWER KEY", "TASTO ACCENSIONE"))?;

    Text::new(
        t(locale, "Screen refresh", "Aggiornamento schermo"),
        Point::new(22, 128),
        heading,
    )
    .draw(display)?;
    Text::new(
        t(
            locale,
            "Run a clean global refresh to clear e-paper ghosting.",
            "Esegui un aggiornamento completo per eliminare gli aloni dell'e-paper.",
        ),
        Point::new(22, 174),
        body,
    )
    .draw(display)?;

    draw_action(
        display,
        246,
        t(locale, "Clear ghosting now", "Elimina aloni ora"),
        state.power_key_menu.selected == 0,
        body,
    )?;
    draw_action(
        display,
        326,
        t(locale, "Cancel", "Annulla"),
        state.power_key_menu.selected == 1,
        body,
    )?;

    Rectangle::new(Point::new(22, 454), Size::new(436, 104))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;
    Text::new(
        t(locale, "Long Power press", "Pressione lunga tasto"),
        Point::new(44, 500),
        heading,
    )
    .draw(display)?;
    Text::new(
        t(
            locale,
            "Enter sleep-image mode",
            "Attiva modalità immagine sospensione",
        ),
        Point::new(44, 534),
        body,
    )
    .draw(display)?;

    draw_footer(display, state, t(locale, "SELECT RUN", "SELECT ESEGUI"))?;
    Ok(())
}

fn draw_action(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    label: &str,
    selected: bool,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    let border = if selected {
        PrimitiveStyle::with_stroke(BinaryColor::On, 4)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    Rectangle::new(Point::new(22, top), Size::new(436, 62))
        .into_styled(border)
        .draw(display)?;
    Text::new(
        if selected { ">" } else { " " },
        Point::new(38, top + 40),
        style,
    )
    .draw(display)?;
    Text::new(label, Point::new(68, top + 40), style).draw(display)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::render_power_key_menu;
    use crate::{app::AppState, framebuffer::FrameBuffer, orientation::OrientedFrameBuffer};

    #[test]
    fn power_key_menu_renders_without_optional_services() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        render_power_key_menu(&mut display, &AppState::default()).unwrap();
    }
}
