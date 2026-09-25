// Assemble one 4:3 board from unscaled native captures at matching widget coordinates.
// Usage: deno run --allow-read --allow-write --allow-run=magick scripts/compose-skin-review.ts <scene-dir> <native-root> <new-review.png>

const [sceneDir, nativeRoot, outputPath] = Deno.args;
if (!sceneDir || !nativeRoot || !outputPath || Deno.args.length !== 3) {
  throw new Error(
    "usage: compose-skin-review.ts <scene-dir> <native-root> <new-review.png>",
  );
}
try {
  await Deno.stat(outputPath);
  throw new Error(`output already exists: ${outputPath}`);
} catch (error) {
  if (!(error instanceof Deno.errors.NotFound)) throw error;
}

type Widget = {
  kind: string;
  x: number;
  y: number;
  width: number;
  height: number;
};
type Scene = { logical_size: number[]; canvases: { widgets: Widget[] }[] };
type NativeManifest = {
  status: string;
  operations: { action: string; status: string; image?: string }[];
};
const cases = JSON.parse(await Deno.readTextFile(`${sceneDir}/cases.json`)) as {
  id: string;
}[];
if (cases.length !== 8) throw new Error("expected eight cases");

async function nativeCapture(id: string): Promise<string> {
  const manifest = JSON.parse(
    await Deno.readTextFile(`${nativeRoot}/${id}/manifest.json`),
  ) as NativeManifest;
  const capture = manifest.operations.find((operation) =>
    operation.action === `capture-review-${id}`
  );
  if (
    manifest.status !== "complete" || capture?.status !== "success" ||
    !capture.image
  ) {
    throw new Error(`${id}: incomplete native capture`);
  }
  return `${nativeRoot}/${id}/${capture.image}`;
}
async function magick(args: string[]): Promise<void> {
  const result = await new Deno.Command("magick", { args, stderr: "piped" })
    .output();
  if (!result.success) {
    throw new Error(new TextDecoder().decode(result.stderr));
  }
}

const temporary = await Deno.makeTempDir({ prefix: "scorepeek-review-" });
try {
  const board = `${temporary}/board.png`;
  await Deno.copyFile(await nativeCapture("01"), board);
  const placements = [];
  for (const { id } of cases) {
    const scene = JSON.parse(
      await Deno.readTextFile(`${sceneDir}/${id}.json`),
    ) as Scene;
    if (scene.logical_size[0] !== 1920 || scene.logical_size[1] !== 1440) {
      throw new Error(`${id}: unexpected source size`);
    }
    for (const kind of ["selection", "score"]) {
      const widget = scene.canvases[0].widgets.find((item) =>
        item.kind === kind
      );
      if (!widget) throw new Error(`${id}: missing ${kind}`);
      placements.push({ caseId: id, ...widget });
      if (id === "01") continue;
      const pad = 4;
      const x = widget.x - pad;
      const y = widget.y - pad;
      const crop = `${temporary}/crop.png`;
      const next = `${temporary}/next.png`;
      await magick([
        await nativeCapture(id),
        "-crop",
        `${widget.width + 2 * pad}x${widget.height + 2 * pad}+${x}+${y}`,
        "+repage",
        crop,
      ]);
      await magick([
        board,
        crop,
        "-geometry",
        `+${x}+${y}`,
        "-compose",
        "Over",
        "-composite",
        next,
      ]);
      await Deno.rename(next, board);
    }
  }
  await Deno.copyFile(board, outputPath);
  await Deno.writeTextFile(
    `${outputPath}.json`,
    `${
      JSON.stringify(
        {
          schema_version: 1,
          source: "native captures only",
          size: [1920, 1440],
          base_case: "01",
          placements,
        },
        null,
        2,
      )
    }\n`,
  );
} finally {
  await Deno.remove(temporary, { recursive: true });
}
