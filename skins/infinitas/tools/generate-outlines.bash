#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$root"
work=$(mktemp -d "${TMPDIR:-/tmp}/infinitas-outlines.XXXXXX")
trap 'rm -rf "$work"' EXIT
# Reuse the renderer's locked font library as an offline authoring dependency.
cargo build --locked -p skrifa --message-format=json > "$work/artifacts.jsonl"
infinitas_skrifa=$(deno eval 'const rows=(await Deno.readTextFile(Deno.args[0])).trim().split("\n").map(s=>JSON.parse(s));const path=rows.filter(r=>r.reason==="compiler-artifact"&&r.target.name==="skrifa").flatMap(r=>r.filenames).find(p=>p.endsWith(".rlib"));if(!path)throw new Error("skrifa artifact missing");console.log(path);' "$work/artifacts.jsonl")
rustc --edition=2024 skins/infinitas/tools/outline.rs --extern "skrifa=$infinitas_skrifa" -L "dependency=$(dirname "$infinitas_skrifa")/deps" -o "$work/outline"
"$work/outline"
rustfmt --edition 2024 skins/infinitas/src/glyphs.rs
