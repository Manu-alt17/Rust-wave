//! Clean placeholder for future modular applications.

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
        widgets::header::draw_header,
    },
    orientation::OrientedFrameBuffer,
};

pub fn render_placeholder(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let route = state.active_route();
    let parent = route
        .parent()
        .map_or("Home", |value| value.label_i18n(locale));
    let title = route.label_i18n(locale).to_ascii_uppercase();
    let heading = state.display.heading_style();
    let body = state.display.body_style();

    draw_header(display, state, &title)?;
    Text::new(
        route.label_i18n(locale),
        Point::new(22, 124),
        state.display.navigation_style(),
    )
    .draw(display)?;
    Rectangle::new(Point::new(22, 176), Size::new(436, 238))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;
    Text::new(
        t(locale, "Reserved for a later", "Riservato a una futura"),
        Point::new(48, 254),
        heading,
    )
    .draw(display)?;
    Text::new(
        t(
            locale,
            "isolated feature milestone.",
            "funzionalità isolata.",
        ),
        Point::new(48, 294),
        heading,
    )
    .draw(display)?;
    Text::new(
        t(
            locale,
            "Navigation is ready now.",
            "La navigazione è già pronta.",
        ),
        Point::new(48, 356),
        body,
    )
    .draw(display)?;
    Text::new(
        &format!("{} {parent}", t(locale, "Parent:", "Superiore:")),
        Point::new(22, 484),
        body,
    )
    .draw(display)?;
    Ok(())
}
