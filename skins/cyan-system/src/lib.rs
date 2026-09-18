use scorepeek_skin_shared::Theme;

static THEME: Theme = Theme {
    graph_score: "#55e9ff",
    graph_miss: "#ffbd44",
    frame_source: [340.0, 200.0],
    frame_factor: 0.16,
    selection_illumination: false,
    status_edge: "#13dcef",
    lamp_accent: "#10dcfa",
    rail_edge: "#13dcef",
    rail_inner: "#086f83",
    rank_material: "rank-type.png",
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
