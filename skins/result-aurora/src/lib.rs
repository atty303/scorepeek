use scorepeek_skin_shared::Theme;

static THEME: Theme = Theme {
    graph_score: "#f4d174",
    graph_miss: "#df74ff",
    frame_source: [160.0, 160.0],
    frame_factor: 0.24,
    selection_illumination: true,
    status_edge: "#c2a660",
    lamp_accent: "#c16aff",
    rail_edge: "#c2a660",
    rail_inner: "#b383cc",
    rank_material: "type.png",
    label_font: "Oxanium",
    label_descent: 6.2,
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
