use std::thread;
use std::time::{Duration, Instant};

use x11rb::COPY_DEPTH_FROM_PARENT;
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, ImageFormat, PropMode,
    WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;

pub const WIDTH: u32 = 1_920;
pub const HEIGHT: u32 = 1_080;
pub const FIDUCIAL_SIZE: u32 = 48;
const LIFETIME: Duration = Duration::from_secs(30);
const PUT_ROWS: u32 = 32;

pub fn rgb8() -> Box<[u8]> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            pixels.extend_from_slice(&pixel(x, y));
        }
    }
    pixels.into_boxed_slice()
}

fn pixel(x: u32, y: u32) -> [u8; 3] {
    for &(left, top, color) in fiducials() {
        if x >= left && x < left + FIDUCIAL_SIZE && y >= top && y < top + FIDUCIAL_SIZE {
            return color;
        }
    }
    let column = x / 120;
    let row = y / 120;
    [
        u8::try_from((column * 47 + row * 17 + 29) % 224 + 16).expect("marker channel is bounded"),
        u8::try_from((column * 19 + row * 61 + 71) % 224 + 16).expect("marker channel is bounded"),
        u8::try_from((column * 83 + row * 23 + 113) % 224 + 16).expect("marker channel is bounded"),
    ]
}

#[must_use]
pub const fn fiducials() -> &'static [(u32, u32, [u8; 3]); 9] {
    &[
        (48, 48, [255, 0, 255]),
        (936, 48, [0, 255, 255]),
        (1_824, 48, [255, 255, 0]),
        (48, 516, [255, 0, 0]),
        (936, 516, [0, 255, 0]),
        (1_824, 516, [0, 0, 255]),
        (48, 984, [255, 64, 0]),
        (936, 984, [64, 255, 0]),
        (1_824, 984, [0, 64, 255]),
    ]
}

pub fn run_x11() -> Result<(), String> {
    let (connection, screen_index) = x11rb::connect(None)
        .map_err(|error| format!("calibration marker X11 connection failed: {error}"))?;
    let screen = &connection.setup().roots[screen_index];
    let window = connection
        .generate_id()
        .map_err(|error| format!("calibration marker window ID failed: {error}"))?;
    connection
        .create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            screen.root,
            0,
            0,
            u16::try_from(WIDTH).expect("marker width fits X11"),
            u16::try_from(HEIGHT).expect("marker height fits X11"),
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY),
        )
        .map_err(|error| format!("calibration marker window creation failed: {error}"))?
        .check()
        .map_err(|error| format!("calibration marker window was rejected: {error}"))?;
    connection
        .change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            b"scorepeek calibration marker",
        )
        .map_err(|error| format!("calibration marker title failed: {error}"))?;
    let graphics = connection
        .generate_id()
        .map_err(|error| format!("calibration marker graphics ID failed: {error}"))?;
    connection
        .create_gc(graphics, window, &CreateGCAux::new())
        .map_err(|error| format!("calibration marker graphics creation failed: {error}"))?
        .check()
        .map_err(|error| format!("calibration marker graphics were rejected: {error}"))?;
    connection
        .map_window(window)
        .map_err(|error| format!("calibration marker mapping failed: {error}"))?
        .check()
        .map_err(|error| format!("calibration marker mapping was rejected: {error}"))?;

    let rgb = rgb8();
    for top in (0..HEIGHT).step_by(PUT_ROWS as usize) {
        let rows = PUT_ROWS.min(HEIGHT - top);
        let mut bgrx = Vec::with_capacity((WIDTH * rows * 4) as usize);
        for y in top..top + rows {
            for x in 0..WIDTH {
                let offset = ((y * WIDTH + x) * 3) as usize;
                bgrx.extend_from_slice(&[rgb[offset + 2], rgb[offset + 1], rgb[offset], 0]);
            }
        }
        connection
            .put_image(
                ImageFormat::Z_PIXMAP,
                window,
                graphics,
                u16::try_from(WIDTH).expect("marker width fits X11"),
                u16::try_from(rows).expect("marker chunk height fits X11"),
                0,
                i16::try_from(top).expect("marker top fits X11"),
                0,
                screen.root_depth,
                &bgrx,
            )
            .map_err(|error| format!("calibration marker upload failed: {error}"))?;
    }
    connection
        .flush()
        .map_err(|error| format!("calibration marker flush failed: {error}"))?;

    let deadline = Instant::now() + LIFETIME;
    while Instant::now() < deadline {
        if connection
            .poll_for_event()
            .map_err(|error| format!("calibration marker event failed: {error}"))?
            .is_some_and(|event| matches!(event, x11rb::protocol::Event::DestroyNotify(_)))
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{HEIGHT, WIDTH, rgb8};

    #[test]
    fn marker_is_deterministic_rgb8_with_distinct_fiducials() {
        let first = rgb8();
        let second = rgb8();
        assert_eq!(first, second);
        assert_eq!(first.len(), (WIDTH * HEIGHT * 3) as usize);
        let first_fiducial = ((48 * WIDTH + 48) * 3) as usize;
        assert_eq!(&first[first_fiducial..first_fiducial + 3], &[255, 0, 255]);
        let last_fiducial = ((984 * WIDTH + 1_824) * 3) as usize;
        assert_eq!(&first[last_fiducial..last_fiducial + 3], &[0, 64, 255]);
    }
}
