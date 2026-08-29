//! Global Power-key display-maintenance menu.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::{
    app::{
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
    let heading = state.display.heading_style();
    let body = state.display.body_style();

    draw_header(display, state, "POWER KEY")?;

    Text::new("Screen refresh", Point::new(22, 128), heading).draw(display)?;
    Text::new(
        "Run a clean global refresh to clear e-paper ghosting.",
        Point::new(22, 174),
        body,
    )
    .draw(display)?;

    draw_action(
        display,
        246,
        "Clear ghosting now",
        state.power_key_menu.selected == 0,
        body,
    )?;
    draw_action(
        display,
        326,
        "Cancel",
        state.power_key_menu.selected == 1,
        body,
    )?;

    Rectangle::new(Point::new(22, 454), Size::new(436, 104))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;
    Text::new("Long Power press", Point::new(44, 500), heading).draw(display)?;
    Text::new("Enter sleep-image mode", Point::new(44, 534), body).draw(display)?;

    draw_footer(display, state.display, "MOVE  SELECT RUN  BOOT BACK")?;
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
