//! Persistent global user-interface typography settings.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::app::typography::{Text, UiTextStyle};

use crate::{
    app::{
        i18n::t,
        state::AppState,
        widgets::{footer::draw_footer, header::draw_header},
    },
    orientation::OrientedFrameBuffer,
};

pub fn render_display(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let prefs = state.display;

    draw_header(display, state, t(locale, "DISPLAY", "SCHERMO"))?;
    Text::new(
        t(locale, "Display preferences", "Preferenze schermo"),
        Point::new(22, 114),
        heading,
    )
    .draw(display)?;

    draw_setting_row(
        display,
        156,
        t(locale, "UI size", "Dimensione UI"),
        prefs.font_size.label_i18n(locale),
        state.display_action_selected == 0,
        body,
    )?;
    draw_setting_row(
        display,
        246,
        t(locale, "Sleep screen", "Sfondo riposo"),
        prefs.sleep_screen.label_i18n(locale),
        state.display_action_selected == 1,
        body,
    )?;

    Text::new(
        t(locale, "Live preview", "Anteprima live"),
        Point::new(22, 454),
        heading,
    )
    .draw(display)?;
    Rectangle::new(Point::new(22, 482), Size::new(436, 160))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;
    Text::new(
        t(locale, "Reader", "Lettore"),
        Point::new(44, 544),
        prefs.navigation_style(),
    )
    .draw(display)?;
    Text::new(
        t(
            locale,
            "Books, progress and bookmarks",
            "Libri, progressi e segnalibri",
        ),
        Point::new(44, 592),
        body,
    )
    .draw(display)?;
    Text::new(
        t(
            locale,
            "Press BOOT to return to Settings.",
            "Premi BOOT per tornare a Impostazioni.",
        ),
        Point::new(22, 700),
        body,
    )
    .draw(display)?;

    draw_footer(display, state, t(locale, "SELECT CHANGE", "SELECT CAMBIA"))?;
    Ok(())
}

fn draw_setting_row(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    label: &str,
    value: &str,
    selected: bool,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    let border = if selected {
        PrimitiveStyle::with_stroke(BinaryColor::On, 4)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    Rectangle::new(Point::new(22, top), Size::new(436, 70))
        .into_styled(border)
        .draw(display)?;
    Text::new(
        if selected { ">" } else { " " },
        Point::new(38, top + 43),
        style,
    )
    .draw(display)?;
    Text::new(label, Point::new(68, top + 43), style).draw(display)?;
    Text::new(value, Point::new(258, top + 43), style).draw(display)?;
    Ok(())
}
