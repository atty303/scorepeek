use smithay_client_toolkit::reexports::protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{io, io::Write as _, os::unix::net::UnixStream, path::Path};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_pointer, wl_registry},
};

struct State;

delegate_noop!(State: ignore ZwlrVirtualPointerManagerV1);
delegate_noop!(State: ignore ZwlrVirtualPointerV1);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _connection: &Connection,
        _handle: &QueueHandle<Self>,
    ) {
    }
}

fn timestamp(started: Instant) -> u32 {
    u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX)
}

fn unix_us() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
}

fn move_to(pointer: &ZwlrVirtualPointerV1, started: Instant, x: u32, y: u32) {
    pointer.motion_absolute(timestamp(started), x, y, 3200, 1080);
    pointer.frame();
}

fn button(
    pointer: &ZwlrVirtualPointerV1,
    started: Instant,
    button: u32,
    state: wl_pointer::ButtonState,
) {
    pointer.button(timestamp(started), button, state);
    pointer.frame();
}

fn click(pointer: &ZwlrVirtualPointerV1, started: Instant, position: [u32; 2], code: u32) {
    move_to(pointer, started, position[0], position[1]);
    button(pointer, started, code, wl_pointer::ButtonState::Pressed);
    button(pointer, started, code, wl_pointer::ButtonState::Released);
}

fn visibility_count(log: &Path, active: bool) -> std::io::Result<usize> {
    let expected = format!("\"active\":{active}");
    Ok(std::fs::read_to_string(log)?
        .lines()
        .filter(|line| {
            line.contains("\"operation\":\"native_surface_visibility\"")
                && line.contains("\"canvas_id\":\"wayland-selection\"")
                && line.contains(&expected)
        })
        .count())
}

fn drive_screen_transition(
    trigger: &mut UnixStream,
    log: &Path,
    screen: &str,
    active: bool,
) -> std::io::Result<()> {
    let before = visibility_count(log, active)?;
    writeln!(trigger, "{screen}")?;
    trigger.flush()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if visibility_count(log, active)? > before {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!("selection canvas did not reach active={active} after {screen}"),
    ))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let feed_trigger = args
        .next()
        .ok_or("usage: virtual_pointer FEED_TRIGGER_SOCKET DIAGNOSTIC_LOG")?;
    let diagnostic_log = args
        .next()
        .ok_or("usage: virtual_pointer FEED_TRIGGER_SOCKET DIAGNOSTIC_LOG")?;
    if args.next().is_some() {
        return Err("usage: virtual_pointer FEED_TRIGGER_SOCKET DIAGNOSTIC_LOG".into());
    }
    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<State>(&connection)?;
    let handle: QueueHandle<State> = queue.handle();
    let manager: ZwlrVirtualPointerManagerV1 = globals.bind(&handle, 1..=2, ())?;
    let pointer = manager.create_virtual_pointer(None, &handle, ());
    queue.roundtrip(&mut State)?;
    let started = Instant::now();
    println!(
        "{{\"operation\":\"nested_wayland_scenario\",\"action\":\"pointer-input-started\",\"timestamp_unix_us\":{}}}",
        unix_us()
    );
    io::stdout().flush()?;

    for _ in 0..16 {
        for ([from_x, from_y], [to_x, to_y]) in
            [([500, 150], [532, 166]), ([2_420, 150], [2_452, 166])]
        {
            move_to(&pointer, started, from_x, from_y);
            button(&pointer, started, 0x110, wl_pointer::ButtonState::Pressed);
            move_to(&pointer, started, to_x, to_y);
            button(&pointer, started, 0x110, wl_pointer::ButtonState::Released);
        }
        for [x, y] in [[100, 265], [100, 305], [2_020, 265], [2_020, 305]] {
            move_to(&pointer, started, x, y);
            button(&pointer, started, 0x110, wl_pointer::ButtonState::Pressed);
            button(&pointer, started, 0x110, wl_pointer::ButtonState::Released);
        }
        move_to(&pointer, started, 100, 500);
        pointer.axis(timestamp(started), wl_pointer::Axis::VerticalScroll, 24.0);
        pointer.frame();
        connection.flush()?;
        std::thread::sleep(Duration::from_millis(250));
    }
    // The controller scenario has finished its move/visibility/delete sequence by
    // this point. Exercise editor -> display -> editor inside the same native run.
    std::thread::sleep(Duration::from_secs(3));
    // Scroll's headless output layout places the active 1280x720 editor stage
    // at the origin. Its action bar is anchored to the bottom; use Discard so
    // the lifecycle exercise is independent of any in-progress field validity.
    click(&pointer, started, [140, 700], 0x110);
    connection.flush()?;
    let mut trigger = UnixStream::connect(feed_trigger)?;
    let diagnostic_log = Path::new(&diagnostic_log);
    drive_screen_transition(&mut trigger, diagnostic_log, "music_select", true)?;
    drive_screen_transition(&mut trigger, diagnostic_log, "play", false)?;
    drive_screen_transition(&mut trigger, diagnostic_log, "music_select", true)?;
    drop(trigger);
    // Reopen only after the second music-select activation has crossed the
    // production display path and emitted its active acknowledgement.
    // Scroll places HEADLESS-1 to the right of the 1280-wide active output.
    click(&pointer, started, [1_320, 40], 0x111);
    connection.flush()?;
    std::thread::sleep(Duration::from_secs(1));
    pointer.destroy();
    manager.destroy();
    connection.flush()?;
    println!(
        "{{\"operation\":\"nested_wayland_scenario\",\"action\":\"pointer-drag-injected\",\"data\":{{\"protocol\":\"zwlr_virtual_pointer_v1\",\"button\":\"primary\",\"axis\":true,\"same_run_close_reopen\":true}}}}"
    );
    io::stdout().flush()?;
    Ok(())
}
