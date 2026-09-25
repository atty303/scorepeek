// Combine production browser captures into one all-variation review video.
// Usage: deno run --allow-read --allow-write --allow-run=magick,ffmpeg,ffprobe compose-browser-review-video.ts SCENE_DIR BROWSER_OUTPUT_DIR REVIEW.webm

const [sceneDirectory, browserDirectory, outputPath] = Deno.args;
if (
  !sceneDirectory || !browserDirectory || !outputPath || Deno.args.length !== 3
) {
  throw new Error(
    "usage: compose-browser-review-video.ts SCENE_DIR BROWSER_OUTPUT_DIR REVIEW.webm",
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
type Timing = {
  fps: number;
  duration_ms: number;
  frame_count: number;
  monotonic_base_ms: number;
  paint_checks: number;
};
const cases = JSON.parse(
  await Deno.readTextFile(`${sceneDirectory}/cases.json`),
) as { id: string; paint_padding: number }[];
if (cases.length !== 8) throw new Error("expected eight review cases");
const timing = JSON.parse(
  await Deno.readTextFile(`${browserDirectory}/01/timing.json`),
) as Timing;
if (
  timing.fps <= 0 || timing.monotonic_base_ms !== 0 ||
  timing.paint_checks !== 6 ||
  timing.frame_count !== timing.duration_ms * timing.fps / 1000
) {
  throw new Error("invalid browser frame timing");
}
const overlays: { id: string; kind: string; x: number; y: number }[] = [];
for (const { id, paint_padding } of cases) {
  const candidate = JSON.parse(
    await Deno.readTextFile(`${browserDirectory}/${id}/timing.json`),
  ) as Timing;
  if (JSON.stringify(candidate) !== JSON.stringify(timing)) {
    throw new Error(`${id}: browser timing mismatch`);
  }
  const scene = JSON.parse(
    await Deno.readTextFile(`${sceneDirectory}/${id}.json`),
  ) as Scene;
  if (scene.logical_size?.[0] !== 1920 || scene.logical_size?.[1] !== 1440) {
    throw new Error(`${id}: unexpected review size`);
  }
  if (id === "01") continue;
  for (const kind of ["selection", "score"]) {
    const widget = scene.canvases[0]?.widgets.find((item) =>
      item.kind === kind
    );
    if (!widget) throw new Error(`${id}: missing ${kind}`);
    overlays.push({
      id,
      kind,
      x: Math.max(0, widget.x - paint_padding),
      y: Math.max(0, widget.y - paint_padding),
    });
  }
}

async function run(command: string, args: string[]): Promise<string> {
  const result = await new Deno.Command(command, {
    args,
    stdout: "piped",
    stderr: "piped",
  }).output();
  if (!result.success) {
    throw new Error(`${command}: ${new TextDecoder().decode(result.stderr)}`);
  }
  return new TextDecoder().decode(result.stdout);
}

const frameDirectory = await Deno.makeTempDir({
  prefix: "scorepeek-review-video-",
});
try {
  for (let frame = 0; frame < timing.frame_count; frame += 1) {
    const number = String(frame).padStart(3, "0");
    const args = [
      "-size",
      "1920x1440",
      "xc:black",
      `${browserDirectory}/01/frame-${number}.png`,
      "-compose",
      "Over",
      "-composite",
    ];
    for (const overlay of overlays) {
      args.push(
        `${browserDirectory}/${overlay.id}/${overlay.kind}-${number}.png`,
        "-geometry",
        `+${overlay.x}+${overlay.y}`,
        "-compose",
        "Over",
        "-composite",
      );
    }
    args.push("-quality", "100", `${frameDirectory}/frame-${number}.jpg`);
    await run("magick", args);
  }
  await run("ffmpeg", [
    "-hide_banner",
    "-loglevel",
    "error",
    "-framerate",
    String(timing.fps),
    "-i",
    `${frameDirectory}/frame-%03d.jpg`,
    "-c:v",
    "libvpx-vp9",
    "-crf",
    "24",
    "-b:v",
    "0",
    "-pix_fmt",
    "yuv420p",
    "-an",
    "-t",
    String(timing.duration_ms / 1000),
    outputPath,
  ]);
  const probe = JSON.parse(
    await run("ffprobe", [
      "-v",
      "error",
      "-show_entries",
      "stream=codec_name,codec_type,width,height,r_frame_rate:format=duration",
      "-of",
      "json",
      outputPath,
    ]),
  );
  const video = probe.streams.find((stream: { codec_type: string }) =>
    stream.codec_type === "video"
  );
  if (
    video?.codec_name !== "vp9" || video.width !== 1920 ||
    video.height !== 1440 ||
    Math.abs(Number(probe.format.duration) - timing.duration_ms / 1000) > 0.1
  ) {
    throw new Error("review video metadata mismatch");
  }
  await Deno.writeTextFile(
    `${outputPath}.json`,
    JSON.stringify(
      {
        schema: "scorepeek-skin-browser-review-v1",
        source: "production browser captures",
        size: [1920, 1440],
        timing,
        base_case: "01",
        overlays,
        video,
      },
      null,
      2,
    ),
  );
} finally {
  await Deno.remove(frameDirectory, { recursive: true });
}
