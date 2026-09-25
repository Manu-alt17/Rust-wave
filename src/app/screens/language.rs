//! On-device language selection.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::{
    app::{
        state::AppState,
        typography::Text,
        widgets::{footer::draw_footer, header::draw_header},
    },
    orientation::OrientedFrameBuffer,
};

pub fn render_language(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();

    draw_header(
        display,
        state,
        crate::app::i18n::t(locale, "LANGUAGE", "LINGUA"),
    )?;
    Text::new(
        crate::app::i18n::t(locale, "Display language", "Lingua dell'interfaccia"),
        Point::new(22, 114),
        heading,
    )
    .draw(display)?;

    let border = PrimitiveStyle::with_stroke(BinaryColor::On, 4);
    Rectangle::new(Point::new(22, 156), Size::new(436, 70))
        .into_styled(border)
        .draw(display)?;
    Text::new(">", Point::new(38, 199), body).draw(display)?;
    Text::new(
        crate::app::i18n::t(locale, "Language", "Lingua"),
        Point::new(68, 199),
        body,
    )
    .draw(display)?;
    Text::new(locale.display_label(), Point::new(258, 199), body).draw(display)?;

    Text::new(
        crate::app::i18n::t(
            locale,
            "Chrome text (menus, labels) shows accents as plain letters;",
            "Il testo dell'interfaccia (menu, etichette) mostra gli accenti come lettere semplici;",
        ),
        Point::new(22, 300),
        body,
    )
    .draw(display)?;
    Text::new(
        crate::app::i18n::t(
            locale,
            "book text in the Reader keeps full accented characters.",
            "il testo dei libri nel Lettore mantiene gli accenti completi.",
        ),
        Point::new(22, 330),
        body,
    )
    .draw(display)?;

    draw_footer(
        display,
        state,
        crate::app::i18n::t(locale, "SELECT SWITCH", "SELECT CAMBIA"),
    )?;
    Ok(())
}
