# Recording field simulation

The recording simulation consumes a private canonical extraction whose source-manifest digest is
bound through its selected recording manifest to the original corpus recording. It does not decode
an MKV inside the runtime. Every extraction frame becomes the same profile-bound canonical owner
used after live normalization; screen routing, registered field inference, and full-catalog scoring
are shared with live capture.

First prepare a one-line canonical JSON profile candidate outside the repository. The candidate
must bind the recording manifest, coverage label, complete extraction, current canonical layout,
registered resources, replay pacing, diagnostic sampling, and ordered result windows. Authoring is
create-only. The episode label timestamps must exactly and uniquely cover every result observation
in the reviewed coverage label:

```bash
mise run recognition:recording-simulation-profile-author -- --candidate /absolute/private/candidate.json --candidate-sha256 CANDIDATE_SHA256 --recording-manifest /absolute/private/recording.json --coverage-label /absolute/private/label.json --extraction /absolute/private/canonical --output /absolute/private/profile.json
```

Run the profile against an existing private diagnostic root:

```bash
mise run recognition:recording-simulation -- --profile /absolute/private/profile.json --profile-sha256 PROFILE_SHA256 --extraction /absolute/private/canonical --diagnostic-root /absolute/private/diagnostics --catalog-store /absolute/private/catalog --bundle /absolute/private/model-bundle --run-id UNIQUE_RUN_ID --build-sha256 BUILD_SHA256 --recording enabled
```

Success requires every canonical frame to be inspected, every submitted screen to retain a
full-catalog candidate set, each expected result window to be detected, and its exact `CLEAR TYPE`
value to occur on at least two frames. Diagnostic recording must finish complete when enabled.
The current command prints counts and digests and therefore proves field-path execution rather than
song-recognition correctness. ADR 0037 supersedes that counts-only evidence boundary for the next
recognition checkpoint: the operator-owned local artifact must retain bounded exact OCR strings, a
run-scoped exact catalog display/comparison string table, candidate string references, song IDs,
complete candidate metrics, decisions and reasons, and reviewed expected-versus-observed values.
Pixel data remains in the referenced bounded image artifact rather than being duplicated in the
value record.
