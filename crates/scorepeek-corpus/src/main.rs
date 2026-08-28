use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use scorepeek_corpus::{
    apply_music_list_motion_review, apply_review, convert_v2_diagnostic, import_diagnostic,
    inspect_music_list_row_observation_draft, inspect_review, measure_music_list_motion,
    plan_music_list_motion_review, render_synthetic_title_set, replay_corpus, replay_video,
    verify_diagnostic, verify_music_list_motion, verify_music_list_row_observation_draft,
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
    if let Some(result) = run_frame_corpus(args) {
        return result;
    }
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
    run_remaining(args)
}

#[allow(clippy::too_many_lines)]
fn run_frame_corpus(args: &[OsString]) -> Option<Result<(), String>> {
    let result = match args {
        [
            diagnostic,
            replay,
            video_flag,
            video,
            profile_flag,
            profile,
            output_flag,
            output,
        ] if diagnostic == "diagnostic"
            && replay == "replay-video"
            && video_flag == "--video"
            && profile_flag == "--profile"
            && output_flag == "--output" =>
        {
            replay_video(
                &PathBuf::from(video),
                &profile.to_string_lossy(),
                &PathBuf::from(output),
            )
            .map_err(|error| format!("video diagnostic replay failed: {error}"))
            .and_then(|summary| print_json(&summary, "video diagnostic replay"))
        }
        [diagnostic, verify, directory] if diagnostic == "diagnostic" && verify == "verify" => {
            verify_diagnostic(&PathBuf::from(directory))
                .map_err(|error| format!("diagnostic verification failed: {error}"))
                .and_then(|summary| print_json(&summary, "diagnostic verification"))
        }
        [
            diagnostic,
            convert,
            diagnostic_flag,
            diagnostic_directory,
            recognition_flag,
            recognition_directory,
            output_flag,
            output,
        ] if diagnostic == "diagnostic"
            && convert == "convert-v2"
            && diagnostic_flag == "--diagnostic"
            && recognition_flag == "--recognition"
            && output_flag == "--output" =>
        {
            convert_v2_diagnostic(
                &PathBuf::from(diagnostic_directory),
                &PathBuf::from(recognition_directory),
                &PathBuf::from(output),
            )
            .map_err(|error| format!("diagnostic conversion failed: {error}"))
            .and_then(|summary| print_json(&summary, "diagnostic conversion"))
        }
        [
            corpus,
            import,
            store_flag,
            store,
            diagnostic_flag,
            diagnostic,
            draft_flag,
            draft,
        ] if corpus == "corpus"
            && import == "import-diagnostic"
            && store_flag == "--store"
            && diagnostic_flag == "--diagnostic"
            && draft_flag == "--review-draft" =>
        {
            import_diagnostic(
                &PathBuf::from(store),
                &PathBuf::from(diagnostic),
                &PathBuf::from(draft),
            )
            .map_err(|error| format!("diagnostic import failed: {error}"))
            .and_then(|summary| print_json(&summary, "diagnostic import"))
        }
        [review, show, draft_flag, draft]
            if review == "review" && show == "show" && draft_flag == "--draft" =>
        {
            inspect_review(&PathBuf::from(draft))
                .map_err(|error| format!("review inspection failed: {error}"))
                .and_then(|summary| print_json(&summary, "review inspection"))
        }
        [
            review,
            apply,
            store_flag,
            store,
            draft_flag,
            draft,
            labels_flag,
            labels,
        ] if review == "review"
            && apply == "apply"
            && store_flag == "--store"
            && draft_flag == "--draft"
            && labels_flag == "--labels" =>
        {
            apply_review(
                &PathBuf::from(store),
                &PathBuf::from(draft),
                &PathBuf::from(labels),
            )
            .map_err(|error| format!("review application failed: {error}"))
            .and_then(|summary| print_json(&summary, "review application"))
        }
        [corpus, replay, store_flag, store]
            if corpus == "corpus" && replay == "replay" && store_flag == "--store" =>
        {
            replay_corpus(&PathBuf::from(store))
                .map_err(|error| format!("corpus replay failed: {error}"))
                .and_then(|summary| print_json(&summary, "corpus replay"))
        }
        _ => return None,
    };
    Some(result)
}

fn run_remaining(args: &[OsString]) -> Result<(), String> {
    match args {
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
            "usage: scorepeek-corpus <diagnostic replay-video --video FILE --profile NAME --output DIRECTORY|diagnostic verify DIRECTORY|diagnostic convert-v2 --diagnostic DIRECTORY --recognition DIRECTORY --output DIRECTORY|corpus import-diagnostic --store ROOT --diagnostic DIRECTORY --review-draft FILE|review show --draft FILE|review apply --store ROOT --draft FILE --labels FILE|corpus replay --store ROOT|synthetic render --output DIRECTORY REQUEST|music-list observation-draft inspect|verify DOCUMENT|music-list motion measure --output ARTIFACT REQUEST|music-list motion verify ARTIFACT|music-list motion review-plan --output PLAN ARTIFACT|music-list motion review-apply --output REQUEST ARTIFACT PLAN DECISIONS>"
                .to_owned(),
        ),
    }
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
        "scorepeek-corpus {}\n\nUsage:\n  scorepeek-corpus diagnostic replay-video --video FILE --profile NAME --output DIRECTORY\n  scorepeek-corpus diagnostic verify DIRECTORY\n  scorepeek-corpus diagnostic convert-v2 --diagnostic DIRECTORY --recognition DIRECTORY --output DIRECTORY\n  scorepeek-corpus corpus import-diagnostic --store ROOT --diagnostic DIRECTORY --review-draft FILE\n  scorepeek-corpus review show --draft FILE\n  scorepeek-corpus review apply --store ROOT --draft FILE --labels FILE\n  scorepeek-corpus corpus replay --store ROOT",
        env!("CARGO_PKG_VERSION")
    );
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::run;

    #[test]
    fn removed_recording_store_routes_are_not_dispatchable() {
        for command in [
            [
                "ingest",
                "--store",
                "/tmp/store",
                "/tmp/video",
                "/tmp/request",
            ]
            .as_slice(),
            ["generation", "seal", "--store", "/tmp/store", "generation"].as_slice(),
            ["replay", "validate", "--store", "/tmp/store", "/tmp/suite"].as_slice(),
        ] {
            let args = command.iter().map(OsString::from).collect::<Vec<_>>();
            assert!(run(&args).is_err());
        }
    }
}
