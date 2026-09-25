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
for (let index = 1; index <= 8; index += 1) {
  const caseId = String(index).padStart(2, "0");
  const reviewCase = cases.find((item) => item.id === caseId);
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
