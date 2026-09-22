use super::*;

pub(super) fn horizontal_edge_pixels(pixels: &[u8], width: u32) -> u32 {
    let row_bytes = width as usize * 3;
    pixels[..row_bytes]
        .chunks_exact(3)
        .zip(pixels[row_bytes..].chunks_exact(3))
        .filter(|(upper, lower)| {
            let luma = |pixel: &[u8]| {
                (u32::from(pixel[0]) * 77 + u32::from(pixel[1]) * 150 + u32::from(pixel[2]) * 29)
                    / 256
            };
            luma(upper).abs_diff(luma(lower)) > 45
        })
        .fold(0, |count, _| count + 1)
}

fn result_crop_selections(
    routed: ResultScreenRgb8Crops,
) -> [(ResultCropField, &'static str, Rgb8Crop); 20] {
    [
        (ResultCropField::Title, "title.ppm", routed.title),
        (ResultCropField::Artist, "artist.ppm", routed.artist),
        (
            ResultCropField::ClearType,
            "clear-type.ppm",
            routed.clear_type,
        ),
        (
            ResultCropField::Difficulty,
            "difficulty.ppm",
            routed.difficulty,
        ),
        (ResultCropField::Level, "level.ppm", routed.level),
        (ResultCropField::Notes, "notes.ppm", routed.notes),
        (
            ResultCropField::CurrentScore,
            "current-score.ppm",
            routed.current_score,
        ),
        (
            ResultCropField::PreviousClearType,
            "previous-clear-type.ppm",
            routed.previous_clear_type,
        ),
        (
            ResultCropField::PreviousScore,
            "previous-score.ppm",
            routed.previous_score,
        ),
        (
            ResultCropField::PreviousMissCount,
            "previous-miss-count.ppm",
            routed.previous_miss_count,
        ),
        (
            ResultCropField::MissCount,
            "miss-count.ppm",
            routed.miss_count,
        ),
        (ResultCropField::Pgreat, "pgreat.ppm", routed.pgreat),
        (ResultCropField::Great, "great.ppm", routed.great),
        (ResultCropField::Good, "good.ppm", routed.good),
        (ResultCropField::Bad, "bad.ppm", routed.bad),
        (ResultCropField::Poor, "poor.ppm", routed.poor),
        (ResultCropField::Fast, "fast.ppm", routed.fast),
        (ResultCropField::Slow, "slow.ppm", routed.slow),
        (
            ResultCropField::ComboBreak,
            "combo-break.ppm",
            routed.combo_break,
        ),
        (
            ResultCropField::PlayOptions,
            "play-options.ppm",
            routed.play_options,
        ),
    ]
}

/// Exports the fixed result-layout crops from a validated canonical frame.
///
/// The output directory must not exist. `manifest.json` is written last, so a partial export is
/// never accepted as a complete crop artifact.
///
/// # Errors
/// Returns an error for a non-result screen, an invalid layout, or any output I/O failure.
pub fn export_result_crops(
    frame: &CanonicalFrame,
    frame_id: &str,
    output: impl AsRef<Path>,
) -> Result<ResultCropExportSummary, RecognitionError> {
    if frame_id.is_empty() || frame_id.len() > 256 || frame_id.chars().any(char::is_control) {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let snapshot = inspect(frame)?;
    if snapshot.screen != ScreenClass::Result {
        return Err(RecognitionError::NotResultScreen);
    }
    let output = output.as_ref();
    fs::create_dir(output)?;

    let ScreenRgb8Crops::Result(routed) = route_screen_rgb8_crops(
        frame.pixels(),
        snapshot
            .crop_route()
            .ok_or(RecognitionError::NotResultScreen)?,
    )?
    else {
        return Err(RecognitionError::NotResultScreen);
    };
    let selections = result_crop_selections(routed);
    let mut crops = Vec::with_capacity(selections.len());
    for (field, filename, crop) in selections {
        let roi = crop.roi;
        let pixels = crop.pixels;
        let header = format!("P6\n{} {}\n255\n", roi.width, roi.height);
        let mut bytes = Vec::with_capacity(header.len() + pixels.len());
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&pixels);
        write_private_file(&output.join(filename), &bytes)?;
        crops.push(ResultCropEvidence {
            field,
            filename: filename.to_owned(),
            roi,
            pixel_sha256: encode_sha256(&pixels),
            file_sha256: encode_sha256(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    let artifact = ResultCropArtifact {
        schema: "scorepeek-private-canonical-result-crops-v2".to_owned(),
        frame_id: frame_id.to_owned(),
        frame_extraction_sha256: snapshot.frame_extraction_sha256,
        canonical_frame_sha256: snapshot.canonical_frame_sha256,
        normalizer_artifact_sha256: snapshot.normalizer_artifact_sha256,
        canonical_layout_sha256: snapshot.canonical_layout_sha256,
        crops,
    };
    let manifest = canonical_evidence_json(&artifact)?;
    write_private_file(&output.join("manifest.json"), &manifest)?;
    Ok(ResultCropExportSummary {
        schema: "scorepeek-result-crop-export-summary-v1".to_owned(),
        output: output.to_path_buf(),
        manifest_sha256: encode_sha256(&manifest),
    })
}

/// Exports the selected title and every visible music-list title slot from a validated frame.
///
/// List slots are geometric observations. Separators and partially visible rows remain in the
/// artifact and must be rejected by downstream recognition rather than silently omitted.
///
/// # Errors
/// Returns an error for a non-music-select screen, invalid layout, or any output I/O failure.
pub fn export_music_select_crops(
    frame: &CanonicalFrame,
    frame_id: &str,
    output: impl AsRef<Path>,
) -> Result<MusicSelectCropExportSummary, RecognitionError> {
    if frame_id.is_empty() || frame_id.len() > 256 || frame_id.chars().any(char::is_control) {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let snapshot = inspect(frame)?;
    if snapshot.screen != ScreenClass::MusicSelect {
        return Err(RecognitionError::NotMusicSelectScreen);
    }
    let output = output.as_ref();
    fs::create_dir(output)?;

    let ScreenRgb8Crops::MusicSelect(routed) = route_screen_rgb8_crops(
        frame.pixels(),
        snapshot
            .crop_route()
            .ok_or(RecognitionError::NotMusicSelectScreen)?,
    )?
    else {
        return Err(RecognitionError::NotMusicSelectScreen);
    };
    let layout = CanonicalLayout::load()?;
    let mut selections = Vec::with_capacity(layout.music_select.list_titles.slots as usize + 1);
    selections.push((
        "selected_title".to_owned(),
        "selected-title.ppm".to_owned(),
        routed.central_title,
    ));
    for (slot, roi) in layout.music_select.list_titles.rois().enumerate() {
        selections.push((
            format!("list_title_{slot:02}"),
            format!("list-title-{slot:02}.ppm"),
            Rgb8Crop {
                roi,
                pixels: crop_canonical_pixels(frame.pixels(), roi)?,
            },
        ));
    }
    let mut crops = Vec::with_capacity(selections.len());
    for (field, filename, crop) in selections {
        let roi = crop.roi;
        let pixels = crop.pixels;
        let header = format!("P6\n{} {}\n255\n", roi.width, roi.height);
        let mut bytes = Vec::with_capacity(header.len() + pixels.len());
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&pixels);
        write_private_file(&output.join(&filename), &bytes)?;
        crops.push(MusicSelectCropEvidence {
            field,
            filename,
            roi,
            pixel_sha256: encode_sha256(&pixels),
            file_sha256: encode_sha256(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    let artifact = MusicSelectCropArtifact {
        schema: "scorepeek-private-canonical-music-select-crops-v1".to_owned(),
        frame_id: frame_id.to_owned(),
        frame_extraction_sha256: snapshot.frame_extraction_sha256,
        canonical_frame_sha256: snapshot.canonical_frame_sha256,
        normalizer_artifact_sha256: snapshot.normalizer_artifact_sha256,
        canonical_layout_sha256: snapshot.canonical_layout_sha256,
        crops,
    };
    let manifest = canonical_evidence_json(&artifact)?;
    write_private_file(&output.join("manifest.json"), &manifest)?;
    Ok(MusicSelectCropExportSummary {
        schema: "scorepeek-music-select-crop-export-summary-v1".to_owned(),
        output: output.to_path_buf(),
        manifest_sha256: encode_sha256(&manifest),
        list_slot_count: layout.music_select.list_titles.slots,
    })
}

/// Exports only the independently measured fields needed by the first integrated-context slice.
///
/// The context layout is versioned separately so adding these observations does not invalidate the
/// existing result-title and music-list diagnostic artifacts. The output directory must not exist;
/// `manifest.json` is written last.
///
/// # Errors
/// Returns an error for an unknown screen, invalid evidence or layout, or output I/O failure.
pub fn export_integrated_context_crops(
    frame: &CanonicalFrame,
    frame_id: &str,
    output: impl AsRef<Path>,
) -> Result<IntegratedContextCropExportSummary, RecognitionError> {
    if frame_id.is_empty() || frame_id.len() > 256 || frame_id.chars().any(char::is_control) {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let snapshot = inspect(frame)?;
    let routed = route_screen_rgb8_crops(
        frame.pixels(),
        snapshot
            .crop_route()
            .ok_or(RecognitionError::InvalidCanonicalFrame)?,
    )?;
    let output = output.as_ref();
    fs::create_dir(output)?;
    let (integrated_context_layout_sha256, selections) = match routed {
        ScreenRgb8Crops::Title(_) => return Err(RecognitionError::InvalidCanonicalFrame),
        ScreenRgb8Crops::Result(crops) => (
            IntegratedContextLayout::sha256(),
            vec![(IntegratedContextField::ResultArtist, crops.artist)],
        ),
        ScreenRgb8Crops::MusicSelect(crops) => (
            crops.integrated_context_layout_sha256,
            vec![
                (IntegratedContextField::MusicSelectArtist, crops.artist),
                (IntegratedContextField::MusicSelectPlayType, crops.play_type),
                (
                    IntegratedContextField::MusicSelectSelectedChart,
                    Rgb8Crop {
                        roi: IntegratedContextLayout::load()?
                            .music_select
                            .legacy_selected_chart,
                        pixels: crop_canonical_pixels(
                            frame.pixels(),
                            IntegratedContextLayout::load()?
                                .music_select
                                .legacy_selected_chart,
                        )?,
                    },
                ),
                (
                    IntegratedContextField::MusicSelectActiveListTitle,
                    crops.active_list_title,
                ),
            ],
        ),
    };
    let mut crops = Vec::with_capacity(selections.len());
    for (field, crop) in selections {
        let filename = integrated_context_filename(field);
        let pixels = crop.pixels;
        let roi = crop.roi;
        let header = format!("P6\n{} {}\n255\n", roi.width, roi.height);
        let mut bytes = Vec::with_capacity(header.len() + pixels.len());
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&pixels);
        write_private_file(&output.join(filename), &bytes)?;
        crops.push(IntegratedContextCropEvidence {
            field,
            filename: filename.to_owned(),
            roi,
            pixel_sha256: encode_sha256(&pixels),
            file_sha256: encode_sha256(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    let screen = snapshot.screen;
    let artifact = IntegratedContextCropArtifact {
        schema: "scorepeek-private-integrated-context-crops-v1".to_owned(),
        frame_id: frame_id.to_owned(),
        frame_extraction_sha256: snapshot.frame_extraction_sha256,
        canonical_frame_sha256: snapshot.canonical_frame_sha256,
        normalizer_artifact_sha256: snapshot.normalizer_artifact_sha256,
        canonical_layout_sha256: snapshot.canonical_layout_sha256,
        integrated_context_layout_sha256,
        screen,
        crops,
    };
    let manifest = canonical_evidence_json(&artifact)?;
    write_private_file(&output.join("manifest.json"), &manifest)?;
    Ok(IntegratedContextCropExportSummary {
        schema: "scorepeek-integrated-context-crop-export-summary-v1".to_owned(),
        output: output.to_path_buf(),
        manifest_sha256: encode_sha256(&manifest),
        screen,
    })
}

fn crop_rgb8(pixels: &[u8], roi: Roi) -> Result<Rgb8Crop, RecognitionError> {
    Ok(Rgb8Crop {
        roi,
        pixels: crop_canonical_pixels(pixels, roi)?,
    })
}

fn route_result_rgb8_crops(
    pixels: &[u8],
    canonical: &CanonicalLayout,
    context: &IntegratedContextLayout,
    panel_side: ResultPanelSide,
) -> Result<ResultScreenRgb8Crops, RecognitionError> {
    let origin_x = canonical.result.panel_origins.get(panel_side);
    let panel = |roi: Roi| roi.translated_x(origin_x);
    let numeric = |field, roi: Roi| {
        roi.translated_x(
            canonical
                .result
                .numeric_panel_origins
                .get(panel_side, field),
        )
    };
    Ok(ResultScreenRgb8Crops {
        canonical_layout_sha256: CanonicalLayout::sha256(),
        panel_side,
        title: crop_rgb8(pixels, canonical.result.title)?,
        artist: crop_rgb8(pixels, context.result.artist)?,
        clear_type: crop_rgb8(pixels, panel(canonical.result.clear_type)?)?,
        difficulty: crop_rgb8(pixels, canonical.result.difficulty)?,
        play_type: crop_rgb8(pixels, context.result.play_type)?,
        level: crop_rgb8(pixels, canonical.result.level)?,
        notes: crop_rgb8(pixels, canonical.result.notes)?,
        current_score: crop_rgb8(
            pixels,
            numeric(NumericField::CurrentScore, canonical.result.current_score)?,
        )?,
        previous_clear_type: crop_rgb8(pixels, panel(canonical.result.previous_clear_type)?)?,
        previous_score: crop_rgb8(
            pixels,
            numeric(NumericField::PreviousScore, canonical.result.previous_score)?,
        )?,
        previous_miss_count: crop_rgb8(
            pixels,
            numeric(
                NumericField::PreviousMissCount,
                canonical.result.previous_miss_count,
            )?,
        )?,
        miss_count: crop_rgb8(
            pixels,
            numeric(NumericField::MissCount, canonical.result.miss_count)?,
        )?,
        pgreat: crop_rgb8(
            pixels,
            numeric(NumericField::Pgreat, canonical.result.pgreat)?,
        )?,
        great: crop_rgb8(
            pixels,
            numeric(NumericField::Great, canonical.result.great)?,
        )?,
        good: crop_rgb8(pixels, numeric(NumericField::Good, canonical.result.good)?)?,
        bad: crop_rgb8(pixels, numeric(NumericField::Bad, canonical.result.bad)?)?,
        poor: crop_rgb8(pixels, numeric(NumericField::Poor, canonical.result.poor)?)?,
        fast: crop_rgb8(pixels, numeric(NumericField::Fast, canonical.result.fast)?)?,
        slow: crop_rgb8(pixels, numeric(NumericField::Slow, canonical.result.slow)?)?,
        combo_break: crop_rgb8(
            pixels,
            numeric(NumericField::ComboBreak, canonical.result.combo_break)?,
        )?,
        play_options: crop_rgb8(pixels, panel(canonical.result.play_options)?)?,
    })
}

/// Routes one already-classified canonical RGB8 frame to all currently measured field crops for
/// that screen.
///
/// This function is synchronous, deterministic, and filesystem-free. Callers retain responsibility
/// for binding the result to capture provenance and for preventing `Unknown` from entering field
/// observation.
///
/// # Errors
/// Returns an error for an unknown screen, invalid canonical pixels, or layout drift.
pub fn route_screen_rgb8_crops(
    pixels: &[u8],
    route: ScreenCropRoute,
) -> Result<ScreenRgb8Crops, RecognitionError> {
    let canonical = CanonicalLayout::load()?;
    let context = IntegratedContextLayout::load()?;
    match route {
        ScreenCropRoute::Title => {
            let path = ScreenPathLayout::load()?;
            Ok(ScreenRgb8Crops::Title(TitleScreenRgb8Crops {
                canonical_layout_sha256: CanonicalLayout::sha256(),
                game_version: crop_rgb8(pixels, path.title.version)?,
            }))
        }
        ScreenCropRoute::Result(panel_side) => Ok(ScreenRgb8Crops::Result(
            route_result_rgb8_crops(pixels, &canonical, &context, panel_side)?,
        )),
        ScreenCropRoute::MusicSelect => {
            Ok(ScreenRgb8Crops::MusicSelect(MusicSelectScreenRgb8Crops {
                best: MusicSelectBestCrops::extract(pixels)?,
                canonical_layout_sha256: CanonicalLayout::sha256(),
                integrated_context_layout_sha256: IntegratedContextLayout::sha256(),
                central_title: crop_rgb8(pixels, canonical.music_select.selected_title)?,
                artist: crop_rgb8(pixels, context.music_select.artist)?,
                play_type: crop_rgb8(pixels, context.music_select.play_type.roi)?,
                difficulty_markers: MusicSelectDifficultyMarkerCrops {
                    beginner: crop_rgb8(pixels, context.music_select.selected_difficulty.beginner)?,
                    normal: crop_rgb8(pixels, context.music_select.selected_difficulty.normal)?,
                    hyper: crop_rgb8(pixels, context.music_select.selected_difficulty.hyper)?,
                    another: crop_rgb8(pixels, context.music_select.selected_difficulty.another)?,
                    leggendaria: crop_rgb8(
                        pixels,
                        context.music_select.selected_difficulty.leggendaria,
                    )?,
                },
                play_side: MusicSelectPlaySideCrops {
                    one_player: crop_rgb8(pixels, context.music_select.play_side.one_player)?,
                    two_player: crop_rgb8(pixels, context.music_select.play_side.two_player)?,
                },
                active_list_title: crop_rgb8(pixels, context.music_select.active_list_title)?,
            }))
        }
    }
}

#[must_use]
/// Evaluates the five fixed `PLAYER 01` marker slots without invoking OCR.
///
/// # Panics
///
/// Panics only if the embedded layout, which is validated by the build's layout contract tests,
/// can no longer be decoded.
const fn integrated_context_filename(field: IntegratedContextField) -> &'static str {
    match field {
        IntegratedContextField::ResultArtist => "result-artist.ppm",
        IntegratedContextField::MusicSelectArtist => "music-select-artist.ppm",
        IntegratedContextField::MusicSelectSelectedChart => "music-select-selected-chart.ppm",
        IntegratedContextField::MusicSelectPlayType => "music-select-play-type.ppm",
        IntegratedContextField::MusicSelectActiveListTitle => "music-select-active-list-title.ppm",
    }
}

/// Runs the selected native dynamic recognizer over text-only integrated-context crops.
///
/// The combined selected-chart crop is deliberately excluded from OCR. Its digest-bound evidence
/// is recorded as unknown until a dedicated chart observer is implemented. The output directory
/// must not exist; `manifest.json` is written last, so its presence denotes a complete run.
///
/// # Errors
/// Returns an error for an unregistered model choice, invalid crop evidence, incomplete model
/// bundle, unexpected ONNX output, or output I/O failure.
#[allow(
    clippy::too_many_lines,
    reason = "the strict integrated-context artifact reader keeps all field bindings together"
)]
pub fn observe_integrated_context(
    crop_directory: impl AsRef<Path>,
    expected_manifest_sha256: &str,
    model_id: &str,
    bundle_path: impl AsRef<Path>,
    output: impl AsRef<Path>,
) -> Result<IntegratedContextObservationSummary, RecognitionError> {
    if model_id != INTEGRATED_CONTEXT_MODEL_ID {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let crop_directory = crop_directory.as_ref();
    let artifact = read_integrated_context_crop_artifact(crop_directory, expected_manifest_sha256)?;
    let text_crops: Vec<_> = artifact
        .crops
        .iter()
        .filter(|crop| {
            !matches!(
                crop.field,
                IntegratedContextField::MusicSelectSelectedChart
                    | IntegratedContextField::MusicSelectPlayType
            )
        })
        .collect();
    // Own the joined paths before serializing references into the decoder request.
    let crop_paths: Vec<_> = text_crops
        .iter()
        .map(|crop| crop_directory.join(&crop.filename))
        .collect();
    let request = IntegratedContextDecodeRequest {
        schema: "scorepeek-private-official-onnx-decode-request-v1",
        rows: text_crops
            .iter()
            .zip(&crop_paths)
            .map(|(crop, path)| IntegratedContextDecodeRequestRow {
                path,
                file_sha256: &crop.file_sha256,
            })
            .collect(),
    };
    let output = output.as_ref();
    if !crop_directory.is_absolute() || !bundle_path.as_ref().is_absolute() || !output.is_absolute()
    {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    fs::create_dir(output)?;
    let request_bytes = canonical_evidence_json(&request)?;
    let request_path = output.join("decode-request.json");
    write_private_file(&request_path, &request_bytes)?;
    let decoded =
        decode_dynamic_official_onnx_crops(model_id, bundle_path.as_ref(), &request_path)?;
    let row_count = text_crops.len();
    if decoded.input_widths.len() != row_count
        || decoded.input_tensor_sha256s.len() != row_count
        || decoded.output_timesteps.len() != row_count
        || decoded.decoded_text.len() != row_count
    {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let text_observations = text_crops
        .iter()
        .enumerate()
        .map(|(index, crop)| IntegratedContextTextObservation {
            field: crop.field,
            crop_file_sha256: crop.file_sha256.clone(),
            input_width: decoded.input_widths[index],
            input_tensor_sha256: decoded.input_tensor_sha256s[index].clone(),
            output_timesteps: decoded.output_timesteps[index],
            open_text: decoded.decoded_text[index].clone(),
        })
        .collect();
    let chart_context = artifact
        .crops
        .iter()
        .find(|crop| crop.field == IntegratedContextField::MusicSelectSelectedChart)
        .map(|crop| IntegratedChartContextEvidence {
            field: crop.field,
            crop_file_sha256: crop.file_sha256.clone(),
            pixel_sha256: crop.pixel_sha256.clone(),
            state: IntegratedChartContextState::Unknown,
            reason: IntegratedChartContextUnknownReason::ObserverNotImplemented,
        });
    let chart_context_state = chart_context.as_ref().map(|evidence| evidence.state);
    let observation = IntegratedContextObservationArtifact {
        schema: "scorepeek-private-integrated-context-observation-v1",
        recording_completeness: IntegratedContextRecordingCompleteness::Complete,
        source_manifest_sha256: expected_manifest_sha256.to_owned(),
        frame_id: artifact.frame_id,
        frame_extraction_sha256: artifact.frame_extraction_sha256,
        canonical_frame_sha256: artifact.canonical_frame_sha256,
        normalizer_artifact_sha256: artifact.normalizer_artifact_sha256,
        canonical_layout_sha256: artifact.canonical_layout_sha256,
        integrated_context_layout_sha256: artifact.integrated_context_layout_sha256,
        screen: artifact.screen,
        model_id: decoded.model_id,
        model_sha256: decoded.model_sha256,
        dictionary_sha256: decoded.dictionary_sha256,
        preprocessor_id: decoded.preprocessor_id,
        request_sha256: decoded.request_sha256,
        elapsed_ms: decoded.elapsed_ms,
        text_observations,
        chart_context,
    };
    let manifest = canonical_evidence_json(&observation)?;
    publish_private_manifest(output, &manifest)?;
    Ok(IntegratedContextObservationSummary {
        schema: "scorepeek-integrated-context-observation-summary-v1",
        output: output.to_path_buf(),
        manifest_sha256: encode_sha256(&manifest),
        screen: observation.screen,
        text_observation_count: observation.text_observations.len(),
        chart_context_state,
    })
}

pub(super) fn read_integrated_context_crop_artifact(
    directory: &Path,
    expected_manifest_sha256: &str,
) -> Result<IntegratedContextCropArtifact, RecognitionError> {
    if !directory.is_absolute()
        || !directory.metadata()?.is_dir()
        || !valid_sha256(expected_manifest_sha256)
    {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let manifest_bytes = read_bounded_regular(
        &directory.join("manifest.json"),
        MAX_EXTRACTION_MANIFEST_BYTES,
        None,
    )?;
    if encode_sha256(&manifest_bytes) != expected_manifest_sha256 {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let artifact: IntegratedContextCropArtifact = serde_json::from_slice(&manifest_bytes)?;
    if canonical_evidence_json(&artifact)? != manifest_bytes
        || artifact.schema != "scorepeek-private-integrated-context-crops-v1"
        || artifact.frame_id.is_empty()
        || !valid_sha256(&artifact.frame_extraction_sha256)
        || !valid_sha256(&artifact.canonical_frame_sha256)
        || !valid_sha256(&artifact.normalizer_artifact_sha256)
        || artifact.canonical_layout_sha256 != CanonicalLayout::sha256()
        || artifact.integrated_context_layout_sha256 != IntegratedContextLayout::sha256()
    {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let layout = IntegratedContextLayout::load()?;
    let expected: Vec<_> = match artifact.screen {
        ScreenClass::Result => vec![(
            IntegratedContextField::ResultArtist,
            "result-artist.ppm",
            layout.result.artist,
        )],
        ScreenClass::MusicSelect => vec![
            (
                IntegratedContextField::MusicSelectArtist,
                "music-select-artist.ppm",
                layout.music_select.artist,
            ),
            (
                IntegratedContextField::MusicSelectSelectedChart,
                "music-select-selected-chart.ppm",
                layout.music_select.legacy_selected_chart,
            ),
            (
                IntegratedContextField::MusicSelectPlayType,
                "music-select-play-type.ppm",
                layout.music_select.play_type.roi,
            ),
            (
                IntegratedContextField::MusicSelectActiveListTitle,
                "music-select-active-list-title.ppm",
                layout.music_select.active_list_title,
            ),
        ],
        ScreenClass::Title
        | ScreenClass::ModeSelect
        | ScreenClass::DecideTransition
        | ScreenClass::Play
        | ScreenClass::Unknown => {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
    };
    if artifact.crops.len() != expected.len() {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    for (crop, (field, filename, roi)) in artifact.crops.iter().zip(expected) {
        let expected_bytes = u64::from(roi.width) * u64::from(roi.height) * 3
            + format!("P6\n{} {}\n255\n", roi.width, roi.height).len() as u64;
        if crop.field != field
            || crop.filename != filename
            || crop.roi != roi
            || crop.bytes != expected_bytes
            || !valid_sha256(&crop.file_sha256)
            || !valid_sha256(&crop.pixel_sha256)
        {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        let bytes = read_bounded_regular(
            &directory.join(filename),
            expected_bytes,
            Some(expected_bytes),
        )?;
        if encode_sha256(&bytes) != crop.file_sha256 {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        let header = format!("P6\n{} {}\n255\n", roi.width, roi.height);
        let pixels = bytes
            .strip_prefix(header.as_bytes())
            .ok_or(RecognitionError::InvalidCanonicalFrame)?;
        if encode_sha256(pixels) != crop.pixel_sha256 {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
    }
    Ok(artifact)
}

#[allow(
    clippy::too_many_lines,
    reason = "strict admission validates every ordered result crop in one versioned contract"
)]
pub(in crate::recognition) fn read_title_crop_artifact(
    directory: &Path,
    expected_manifest_sha256: &str,
) -> Result<(Roi, Vec<u8>), RecognitionError> {
    if !directory.is_absolute()
        || !directory.metadata()?.is_dir()
        || !valid_sha256(expected_manifest_sha256)
    {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let manifest_bytes = read_bounded_regular(
        &directory.join("manifest.json"),
        MAX_EXTRACTION_MANIFEST_BYTES,
        None,
    )?;
    if encode_sha256(&manifest_bytes) != expected_manifest_sha256 {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let artifact: ResultCropArtifact = serde_json::from_slice(&manifest_bytes)?;
    if canonical_evidence_json(&artifact)? != manifest_bytes
        || artifact.schema != "scorepeek-private-canonical-result-crops-v2"
        || artifact.frame_id.is_empty()
        || !valid_sha256(&artifact.frame_extraction_sha256)
        || !valid_sha256(&artifact.canonical_frame_sha256)
        || !valid_sha256(&artifact.normalizer_artifact_sha256)
        || artifact.canonical_layout_sha256 != CanonicalLayout::sha256()
    {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let layout = CanonicalLayout::load()?;
    let expected = [
        (ResultCropField::Title, "title.ppm", layout.result.title),
        (ResultCropField::Artist, "artist.ppm", layout.result.artist),
        (
            ResultCropField::ClearType,
            "clear-type.ppm",
            layout.result.clear_type,
        ),
        (
            ResultCropField::Difficulty,
            "difficulty.ppm",
            layout.result.difficulty,
        ),
        (ResultCropField::Level, "level.ppm", layout.result.level),
        (ResultCropField::Notes, "notes.ppm", layout.result.notes),
        (
            ResultCropField::CurrentScore,
            "current-score.ppm",
            layout.result.current_score,
        ),
        (
            ResultCropField::PreviousClearType,
            "previous-clear-type.ppm",
            layout.result.previous_clear_type,
        ),
        (
            ResultCropField::PreviousScore,
            "previous-score.ppm",
            layout.result.previous_score,
        ),
        (
            ResultCropField::PreviousMissCount,
            "previous-miss-count.ppm",
            layout.result.previous_miss_count,
        ),
        (
            ResultCropField::MissCount,
            "miss-count.ppm",
            layout.result.miss_count,
        ),
        (ResultCropField::Pgreat, "pgreat.ppm", layout.result.pgreat),
        (ResultCropField::Great, "great.ppm", layout.result.great),
        (ResultCropField::Good, "good.ppm", layout.result.good),
        (ResultCropField::Bad, "bad.ppm", layout.result.bad),
        (ResultCropField::Poor, "poor.ppm", layout.result.poor),
        (ResultCropField::Fast, "fast.ppm", layout.result.fast),
        (ResultCropField::Slow, "slow.ppm", layout.result.slow),
        (
            ResultCropField::ComboBreak,
            "combo-break.ppm",
            layout.result.combo_break,
        ),
        (
            ResultCropField::PlayOptions,
            "play-options.ppm",
            layout.result.play_options,
        ),
    ];
    if artifact.crops.len() != expected.len() {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let mut title = None;
    for (crop, (field, filename, roi)) in artifact.crops.iter().zip(expected) {
        let header = format!("P6\n{} {}\n255\n", roi.width, roi.height);
        let expected_bytes = header.len() as u64 + u64::from(roi.width) * u64::from(roi.height) * 3;
        if crop.field != field
            || crop.filename != filename
            || crop.roi != roi
            || crop.bytes != expected_bytes
            || !valid_sha256(&crop.pixel_sha256)
            || !valid_sha256(&crop.file_sha256)
        {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        let bytes = read_bounded_regular(
            &directory.join(filename),
            expected_bytes,
            Some(expected_bytes),
        )?;
        if encode_sha256(&bytes) != crop.file_sha256 {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        let pixels = bytes
            .strip_prefix(header.as_bytes())
            .ok_or(RecognitionError::InvalidCanonicalFrame)?;
        if encode_sha256(pixels) != crop.pixel_sha256 {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        if field == ResultCropField::Title {
            title = Some((roi, pixels.to_vec()));
        }
    }
    title.ok_or(RecognitionError::InvalidCanonicalFrame)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), RecognitionError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(super) fn publish_private_manifest(
    directory: &Path,
    bytes: &[u8],
) -> Result<(), RecognitionError> {
    let manifest = directory.join("manifest.json");
    let staging = directory.join(".manifest.json.scorepeek-staging");
    if let Err(error) = write_private_file(&staging, bytes) {
        let _ = fs::remove_file(&staging);
        return Err(error);
    }
    if let Err(error) = fs::hard_link(&staging, &manifest) {
        let _ = fs::remove_file(&staging);
        return Err(error.into());
    }
    fs::remove_file(&staging)?;
    File::open(directory)?.sync_all()?;
    File::open(
        directory
            .parent()
            .ok_or(RecognitionError::InvalidCanonicalFrame)?,
    )?
    .sync_all()?;
    Ok(())
}
