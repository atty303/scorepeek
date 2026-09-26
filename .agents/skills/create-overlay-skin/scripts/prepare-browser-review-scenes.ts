// Convert native review scenarios to matching OBS fixture scenes without changing data.
// Usage: deno run --allow-read --allow-write prepare-browser-review-scenes.ts NATIVE_SCENE_DIR OUTPUT_DIR SLUG
const [nativeDirectory, outputDirectory, slug] = Deno.args;
if (!nativeDirectory || !outputDirectory || !slug || Deno.args.length !== 3) {
  throw new Error(
    "usage: prepare-browser-review-scenes.ts NATIVE_SCENE_DIR OUTPUT_DIR SLUG",
  );
}
await Deno.mkdir(outputDirectory);
const cases = JSON.parse(
  await Deno.readTextFile(`${nativeDirectory}/cases.json`),
) as {
  id: string;
  paint_padding: number;
  media: { fps: number; duration_ms: number; motion_periods_ms: number[] };
}[];
const caseIds = [
  ...Array.from(
    { length: 8 },
    (_, index) => String(index + 1).padStart(2, "0"),
  ),
  "rank-a",
  "rank-aa",
  "rank-aaa",
];
for (const caseId of caseIds) {
  const reviewCase = cases.find((item) => item.id === caseId) ??
    (caseId.startsWith("rank-") ? cases[0] : undefined);
  if (!reviewCase) throw new Error(`missing review case ${caseId}`);
  const scenario = JSON.parse(
    await Deno.readTextFile(`${nativeDirectory}/${caseId}.json`),
  );
  const canvas = scenario.canvases?.[0];
  const state = scenario.actions?.find((action: { action: string }) =>
    action.action === "set_state"
  )?.state;
  if (!canvas || !state || !canvas.skin || !Array.isArray(canvas.widgets)) {
    throw new Error(`incomplete native review scenario: ${caseId}`);
  }
  const scene = {
    schema: "scorepeek-skin-preview-scene-v1",
    skins: [{ slug, id: canvas.skin }],
    canvas: {
      width: canvas.width,
      height: canvas.height,
      skin_properties: canvas.skin_properties ?? {},
      paint_padding: reviewCase.paint_padding,
      widgets: canvas.widgets,
    },
    state,
    media: reviewCase.media,
  };
  await Deno.writeTextFile(
    `${outputDirectory}/${caseId}.json`,
    JSON.stringify(scene),
  );
}
