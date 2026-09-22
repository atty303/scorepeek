use super::*;

fn paint_screen_reference(pixels: &mut [u8], encoded: &[u8]) {
    let (header, reference) = qoi::decode_to_vec(encoded).unwrap();
    assert_eq!((header.width, header.height), (410, 60));
    for y in 0..header.height as usize {
        let source_start = y * header.width as usize * 3;
        let target_start = ((50 + y) * CANONICAL_WIDTH as usize + 50) * 3;
        pixels[target_start..target_start + header.width as usize * 3]
            .copy_from_slice(&reference[source_start..source_start + header.width as usize * 3]);
    }
}

fn paint_music_reference(pixels: &mut [u8]) {
    paint_screen_reference(
        pixels,
        include_bytes!("../../../assets/screen-references-v1/music-select.qoi"),
    );
}

fn test_frame(pixels: Vec<u8>) -> CanonicalFrame {
    CanonicalFrame {
        pixels: pixels.into(),
        source_pts_ms: 0,
        decode_index: 0,
        capture_profile_id: "0".repeat(64),
        normalizer_artifact_sha256: "1".repeat(64),
        frame_extraction_sha256: "2".repeat(64),
    }
}

fn paint_title_bbox(pixels: &mut [u8], filled: bool) {
    let roi = ScreenPathLayout::load().unwrap().title.version;
    let bbox = Roi {
        x: 10,
        y: 9,
        width: 232,
        height: 14,
    };
    for y in bbox.y..bbox.y + bbox.height {
        for x in bbox.x..bbox.x + bbox.width {
            if filled
                || x == bbox.x
                || x == bbox.x + bbox.width - 1
                || y == bbox.y
                || y == bbox.y + bbox.height - 1
            {
                let frame_x = usize::try_from(roi.x + x).unwrap();
                let frame_y = usize::try_from(roi.y + y).unwrap();
                let offset = (frame_y * CANONICAL_WIDTH as usize + frame_x) * 3;
                pixels[offset..offset + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
    }
}

#[test]
fn title_presence_uses_the_bright_bounding_box_not_bright_pixel_count() {
    for filled in [false, true] {
        let mut pixels = vec![0; CANONICAL_BYTES];
        paint_title_bbox(&mut pixels, filled);
        let observation = inspect_canonical_rgb8(&pixels).unwrap();
        assert_eq!(observation.screen, ScreenClass::Unknown);
        assert_eq!(
            observation.title_presence.bright_bbox,
            Some(Roi {
                x: 10,
                y: 9,
                width: 232,
                height: 14,
            })
        );
        assert!(observation.title_presence.qualifies);
    }

    let mut pixels = vec![0; CANONICAL_BYTES];
    paint_title_bbox(&mut pixels, false);
    paint_play_presence(&mut pixels, &ScreenPathLayout::load().unwrap());
    let overlapping = inspect_canonical_rgb8(&pixels).unwrap();
    assert_eq!(overlapping.screen, ScreenClass::Play);
    assert!(overlapping.title_presence.qualifies);

    let mut pixels = vec![0; CANONICAL_BYTES];
    paint_title_bbox(&mut pixels, false);
    let roi = ScreenPathLayout::load().unwrap().title.version;
    let offset = (usize::try_from(roi.y + 8).unwrap() * CANONICAL_WIDTH as usize
        + usize::try_from(roi.x + 10).unwrap())
        * 3;
    pixels[offset..offset + 3].copy_from_slice(&[255, 255, 255]);
    assert!(
        !inspect_canonical_rgb8(&pixels)
            .unwrap()
            .title_presence
            .qualifies
    );
}

fn paint_result_presence(pixels: &mut [u8], layout: &CanonicalLayout) {
    paint_result_presence_on(pixels, layout, ResultPanelSide::Left);
}

fn paint_result_presence_on(pixels: &mut [u8], layout: &CanonicalLayout, side: ResultPanelSide) {
    for index in 0..layout.result.presence.warm_pixels_min as usize {
        let x = layout.result.header.x as usize + index % layout.result.header.width as usize;
        let y = layout.result.header.y as usize + index / layout.result.header.width as usize;
        pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&[140, 100, 60]);
    }
    for edge in [
        layout.result.upper_panel_edge,
        layout.result.lower_panel_edge,
    ] {
        let edge = edge
            .translated_x(layout.result.panel_origins.get(side))
            .unwrap();
        for x in 0..layout.result.presence.horizontal_edge_pixels_min as usize {
            let upper = (edge.y as usize * CANONICAL_WIDTH as usize + edge.x as usize + x) * 3;
            let lower = upper + CANONICAL_WIDTH as usize * 3;
            pixels[upper..upper + 3].copy_from_slice(&[0, 0, 0]);
            pixels[lower..lower + 3].copy_from_slice(&[255, 255, 255]);
        }
    }
}

fn paint_decide_transition_presence(pixels: &mut [u8], layout: &ScreenPathLayout) {
    let roi = layout.decide_transition.splash;
    let paint = |pixels: &mut [u8], index: usize, color: [u8; 3]| {
        let x = roi.x as usize + index % roi.width as usize;
        let y = roi.y as usize + index / roi.width as usize;
        pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&color);
    };
    for index in 0..layout.decide_transition.presence.cyan_pixels_min as usize {
        paint(pixels, index, [20, 160, 220]);
    }
    let saturated_remainder = layout
        .decide_transition
        .presence
        .saturated_pixels_min
        .saturating_sub(layout.decide_transition.presence.cyan_pixels_min);
    for index in 0..saturated_remainder as usize {
        paint(
            pixels,
            layout.decide_transition.presence.cyan_pixels_min as usize + index,
            [220, 40, 40],
        );
    }
    for index in 0..layout.decide_transition.presence.bright_pixels_min as usize {
        paint(
            pixels,
            layout.decide_transition.presence.saturated_pixels_min as usize + index,
            [220, 220, 220],
        );
    }
}

fn paint_play_presence(pixels: &mut [u8], layout: &ScreenPathLayout) {
    let roi = layout.play.bpm_outline_search;
    paint_play_outline(pixels, roi.x as usize + 606, roi.y as usize + 12);
}

fn paint_play_outline(pixels: &mut [u8], origin_x: usize, origin_y: usize) {
    for y in 0..71_usize {
        let ranges = if y < 9 {
            let start = 31 - y;
            [(start, start + 287 + y * 2), (0, 0)]
        } else if y >= 64 {
            let row = y - 64;
            let start = 17 + row;
            [(start, start + 315 - row * 2), (0, 0)]
        } else {
            [(31, 39), (310, 318)]
        };
        for (start, end) in ranges {
            for x in start..end {
                let frame_x = origin_x + x;
                let frame_y = origin_y + y;
                pixels[(frame_y * CANONICAL_WIDTH as usize + frame_x) * 3..][..3]
                    .copy_from_slice(&[20, 100, 150]);
            }
        }
    }
}

fn paint_play_edge_pair(pixels: &mut [u8], origin_x: usize, origin_y: usize) {
    for y in 0..6_usize {
        for x in 31..318_usize {
            let index = ((origin_y + y) * CANONICAL_WIDTH as usize + origin_x + x) * 3;
            pixels[index..index + 3].copy_from_slice(&[20, 100, 150]);
        }
    }
    for y in 64..70_usize {
        for x in 17..332_usize {
            let index = ((origin_y + y) * CANONICAL_WIDTH as usize + origin_x + x) * 3;
            pixels[index..index + 3].copy_from_slice(&[20, 100, 150]);
        }
    }
}

fn assert_selected_active_title_layout(
    canonical: &CanonicalLayout,
    context: &IntegratedContextLayout,
) {
    let active_list_slot = canonical.music_select.list_titles.rois().nth(10).unwrap();
    assert_eq!(
        context.music_select.active_list_title,
        Roi {
            x: 1305,
            y: 525,
            width: 505,
            height: 30,
        }
    );
    assert!(context.music_select.active_list_title.x < active_list_slot.x);
    assert!(context.music_select.active_list_title.y >= active_list_slot.y);
    assert!(
        context.music_select.active_list_title.y + context.music_select.active_list_title.height
            <= active_list_slot.y + active_list_slot.height
    );
    assert_eq!(
        context.music_select.active_list_title.x + context.music_select.active_list_title.width,
        active_list_slot.x + active_list_slot.width
    );
}

#[test]
fn canonical_layout_is_bounded_and_hash_stable() {
    let layout = CanonicalLayout::load().unwrap();
    assert_eq!(
        layout.result.title,
        Roi {
            x: 660,
            y: 950,
            width: 600,
            height: 50
        }
    );
    assert_eq!(
        layout.result.artist,
        Roi {
            x: 650,
            y: 990,
            width: 650,
            height: 40
        }
    );
    assert_eq!(CanonicalLayout::sha256().len(), 64);
}

#[test]
fn crop_uses_canonical_row_major_coordinates() {
    let mut pixels = vec![0_u8; CANONICAL_BYTES];
    let offset = (10 * CANONICAL_WIDTH as usize + 20) * 3;
    pixels[offset..offset + 3].copy_from_slice(&[1, 2, 3]);
    let frame = test_frame(pixels);
    assert_eq!(
        frame
            .crop(Roi {
                x: 20,
                y: 10,
                width: 1,
                height: 1
            })
            .unwrap(),
        [1, 2, 3]
    );
}

#[test]
fn active_title_foreground_uses_fixed_gray_bbox_and_horizontal_margin() {
    let mut pixels = vec![0_u8; 20 * 4 * 3];
    for y in 1..=2 {
        for x in 8..=10 {
            let offset = (y * 20 + x) * 3;
            pixels[offset..offset + 3].copy_from_slice(&[255, 255, 255]);
        }
    }
    let crop = Rgb8Crop {
        roi: Roi {
            x: 100,
            y: 200,
            width: 20,
            height: 4,
        },
        pixels,
    };
    let (foreground, geometry) = crop.title_foreground_crop().unwrap();
    assert_eq!(
        foreground.roi,
        Roi {
            x: 104,
            y: 200,
            width: 11,
            height: 4
        }
    );
    assert_eq!(
        geometry.bbox,
        Roi {
            x: 108,
            y: 201,
            width: 3,
            height: 2
        }
    );
    assert_eq!(geometry.occupancy_width_ppm, 150_000);
    assert!(!geometry.touches_left_edge);
    assert!(!geometry.touches_right_edge);

    let empty = Rgb8Crop {
        roi: Roi {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        },
        pixels: vec![0; 12],
    };
    assert!(empty.title_foreground_crop().is_none());
}

#[test]
fn screen_field_observations_keep_complete_screen_specific_shapes() {
    let result_crops = route_screen_rgb8_crops(
        &vec![0; CANONICAL_BYTES],
        ScreenCropRoute::Result(ResultPanelSide::Left),
    )
    .unwrap();
    let mut result_calls = 0;
    let result = observe_screen_fields(&result_crops, |_, crop| {
        result_calls += 1;
        Ok::<_, ()>(DynamicTextObservation {
            input_width: crop.roi.width as usize,
            output_timesteps: result_calls,
            open_text: format!("result-{result_calls}"),
            constrained_text: None,
        })
    })
    .unwrap();
    let ScreenFieldObservations::Result(result) = result else {
        panic!("result crops produced another screen output");
    };
    assert_eq!(result_calls, 21);
    assert_eq!(result.title.open_text, "result-1");
    assert_eq!(result.artist.open_text, "result-2");
    assert_eq!(result.clear_type.open_text, "result-3");
    assert_eq!(result.difficulty.open_text, "result-4");
    assert_eq!(result.play_type.open_text, "result-5");
    assert_eq!(result.level.open_text, "result-6");
    assert_eq!(result.notes.open_text, "result-7");
    assert_eq!(result.current_score.open_text, "result-8");
    assert_eq!(result.previous_clear_type.open_text, "result-9");
    assert_eq!(result.previous_score.open_text, "result-10");
    assert_eq!(result.previous_miss_count.open_text, "result-11");
    assert_eq!(result.miss_count.open_text, "result-12");
    assert_eq!(result.pgreat.open_text, "result-13");
    assert_eq!(result.great.open_text, "result-14");
    assert_eq!(result.good.open_text, "result-15");
    assert_eq!(result.bad.open_text, "result-16");
    assert_eq!(result.poor.open_text, "result-17");
    assert_eq!(result.fast.open_text, "result-18");
    assert_eq!(result.slow.open_text, "result-19");
    assert_eq!(result.combo_break.open_text, "result-20");

    let music_crops =
        route_screen_rgb8_crops(&vec![0; CANONICAL_BYTES], ScreenCropRoute::MusicSelect).unwrap();
    let mut music_calls = 0;
    let music = observe_screen_fields(&music_crops, |_, crop| {
        music_calls += 1;
        Ok::<_, ()>(DynamicTextObservation {
            input_width: crop.roi.width as usize,
            output_timesteps: music_calls,
            open_text: format!("music-{music_calls}"),
            constrained_text: None,
        })
    })
    .unwrap();
    let ScreenFieldObservations::MusicSelect(music) = music else {
        panic!("music-select crops produced another screen output");
    };
    assert_eq!(music_calls, 3);
    assert_eq!(music.central_title.open_text, "music-1");
    assert_eq!(music.artist.open_text, "music-2");
    assert_eq!(
        music.selected_difficulty.state,
        MusicSelectDifficultyState::Unknown(MusicSelectDifficultyUnknownReason::NoCandidate)
    );
    assert_eq!(
        music.play_side.state,
        MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::NoCandidate)
    );
    assert_eq!(music.active_list_title.open_text, "music-3");
}

#[test]
fn result_crop_router_translates_only_panel_local_fields() {
    let pixels = vec![0; CANONICAL_BYTES];
    let ScreenRgb8Crops::Result(left) =
        route_screen_rgb8_crops(&pixels, ScreenCropRoute::Result(ResultPanelSide::Left)).unwrap()
    else {
        unreachable!();
    };
    let ScreenRgb8Crops::Result(right) =
        route_screen_rgb8_crops(&pixels, ScreenCropRoute::Result(ResultPanelSide::Right)).unwrap()
    else {
        unreachable!();
    };
    for (left, right) in [
        (&left.title, &right.title),
        (&left.artist, &right.artist),
        (&left.difficulty, &right.difficulty),
        (&left.play_type, &right.play_type),
        (&left.level, &right.level),
        (&left.notes, &right.notes),
    ] {
        assert_eq!(left.roi, right.roi);
    }
    for (left, right) in [
        (&left.clear_type, &right.clear_type),
        (&left.previous_clear_type, &right.previous_clear_type),
        (&left.play_options, &right.play_options),
    ] {
        assert_eq!(right.roi.x, left.roi.x + 1_360);
        assert_eq!(right.roi.y, left.roi.y);
        assert_eq!(right.roi.width, left.roi.width);
        assert_eq!(right.roi.height, left.roi.height);
    }
    for (left, right, origin) in [
        (&left.current_score, &right.current_score, 1_350),
        (&left.previous_score, &right.previous_score, 1_350),
        (&left.previous_miss_count, &right.previous_miss_count, 1_350),
        (&left.miss_count, &right.miss_count, 1_350),
        (&left.pgreat, &right.pgreat, 1_349),
        (&left.great, &right.great, 1_349),
        (&left.good, &right.good, 1_349),
        (&left.bad, &right.bad, 1_349),
        (&left.poor, &right.poor, 1_349),
        (&left.fast, &right.fast, 1_344),
        (&left.slow, &right.slow, 1_344),
        (&left.combo_break, &right.combo_break, 1_350),
    ] {
        assert_eq!(right.roi.x, left.roi.x + origin);
        assert_eq!(right.roi.y, left.roi.y);
        assert_eq!(right.roi.width, left.roi.width);
        assert_eq!(right.roi.height, left.roi.height);
    }
}

fn side_crop(bright_pixels: usize) -> Rgb8Crop {
    let roi = Roi {
        x: 0,
        y: 0,
        width: 140,
        height: 18,
    };
    let mut pixels = vec![0; 140 * 18 * 3];
    for pixel in pixels.chunks_exact_mut(3).take(bright_pixels) {
        pixel.fill(200);
    }
    Rgb8Crop { roi, pixels }
}

fn side_crops(one_player: usize, two_player: usize) -> MusicSelectPlaySideCrops {
    MusicSelectPlaySideCrops {
        one_player: side_crop(one_player),
        two_player: side_crop(two_player),
    }
}

#[test]
fn music_select_footer_resolves_each_play_side() {
    assert_eq!(
        observe_music_select_play_side(&side_crops(380, 250)).known(),
        Some(PlaySide::OnePlayer)
    );
    assert_eq!(
        observe_music_select_play_side(&side_crops(250, 380)).known(),
        Some(PlaySide::TwoPlayer)
    );
}

#[test]
fn music_select_footer_fails_closed_for_absence_multiple_and_small_margin() {
    assert_eq!(
        observe_music_select_play_side(&side_crops(0, 0)).state,
        MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::NoCandidate)
    );
    assert_eq!(
        observe_music_select_play_side(&side_crops(330, 330)).state,
        MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::MultipleCandidates)
    );
    assert_eq!(
        observe_music_select_play_side(&side_crops(330, 290)).state,
        MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::InsufficientMargin)
    );
}

fn marker_crop(columns: usize) -> Rgb8Crop {
    let roi = Roi {
        x: 0,
        y: 0,
        width: 128,
        height: 30,
    };
    let mut pixels = vec![0; 128 * 30 * 3];
    for (y, start, width) in [(8, 36, 78), (26, 10, 104)] {
        for x in start..start + width * columns / 100 {
            let offset = (y * 128 + x) * 3;
            pixels[offset..offset + 3].fill(220);
        }
    }
    Rgb8Crop { roi, pixels }
}

fn marker_crops(selected: &[Difficulty]) -> MusicSelectDifficultyMarkerCrops {
    let crop = |difficulty| {
        marker_crop(if selected.contains(&difficulty) {
            100
        } else {
            0
        })
    };
    MusicSelectDifficultyMarkerCrops {
        beginner: crop(Difficulty::Beginner),
        normal: crop(Difficulty::Normal),
        hyper: crop(Difficulty::Hyper),
        another: crop(Difficulty::Another),
        leggendaria: crop(Difficulty::Leggendaria),
    }
}

#[test]
fn fixed_music_select_marker_resolves_each_single_slot() {
    for difficulty in [
        Difficulty::Beginner,
        Difficulty::Normal,
        Difficulty::Hyper,
        Difficulty::Another,
        Difficulty::Leggendaria,
    ] {
        assert_eq!(
            observe_music_select_difficulty(&marker_crops(&[difficulty])).known(),
            Some(difficulty)
        );
    }
}

#[test]
fn fixed_music_select_marker_rejects_absence_multiple_and_broad_background_bands() {
    assert_eq!(
        observe_music_select_difficulty(&marker_crops(&[])).known(),
        None
    );
    assert_eq!(
        observe_music_select_difficulty(&marker_crops(&[Difficulty::Normal, Difficulty::Hyper]))
            .state,
        MusicSelectDifficultyState::Unknown(MusicSelectDifficultyUnknownReason::MultipleCandidates)
    );
    for rows in [0..30, 8..13, 23..28] {
        let mut crops = marker_crops(&[]);
        for y in rows {
            crops.normal.pixels[y * 128 * 3..(y + 1) * 128 * 3].fill(220);
        }
        assert_eq!(observe_music_select_difficulty(&crops).known(), None);
    }
    let mut crops = marker_crops(&[Difficulty::Hyper]);
    crops.normal.pixels.fill(220);
    assert_eq!(
        observe_music_select_difficulty(&crops).known(),
        Some(Difficulty::Hyper)
    );
}

#[test]
fn fixed_music_select_marker_rejects_insufficient_winner_margin() {
    let mut crops = marker_crops(&[]);
    crops.hyper = marker_crop(85);
    crops.normal = marker_crop(79);
    assert_eq!(
        observe_music_select_difficulty(&crops).state,
        MusicSelectDifficultyState::Unknown(MusicSelectDifficultyUnknownReason::InsufficientMargin)
    );
}

#[test]
fn failed_text_field_does_not_construct_a_partial_screen_observation() {
    let crops = route_screen_rgb8_crops(
        &vec![0; CANONICAL_BYTES],
        ScreenCropRoute::Result(ResultPanelSide::Left),
    )
    .unwrap();
    let mut calls = 0;
    let error = observe_screen_fields(&crops, |_, _| {
        calls += 1;
        if calls == 2 {
            Err("runtime-failed")
        } else {
            Ok(DynamicTextObservation {
                input_width: 1,
                output_timesteps: 1,
                open_text: "discarded".to_owned(),
                constrained_text: None,
            })
        }
    })
    .unwrap_err();
    assert_eq!(calls, 2);
    assert_eq!(error.field, ScreenTextField::ResultArtist);
    assert_eq!(error.source_error(), &"runtime-failed");
}

#[test]
fn general_text_numeric_comparison_keeps_its_fixed_character_sets() {
    for field in [
        ScreenTextField::ResultNotes,
        ScreenTextField::ResultCurrentScore,
        ScreenTextField::ResultPgreat,
        ScreenTextField::ResultGreat,
        ScreenTextField::ResultGood,
        ScreenTextField::ResultBad,
        ScreenTextField::ResultPoor,
    ] {
        assert_eq!(field.ctc_character_set(), Some(CtcCharacterSet::Digits));
    }
    assert_eq!(
        ScreenTextField::ResultLevel.ctc_character_set(),
        Some(CtcCharacterSet::DigitsUpToTwo)
    );
    for field in [
        ScreenTextField::ResultPreviousScore,
        ScreenTextField::ResultPreviousMissCount,
        ScreenTextField::ResultMissCount,
        ScreenTextField::ResultFast,
        ScreenTextField::ResultSlow,
    ] {
        assert_eq!(
            field.ctc_character_set(),
            Some(CtcCharacterSet::DigitsAndDashes)
        );
    }
    assert_eq!(
        ScreenTextField::ResultComboBreak.ctc_character_set(),
        Some(CtcCharacterSet::DigitsAndDashesUpToThree)
    );
    assert_eq!(ScreenTextField::ResultTitle.ctc_character_set(), None);
    assert_eq!(
        ScreenTextField::MusicSelectCentralTitle.ctc_character_set(),
        None
    );
}

#[test]
fn result_presence_is_fail_closed() {
    let layout = CanonicalLayout::load().unwrap();
    let mut pixels = vec![0_u8; CANONICAL_BYTES];
    paint_result_presence(&mut pixels, &layout);
    let frame = test_frame(pixels);
    let snapshot = inspect(&frame).unwrap();
    assert_eq!(snapshot.screen, ScreenClass::Result);
    assert_eq!(snapshot.result_presence.warm_pixels, 3_000);
    assert_eq!(
        snapshot.result_presence.panel_side.known(),
        Some(ResultPanelSide::Left)
    );
    assert_eq!(
        snapshot.result_presence.panels[0].upper_panel_edge_pixels,
        490
    );
    assert_eq!(
        snapshot.result_presence.panels[0].lower_panel_edge_pixels,
        490
    );

    let mut right = vec![0_u8; CANONICAL_BYTES];
    paint_result_presence_on(&mut right, &layout, ResultPanelSide::Right);
    let right = inspect(&test_frame(right)).unwrap();
    assert_eq!(right.screen, ScreenClass::Result);
    assert_eq!(
        right.result_presence.panel_side.known(),
        Some(ResultPanelSide::Right)
    );

    let mut both = frame.pixels.to_vec();
    paint_result_presence_on(&mut both, &layout, ResultPanelSide::Right);
    let both = inspect(&test_frame(both)).unwrap();
    assert_eq!(both.screen, ScreenClass::Unknown);
    assert_eq!(
        both.result_presence.panel_side,
        ResultPanelSideState::Unknown(ResultPanelSideUnknownReason::MultipleCandidates)
    );

    let empty = test_frame(vec![0_u8; CANONICAL_BYTES]);
    assert_eq!(inspect(&empty).unwrap().screen, ScreenClass::Unknown);

    let mut ambiguous = frame.pixels.to_vec();
    for index in 0..layout.music_select.presence.cyan_header_pixels_min as usize {
        let x = index % 600;
        let y = index / 600;
        ambiguous[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&[20, 160, 220]);
    }
    for index in 0..layout.music_select.presence.colored_level_pixels_min as usize {
        let x = 1_320 + index % 30;
        let y = index / 30;
        ambiguous[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&[20, 180, 40]);
    }
    for index in 0..layout.music_select.presence.bright_label_pixels_min as usize {
        let x =
            layout.music_select.label.x as usize + index % layout.music_select.label.width as usize;
        let y =
            layout.music_select.label.y as usize + index / layout.music_select.label.width as usize;
        ambiguous[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&[220, 220, 220]);
    }
    paint_music_reference(&mut ambiguous);
    assert_eq!(
        inspect(&test_frame(ambiguous)).unwrap().screen,
        ScreenClass::Unknown
    );
}

#[test]
fn result_presence_does_not_depend_on_the_background_palette() {
    let layout = CanonicalLayout::load().unwrap();
    for background in [[0, 0, 0], [170, 20, 20], [20, 80, 190], [220, 220, 220]] {
        let mut pixels = Vec::with_capacity(CANONICAL_BYTES);
        for _ in 0..CANONICAL_BYTES / 3 {
            pixels.extend_from_slice(&background);
        }
        paint_result_presence(&mut pixels, &layout);
        assert_eq!(
            inspect(&test_frame(pixels)).unwrap().screen,
            ScreenClass::Result
        );
    }
}

#[test]
fn decide_transition_and_play_presence_are_exactly_one_fail_closed() {
    let layout = ScreenPathLayout::load().unwrap();
    let mut decide = vec![0_u8; CANONICAL_BYTES];
    paint_decide_transition_presence(&mut decide, &layout);
    let decide = inspect(&test_frame(decide)).unwrap();
    assert_eq!(decide.screen, ScreenClass::DecideTransition);
    assert_eq!(
        decide.decide_transition_presence.cyan_pixels,
        layout.decide_transition.presence.cyan_pixels_min
    );
    assert_eq!(
        decide.decide_transition_presence.bright_pixels,
        layout.decide_transition.presence.bright_pixels_min
    );

    let mut play = vec![0_u8; CANONICAL_BYTES];
    paint_play_presence(&mut play, &layout);
    let play = inspect(&test_frame(play)).unwrap();
    assert_eq!(play.screen, ScreenClass::Play);
    assert_eq!(play.play_presence.qualifying_candidates, 1);
    let outline = play
        .play_presence
        .candidates
        .iter()
        .flatten()
        .next()
        .unwrap();
    assert!((280..=305).contains(&outline.top_edge_pixels));
    assert!((300..=320).contains(&outline.bottom_edge_pixels));
    assert!((59..=70).contains(&(outline.bottom_y - outline.top_y)));

    let mut color_area_only = vec![0_u8; CANONICAL_BYTES];
    let roi = layout.play.bpm_outline_search;
    for y in 0..20_usize {
        for x in 0..220_usize {
            let index = ((roi.y as usize + y) * CANONICAL_WIDTH as usize + roi.x as usize + x) * 3;
            color_area_only[index..index + 3].copy_from_slice(&[20, 100, 150]);
        }
    }
    assert_eq!(
        inspect(&test_frame(color_area_only)).unwrap().screen,
        ScreenClass::Unknown
    );

    let mut former_graph_panel = vec![0_u8; CANONICAL_BYTES];
    for roi in [
        Roi {
            x: 1_505,
            y: 0,
            width: 410,
            height: 24,
        },
        Roi {
            x: 1_508,
            y: 160,
            width: 140,
            height: 22,
        },
    ] {
        for y in roi.y..roi.y + roi.height {
            for x in roi.x..roi.x + roi.width {
                let index = (y as usize * CANONICAL_WIDTH as usize + x as usize) * 3;
                former_graph_panel[index..index + 3].copy_from_slice(&[210, 150, 20]);
            }
        }
    }
    assert_eq!(
        inspect(&test_frame(former_graph_panel)).unwrap().screen,
        ScreenClass::Unknown
    );

    let mut overlap = vec![0_u8; CANONICAL_BYTES];
    paint_decide_transition_presence(&mut overlap, &layout);
    paint_play_presence(&mut overlap, &layout);
    assert_eq!(
        inspect(&test_frame(overlap)).unwrap().screen,
        ScreenClass::Unknown
    );
}

#[test]
fn bpm_outline_accepts_all_measured_positions_and_rejects_solid_panels() {
    // Positions measured independently from canonical captures, not derived from the ROI.
    for origin_x in [298, 715, 778, 866, 1283] {
        let mut pixels = vec![0_u8; CANONICAL_BYTES];
        paint_play_outline(&mut pixels, origin_x, 952);
        assert_eq!(
            inspect(&test_frame(pixels)).unwrap().screen,
            ScreenClass::Play
        );

        let mut solid = vec![0_u8; CANONICAL_BYTES];
        for y in 952..1023 {
            for x in origin_x..origin_x + 340 {
                let index = (y * CANONICAL_WIDTH as usize + x) * 3;
                solid[index..index + 3].copy_from_slice(&[20, 100, 150]);
            }
        }
        assert_eq!(
            inspect(&test_frame(solid)).unwrap().screen,
            ScreenClass::Unknown
        );
    }
}

#[test]
fn bpm_outline_ignores_connected_interior_judge_pixels() {
    let mut pixels = vec![0_u8; CANONICAL_BYTES];
    paint_play_outline(&mut pixels, 715, 952);
    for y in 970..1010 {
        for x in 900..1450 {
            let index = (y * CANONICAL_WIDTH as usize + x) * 3;
            pixels[index..index + 3].copy_from_slice(&[20, 100, 150]);
        }
    }
    let observation = inspect(&test_frame(pixels)).unwrap();
    assert_eq!(observation.screen, ScreenClass::Play);
    assert_eq!(observation.play_presence.qualifying_candidates, 1);
}

#[test]
fn bpm_outline_rejects_simultaneous_left_and_center_right_candidates() {
    let mut pixels = vec![0_u8; CANONICAL_BYTES];
    paint_play_outline(&mut pixels, 298, 952);
    paint_play_outline(&mut pixels, 866, 952);
    let observation = inspect(&test_frame(pixels)).unwrap();
    assert_eq!(observation.screen, ScreenClass::Unknown);
    assert_eq!(observation.play_presence.qualifying_candidates, 2);
}

#[test]
fn bpm_outline_rejects_vertically_separated_candidates_at_the_same_center() {
    let mut pixels = vec![0_u8; CANONICAL_BYTES];
    paint_play_edge_pair(&mut pixels, 715, 940);
    paint_play_edge_pair(&mut pixels, 715, 1010);
    let observation = inspect(&test_frame(pixels)).unwrap();
    assert_eq!(observation.screen, ScreenClass::Unknown);
    assert_eq!(observation.play_presence.qualifying_candidates, 2);
}

#[test]
fn bpm_outline_ignores_loading_and_variable_tempo_interior() {
    let layout = ScreenPathLayout::load().unwrap();
    let mut loading = vec![0_u8; CANONICAL_BYTES];
    paint_play_presence(&mut loading, &layout);
    assert_eq!(
        inspect(&test_frame(loading.clone())).unwrap().screen,
        ScreenClass::Play
    );

    let roi = layout.play.bpm_outline_search;
    let mut variable_tempo = loading;
    for (x, width) in [(105_u32, 28_u32), (185, 45), (275, 28)] {
        for y in 47..57_u32 {
            for local_x in x..x + width {
                let frame_x = roi.x + 606 + local_x;
                let frame_y = roi.y + 12 + y;
                let index = (frame_y as usize * CANONICAL_WIDTH as usize + frame_x as usize) * 3;
                variable_tempo[index..index + 3].copy_from_slice(&[210, 210, 210]);
            }
        }
    }
    assert_eq!(
        inspect(&test_frame(variable_tempo)).unwrap().screen,
        ScreenClass::Play
    );
}

#[test]
fn music_select_presence_and_crops_are_fail_closed_and_layout_bound() {
    let layout = CanonicalLayout::load().unwrap();
    let mut pixels = vec![0_u8; CANONICAL_BYTES];
    for index in 0..layout.music_select.presence.cyan_header_pixels_min as usize {
        let x = layout.music_select.header.x as usize
            + index % layout.music_select.header.width as usize;
        let y = layout.music_select.header.y as usize
            + index / layout.music_select.header.width as usize;
        pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&[20, 160, 220]);
    }
    for index in 0..layout.music_select.presence.colored_level_pixels_min as usize {
        let x = layout.music_select.level_column.x as usize
            + index % layout.music_select.level_column.width as usize;
        let y = layout.music_select.level_column.y as usize
            + index / layout.music_select.level_column.width as usize;
        pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&[20, 180, 40]);
    }
    assert_eq!(
        inspect(&test_frame(pixels.clone())).unwrap().screen,
        ScreenClass::Unknown
    );
    for index in 0..layout.music_select.presence.bright_label_pixels_min as usize {
        let x =
            layout.music_select.label.x as usize + index % layout.music_select.label.width as usize;
        let y =
            layout.music_select.label.y as usize + index / layout.music_select.label.width as usize;
        pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&[220, 220, 220]);
    }
    let mut mode_select = pixels.clone();
    paint_screen_reference(
        &mut mode_select,
        include_bytes!("../../../assets/screen-references-v1/mode-select.qoi"),
    );
    let mode_snapshot = inspect(&test_frame(mode_select)).unwrap();
    assert_eq!(mode_snapshot.screen, ScreenClass::ModeSelect);
    assert!(mode_snapshot.music_select_presence.reference_evaluated);
    assert!(
        mode_snapshot
            .music_select_presence
            .mode_select_reference_score_ppm
            > mode_snapshot
                .music_select_presence
                .music_reference_score_ppm
    );
    paint_music_reference(&mut pixels);
    let frame = test_frame(pixels);
    let snapshot = inspect(&frame).unwrap();
    assert_eq!(
        snapshot.screen,
        ScreenClass::MusicSelect,
        "{:?}",
        snapshot.music_select_presence
    );
    assert!(snapshot.music_select_presence.cyan_header_pixels >= 7_000);
    assert_eq!(snapshot.music_select_presence.colored_level_pixels, 1_000);
    assert!(snapshot.music_select_presence.bright_label_pixels >= 4_000);
    assert!(snapshot.music_select_presence.reference_evaluated);
    assert_eq!(
        snapshot.music_select_presence.music_reference_score_ppm,
        1_000_000
    );
    assert!(
        snapshot.music_select_presence.music_reference_score_ppm
            > snapshot
                .music_select_presence
                .mode_select_reference_score_ppm
    );
}

#[test]
fn integrated_context_layout_keeps_the_base_layout_stable() {
    let canonical = CanonicalLayout::load().unwrap();
    let context = IntegratedContextLayout::load().unwrap();
    assert_eq!(context.result.artist, canonical.result.artist);
    assert_selected_active_title_layout(&canonical, &context);
}
