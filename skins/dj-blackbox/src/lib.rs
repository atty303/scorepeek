use scorepeek_skin_shared::Theme;

static THEME: Theme = Theme {
    graph_score: "#c6f139",
    graph_miss: "#ff9e30",
    frame_source: [260.0, 120.0],
    frame_factor: 0.22,
    selection_illumination: false,
    status_edge: "#687067",
    lamp_accent: "#c5e819",
    rail_edge: "#8c918c",
    rail_inner: "#444943",
    rank_material: "type.png",
    label_font: "Rajdhani",
    label_descent: 6.16,
};

#[unsafe(no_mangle)]
pub extern "C" fn scorepeek_alloc(length: i32) -> i32 {
    scorepeek_skin_shared::allocate(length)
}

#[unsafe(no_mangle)]
pub extern "C" fn scorepeek_dealloc(pointer: i32, length: i32) {
    scorepeek_skin_shared::deallocate(pointer, length);
}

#[unsafe(no_mangle)]
pub extern "C" fn scorepeek_init(pointer: i32, length: i32) -> i64 {
    scorepeek_skin_shared::render(pointer, length, &THEME)
}

#[unsafe(no_mangle)]
pub extern "C" fn scorepeek_render(pointer: i32, length: i32) -> i64 {
    scorepeek_skin_shared::render(pointer, length, &THEME)
}
