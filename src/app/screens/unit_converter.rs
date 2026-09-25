//! Offline fixed-point Unit Converter Tools application.

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
    regional::Locale,
    unit_converter::{format_milli, ConversionResult, ConverterField},
};

/// Render the interactive offline converter screen.
pub fn render_unit_converter(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let converter = state.unit_converter;
    let body = state.display.body_style();
    let heading = state.display.heading_style();
    let result = result_label(locale, converter.result(), converter.to_unit().symbol());
    let value = format_milli(converter.value_milli);
    let step = format_milli(converter.step_milli());

    // "Conv" reads the same in English and Italian chrome, so this header
    // does not need a locale-branched `t()` call.
    draw_header(display, state, "CONV")?;

    Text::new(
        t(locale, "Conversion", "Conversione"),
        Point::new(22, 112),
        heading,
    )
    .draw(display)?;
    draw_field(
        display,
        138,
        t(locale, "Category", "Categoria"),
        converter.category.label_i18n(locale),
        converter.active_field == ConverterField::Category,
        body,
    )?;
    draw_field(
        display,
        200,
        t(locale, "From", "Da"),
        converter.from_unit().label_i18n(locale),
        converter.active_field == ConverterField::FromUnit,
        body,
    )?;
    draw_field(
        display,
        262,
        t(locale, "Value", "Valore"),
        &value,
        converter.active_field == ConverterField::Value,
        body,
    )?;
    draw_field(
        display,
        324,
        t(locale, "To", "A"),
        converter.to_unit().label_i18n(locale),
        converter.active_field == ConverterField::ToUnit,
        body,
    )?;
    draw_field(
        display,
        386,
        t(locale, "Step", "Passo"),
        &step,
        converter.active_field == ConverterField::StepSize,
        body,
    )?;

    Rectangle::new(Point::new(22, 474), Size::new(436, 152))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2))
        .draw(display)?;
    Text::new(t(locale, "RESULT", "RISULTATO"), Point::new(42, 514), body).draw(display)?;
    Text::new(&result, Point::new(42, 578), state.display.large_style()).draw(display)?;

    draw_footer(display, state, t(locale, "SELECT NEXT", "SELECT AVANTI"))?;
    Ok(())
}

fn draw_field(
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
    Rectangle::new(Point::new(22, top), Size::new(436, 52))
        .into_styled(border)
        .draw(display)?;
    Text::new(
        if selected { ">" } else { " " },
        Point::new(38, top + 34),
        style,
    )
    .draw(display)?;
    Text::new(label, Point::new(68, top + 34), style).draw(display)?;
    Text::new(value, Point::new(212, top + 34), style).draw(display)?;
    Ok(())
}

fn result_label(locale: Locale, result: ConversionResult, symbol: &str) -> String {
    match result {
        ConversionResult::Value(value) => format!("{} {symbol}", format_milli(value)),
        ConversionResult::OverRange => t(locale, "OVER RANGE", "FUORI INTERVALLO").into(),
        ConversionResult::Invalid => t(locale, "INVALID", "NON VALIDO").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::render_unit_converter;
    use crate::{app::AppState, framebuffer::FrameBuffer, orientation::OrientedFrameBuffer};

    #[test]
    fn renders_default_offline_converter() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        render_unit_converter(&mut display, &AppState::default()).unwrap();
    }
}
