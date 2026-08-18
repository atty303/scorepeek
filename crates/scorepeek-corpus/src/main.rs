use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use scorepeek_corpus::{
    CorpusStore, apply_music_list_motion_review, inspect_music_list_row_observation_draft,
    measure_music_list_motion, plan_music_list_motion_review, render_synthetic_title_set,
    verify_music_list_motion, verify_music_list_row_observation_draft,
};

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[OsString]) -> Result<(), String> {
    if let [music_list, motion, measure, request, output, document] = args
        && music_list == "music-list"
        && motion == "motion"
        && measure == "measure"
        && request == "--output"
    {
        let summary = measure_music_list_motion(PathBuf::from(document), PathBuf::from(output))
            .map_err(|error| format!("music-list motion measurement failed: {error}"))?;
        return print_json(&summary, "music-list motion measurement");
    }
    if let [music_list, motion, verify, document] = args
        && music_list == "music-list"
        && motion == "motion"
        && verify == "verify"
    {
        let summary = verify_music_list_motion(PathBuf::from(document))
            .map_err(|error| format!("music-list motion verification failed: {error}"))?;
        return print_json(&summary, "music-list motion verification");
    }
    if let [music_list, motion, review_plan, output, plan, artifact] = args
        && music_list == "music-list"
        && motion == "motion"
        && review_plan == "review-plan"
        && output == "--output"
    {
        let summary =
            plan_music_list_motion_review(PathBuf::from(artifact), PathBuf::from(plan))
                .map_err(|error| format!("music-list motion review planning failed: {error}"))?;
        return print_json(&summary, "music-list motion review planning");
    }
    if let [
        music_list,
        motion,
        review_apply,
        output,
        request,
        artifact,
        plan,
        decisions,
    ] = args
        && music_list == "music-list"
        && motion == "motion"
        && review_apply == "review-apply"
        && output == "--output"
    {
        let summary = apply_music_list_motion_review(
            PathBuf::from(artifact),
            PathBuf::from(plan),
            PathBuf::from(decisions),
            PathBuf::from(request),
        )
        .map_err(|error| format!("music-list motion review application failed: {error}"))?;
        return print_json(&summary, "music-list motion review application");
    }
    if let [music_list, observation, inspect, document] = args
        && music_list == "music-list"
        && observation == "observation-draft"
        && inspect == "inspect"
    {
        let summary =
            inspect_music_list_row_observation_draft(PathBuf::from(document)).map_err(|error| {
                format!("music-list row observation draft inspection failed: {error}")
            })?;
        return print_json(&summary, "music-list row observation draft inspection");
    }
    if let [music_list, observation, verify, document] = args
        && music_list == "music-list"
        && observation == "observation-draft"
        && verify == "verify"
    {
        let summary =
            verify_music_list_row_observation_draft(PathBuf::from(document)).map_err(|error| {
                format!("music-list row observation draft verification failed: {error}")
            })?;
        return print_json(&summary, "music-list row observation draft verification");
    }
    if let Some(result) = run_dataset(args) {
        return result;
    }
    if let Some(result) = run_media(args) {
        return result;
    }
    run_legacy(args)
}

fn run_legacy(args: &[OsString]) -> Result<(), String> {
    match args {
        [command, store, root, source, request] if command == "ingest" && store == "--store" => {
            let manifest = CorpusStore::new(PathBuf::from(root))
                .ingest(PathBuf::from(source), PathBuf::from(request))
                .map_err(|error| format!("scorepeek-corpus ingest failed: {error}"))?;
            let summary = manifest
                .summary()
                .map_err(|error| format!("ingest result validation failed: {error}"))?;
            println!(
                "{}",
                serde_json::to_string(&summary)
                    .map_err(|error| format!("ingest result encoding failed: {error}"))?
            );
            Ok(())
        }
        [replay, validate, store, root, suite]
            if replay == "replay" && validate == "validate" && store == "--store" =>
        {
            let summary = CorpusStore::new(PathBuf::from(root))
                .validate_replay_suite(PathBuf::from(suite))
                .map_err(|error| format!("invalid replay suite: {error}"))?;
            println!(
                "{}",
                serde_json::to_string(&summary)
                    .map_err(|error| format!("replay summary encoding failed: {error}"))?
            );
            Ok(())
        }
        [label, author, store, root, document]
            if label == "label" && author == "author" && store == "--store" =>
        {
            let summary = CorpusStore::new(PathBuf::from(root))
                .author_complete_label(PathBuf::from(document))
                .map_err(|error| format!("scorepeek-corpus label author failed: {error}"))?;
            println!(
                "{}",
                serde_json::to_string(&summary)
                    .map_err(|error| format!("complete-label summary encoding failed: {error}"))?
            );
            Ok(())
        }
        [generation, seal, store, root, generation_id]
            if generation == "generation" && seal == "seal" && store == "--store" =>
        {
            let summary = CorpusStore::new(PathBuf::from(root))
                .seal_generation(&generation_id.to_string_lossy())
                .map_err(|error| format!("scorepeek-corpus generation seal failed: {error}"))?;
            println!(
                "{}",
                serde_json::to_string(&summary)
                    .map_err(|error| format!("generation summary encoding failed: {error}"))?
            );
            Ok(())
        }
        [index, generate, store, root, plan]
            if index == "index" && generate == "generate" && store == "--store" =>
        {
            let summary = CorpusStore::new(PathBuf::from(root))
                .generate_replay_index(PathBuf::from(plan))
                .map_err(|error| format!("scorepeek-corpus index generate failed: {error}"))?;
            println!(
                "{}",
                serde_json::to_string(&summary)
                    .map_err(|error| format!("replay-index summary encoding failed: {error}"))?
            );
            Ok(())
        }
        [synthetic, render, output, directory, request]
            if synthetic == "synthetic" && render == "render" && output == "--output" =>
        {
            let summary = render_synthetic_title_set(
                PathBuf::from(request),
                PathBuf::from(directory),
            )
            .map_err(|error| format!("scorepeek-corpus synthetic render failed: {error}"))?;
            println!(
                "{}",
                serde_json::to_string(&summary)
                    .map_err(|error| format!("synthetic summary encoding failed: {error}"))?
            );
            Ok(())
        }
        [flag] if flag == "--help" || flag == "-h" => {
            print_usage();
            Ok(())
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("scorepeek-corpus {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => Err(
            "usage: scorepeek-corpus <recording import --store ROOT --capture-context CONTEXT [--external] RECORDING|dataset seal|push|pull|verify|remote-verify ...|ingest --store ROOT SOURCE REQUEST|generation seal --store ROOT GENERATION_ID|label author --store ROOT DOCUMENT|index generate --store ROOT PLAN|media probe --store ROOT --output MANIFEST FIXTURE_ID|media extract --store ROOT --output DIRECTORY PROBE_MANIFEST REQUEST|canonical extract --store ROOT --output DIRECTORY PROBE_MANIFEST REQUEST|synthetic render --output DIRECTORY REQUEST|music-list observation-draft inspect|verify DOCUMENT|music-list motion measure --output ARTIFACT REQUEST|music-list motion verify ARTIFACT|music-list motion review-plan --output PLAN ARTIFACT|music-list motion review-apply --output REQUEST ARTIFACT PLAN DECISIONS|replay validate --store ROOT SUITE>"
                .to_owned(),
        ),
    }
}

fn run_dataset(args: &[OsString]) -> Option<Result<(), String>> {
    let result = match args {
        [
            recording,
            import,
            store,
            root,
            capture_context,
            context,
            source,
        ] if recording == "recording"
            && import == "import"
            && store == "--store"
            && capture_context == "--capture-context" =>
        {
            CorpusStore::new(PathBuf::from(root))
                .import_recording(PathBuf::from(source), PathBuf::from(context))
                .map_err(|error| format!("scorepeek-corpus recording import failed: {error}"))
                .and_then(|summary| print_json(&summary, "recording import summary"))
        }
        [
            recording,
            import,
            store,
            root,
            capture_context,
            context,
            external,
            source,
        ] if recording == "recording"
            && import == "import"
            && store == "--store"
            && capture_context == "--capture-context"
            && external == "--external" =>
        {
            CorpusStore::new(PathBuf::from(root))
                .import_external_recording(PathBuf::from(source), PathBuf::from(context))
                .map_err(|error| format!("scorepeek-corpus recording import failed: {error}"))
                .and_then(|summary| print_json(&summary, "recording import summary"))
        }
        [dataset, seal, store, root, dataset_id]
            if dataset == "dataset" && seal == "seal" && store == "--store" =>
        {
            CorpusStore::new(PathBuf::from(root))
                .seal_recording_dataset(&dataset_id.to_string_lossy())
                .map_err(|error| format!("scorepeek-corpus dataset seal failed: {error}"))
                .and_then(|summary| print_json(&summary, "dataset summary"))
        }
        [dataset, verify, store, root, generation]
            if dataset == "dataset" && verify == "verify" && store == "--store" =>
        {
            CorpusStore::new(PathBuf::from(root))
                .verify_recording_dataset(&generation.to_string_lossy())
                .map_err(|error| format!("scorepeek-corpus dataset verify failed: {error}"))
                .and_then(|summary| print_json(&summary, "dataset summary"))
        }
        [
            dataset,
            operation,
            store,
            root,
            remote,
            remote_config,
            generation,
        ] if dataset == "dataset"
            && store == "--store"
            && remote == "--remote"
            && matches!(
                operation.to_string_lossy().as_ref(),
                "push" | "pull" | "remote-verify"
            ) =>
        {
            let corpus = CorpusStore::new(PathBuf::from(root));
            let result = match operation.to_string_lossy().as_ref() {
                "push" => corpus.push_recording_dataset(
                    PathBuf::from(remote_config),
                    &generation.to_string_lossy(),
                ),
                "pull" => corpus.pull_recording_dataset(
                    PathBuf::from(remote_config),
                    &generation.to_string_lossy(),
                ),
                "remote-verify" => corpus.verify_remote_recording_dataset(
                    PathBuf::from(remote_config),
                    &generation.to_string_lossy(),
                ),
                _ => unreachable!(),
            };
            result
                .map_err(|error| format!("scorepeek-corpus dataset operation failed: {error}"))
                .and_then(|summary| print_json(&summary, "remote dataset summary"))
        }
        _ => return None,
    };
    Some(result)
}

fn run_media(args: &[OsString]) -> Option<Result<(), String>> {
    let result = match args {
        [media, probe, store, root, output, manifest, fixture_id]
            if media == "media"
                && probe == "probe"
                && store == "--store"
                && output == "--output" =>
        {
            CorpusStore::new(PathBuf::from(root))
                .probe_media(&fixture_id.to_string_lossy(), PathBuf::from(manifest))
                .map_err(|error| format!("scorepeek-corpus media probe failed: {error}"))
                .and_then(|summary| print_json(&summary, "media probe summary"))
        }
        [
            media,
            extract,
            store,
            root,
            output,
            directory,
            probe_manifest,
            request,
        ] if media == "media"
            && extract == "extract"
            && store == "--store"
            && output == "--output" =>
        {
            CorpusStore::new(PathBuf::from(root))
                .extract_frames(
                    PathBuf::from(probe_manifest),
                    PathBuf::from(request),
                    PathBuf::from(directory),
                )
                .map_err(|error| format!("scorepeek-corpus media extract failed: {error}"))
                .and_then(|summary| print_json(&summary, "frame extraction summary"))
        }
        [
            canonical,
            extract,
            store,
            root,
            output,
            directory,
            probe_manifest,
            request,
        ] if canonical == "canonical"
            && extract == "extract"
            && store == "--store"
            && output == "--output" =>
        {
            CorpusStore::new(PathBuf::from(root))
                .extract_canonical_frames(
                    PathBuf::from(probe_manifest),
                    PathBuf::from(request),
                    PathBuf::from(directory),
                )
                .map_err(|error| format!("scorepeek-corpus canonical extract failed: {error}"))
                .and_then(|summary| print_json(&summary, "canonical frame extraction summary"))
        }
        _ => return None,
    };
    Some(result)
}

fn print_json(value: &impl serde::Serialize, context: &str) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(value)
            .map_err(|error| format!("{context} encoding failed: {error}"))?
    );
    Ok(())
}

fn print_usage() {
    println!(
        "scorepeek-corpus {}\n\nUsage:\n  scorepeek-corpus recording import --store ROOT --capture-context CONTEXT [--external] RECORDING\n  scorepeek-corpus dataset seal --store ROOT DATASET_ID\n  scorepeek-corpus dataset push --store ROOT --remote REMOTE GENERATION_SHA256\n  scorepeek-corpus dataset pull --store ROOT --remote REMOTE GENERATION_SHA256\n  scorepeek-corpus dataset verify --store ROOT GENERATION_SHA256\n  scorepeek-corpus dataset remote-verify --store ROOT --remote REMOTE GENERATION_SHA256\n  scorepeek-corpus ingest --store ROOT SOURCE REQUEST\n  scorepeek-corpus generation seal --store ROOT GENERATION_ID\n  scorepeek-corpus label author --store ROOT DOCUMENT\n  scorepeek-corpus index generate --store ROOT PLAN\n  scorepeek-corpus media probe --store ROOT --output MANIFEST FIXTURE_ID\n  scorepeek-corpus media extract --store ROOT --output DIRECTORY PROBE_MANIFEST REQUEST\n  scorepeek-corpus canonical extract --store ROOT --output DIRECTORY PROBE_MANIFEST REQUEST\n  scorepeek-corpus synthetic render --output DIRECTORY REQUEST\n  scorepeek-corpus music-list observation-draft inspect DOCUMENT\n  scorepeek-corpus music-list observation-draft verify DOCUMENT\n  scorepeek-corpus music-list motion measure --output ARTIFACT REQUEST\n  scorepeek-corpus music-list motion verify ARTIFACT\n  scorepeek-corpus music-list motion review-plan --output PLAN ARTIFACT\n  scorepeek-corpus music-list motion review-apply --output REQUEST ARTIFACT PLAN DECISIONS\n  scorepeek-corpus replay validate --store ROOT SUITE",
        env!("CARGO_PKG_VERSION")
    );
}
