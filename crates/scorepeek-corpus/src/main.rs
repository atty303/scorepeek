use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use scorepeek_corpus::{
    TemporalEvaluationPolicy, apply_music_list_motion_review, apply_review, convert_v2_diagnostic,
    evaluate_temporal_corpus, import_diagnostic, inspect_music_list_row_observation_draft,
    inspect_review, measure_music_list_motion, plan_music_list_motion_review,
    plan_music_select_motion_review, render_synthetic_title_set, replay_corpus, replay_video,
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
    if let Some(result) = run_music_select_motion(args) {
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

fn run_music_select_motion(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        music_select,
        motion,
        review_plan,
        store_flag,
        store,
        session_flag,
        session,
        video_flag,
        video,
        output_flag,
        output,
    ] = args
    else {
        return None;
    };
    if music_select == "music-select"
        && motion == "motion"
        && review_plan == "review-plan"
        && store_flag == "--store"
        && session_flag == "--session-sha256"
        && video_flag == "--video"
        && output_flag == "--output"
    {
        let summary = plan_music_select_motion_review(
            &PathBuf::from(store),
            &session.to_string_lossy(),
            &PathBuf::from(video),
            &PathBuf::from(output),
        )
        .map_err(|error| format!("music-select motion review planning failed: {error}"));
        return Some(
            summary.and_then(|value| print_json(&value, "music-select motion review planning")),
        );
    }
    None
}

#[allow(clippy::too_many_lines)]
fn run_frame_corpus(args: &[OsString]) -> Option<Result<(), String>> {
    if args.starts_with(&[OsString::from("temporal"), OsString::from("evaluate")]) {
        return Some(run_temporal_evaluation(&args[2..]));
    }
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

fn run_temporal_evaluation(args: &[OsString]) -> Result<(), String> {
    let mut store = None;
    let mut policies = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let flag = &args[index];
        let value = args.get(index + 1).ok_or_else(temporal_usage)?;
        if flag == "--store" && store.is_none() {
            store = Some(PathBuf::from(value));
        } else if flag == "--policy" {
            policies.push(parse_temporal_policy(value)?);
        } else {
            return Err(temporal_usage());
        }
        index += 2;
    }
    let store = store.ok_or_else(temporal_usage)?;
    if policies.is_empty() {
        policies = vec![
            TemporalEvaluationPolicy::new(2, 250)
                .expect("registered runtime temporal policy is valid"),
            TemporalEvaluationPolicy::new(3, 250)
                .expect("registered comparison temporal policy is valid"),
        ];
    }
    evaluate_temporal_corpus(&store, &policies)
        .map_err(|error| format!("temporal corpus evaluation failed: {error}"))
        .and_then(|summary| print_json(&summary, "temporal corpus evaluation"))
}

fn parse_temporal_policy(value: &OsString) -> Result<TemporalEvaluationPolicy, String> {
    let value = value.to_str().ok_or_else(temporal_usage)?;
    let (required, gap) = value.split_once(':').ok_or_else(temporal_usage)?;
    let required = required.parse::<u8>().map_err(|_| temporal_usage())?;
    let gap = gap.parse::<u64>().map_err(|_| temporal_usage())?;
    TemporalEvaluationPolicy::new(required, gap).map_err(|error| error.to_string())
}

fn temporal_usage() -> String {
    "usage: scorepeek-corpus temporal evaluate --store ROOT [--policy OBSERVATIONS:GAP_MS ...]"
        .to_owned()
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
            "usage: scorepeek-corpus <diagnostic replay-video --video FILE --profile NAME --output DIRECTORY|diagnostic verify DIRECTORY|diagnostic convert-v2 --diagnostic DIRECTORY --recognition DIRECTORY --output DIRECTORY|corpus import-diagnostic --store ROOT --diagnostic DIRECTORY --review-draft FILE|review show --draft FILE|review apply --store ROOT --draft FILE --labels FILE|corpus replay --store ROOT|temporal evaluate --store ROOT [--policy OBSERVATIONS:GAP_MS ...]|synthetic render --output DIRECTORY REQUEST|music-list observation-draft inspect|verify DOCUMENT|music-list motion measure --output ARTIFACT REQUEST|music-list motion verify ARTIFACT|music-list motion review-plan --output PLAN ARTIFACT|music-list motion review-apply --output REQUEST ARTIFACT PLAN DECISIONS|music-select motion review-plan --store ROOT --session-sha256 SHA256 --video FILE --output FILE>"
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
        "scorepeek-corpus {}\n\nUsage:\n  scorepeek-corpus diagnostic replay-video --video FILE --profile NAME --output DIRECTORY\n  scorepeek-corpus diagnostic verify DIRECTORY\n  scorepeek-corpus diagnostic convert-v2 --diagnostic DIRECTORY --recognition DIRECTORY --output DIRECTORY\n  scorepeek-corpus corpus import-diagnostic --store ROOT --diagnostic DIRECTORY --review-draft FILE\n  scorepeek-corpus review show --draft FILE\n  scorepeek-corpus review apply --store ROOT --draft FILE --labels FILE\n  scorepeek-corpus corpus replay --store ROOT\n  scorepeek-corpus temporal evaluate --store ROOT [--policy OBSERVATIONS:GAP_MS ...]\n  scorepeek-corpus music-select motion review-plan --store ROOT --session-sha256 SHA256 --video FILE --output FILE",
        env!("CARGO_PKG_VERSION")
    );
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{parse_temporal_policy, run};

    #[test]
    fn temporal_policy_requires_bounded_observation_and_gap_pair() {
        let parsed = parse_temporal_policy(&OsString::from("3:250")).unwrap();
        assert_eq!(parsed.required_observations, 3);
        assert_eq!(parsed.maximum_gap_ms, 250);
        for invalid in ["1:250", "2:0", "17:250", "2:60001", "2", "two:250"] {
            assert!(parse_temporal_policy(&OsString::from(invalid)).is_err());
        }
    }

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
