// Check scene coverage, changing numeric data, native captures and browser video.
// Usage: deno run --allow-read --allow-run=magick scripts/check-skin-review-scenes.ts <scene-dir> <native-output-root> <review-video>

const [sceneDir, nativeRoot, reviewVideo] = Deno.args;
if (!sceneDir || !nativeRoot || !reviewVideo || Deno.args.length !== 3) {
  throw new Error(
    "usage: check-skin-review-scenes.ts <scene-dir> <native-output-root> <review-video>",
  );
}

type ReviewCase = {
  id: string;
  play: string;
  difficulty: string;
  rank: string;
  clear: string;
  paint_padding: number;
  background_motion: "animated" | "still";
  media: {
    fps: number;
    duration_ms: number;
    motion_periods_ms: number[];
    native_motion_seconds: number[];
  };
};
type ReviewState = {
  system: string;
  result_signal: string;
  chart: {
    play_type: string;
    difficulty: string;
    level: number;
    notes: number;
  };
  best: { score: string; dj_level: string; clear: string; miss: string };
  detail: Record<string, string>;
  history: {
    plays: {
      score: string;
      dj_level: string;
      clear: string;
      miss: string;
    }[];
    graph: { score_ratio: number; miss_ratio: number | null }[];
  };
};
type ReviewScene = {
  logical_size: number[];
  canvases: {
    skin_properties?: { background?: string };
    widgets: {
      kind: string;
      x: number;
      y: number;
      width: number;
      height: number;
      settings?: {
        history_count?: number;
        graph_months?: number;
        title?: string;
      };
      skin_properties?: Record<string, unknown>;
    }[];
  }[];
  actions: {
    action: string;
    state?: ReviewState;
    name?: string;
    seconds?: number;
  }[];
};
type NativeManifest = {
  status: string;
  completeness: string;
  logical_size: number[];
  operations: {
    action: string;
    status: string;
    image?: string;
    layout?: string;
  }[];
};
type NativeLayout = { elements: { selector: string; matches: number[][] }[] };

function requireCondition(
  condition: unknown,
  description: string,
): asserts condition {
  if (!condition) throw new Error(description);
}
function exactly(values: string[], expected: string[], label: string): void {
  requireCondition(
    JSON.stringify([...new Set(values)].sort()) ===
      JSON.stringify([...expected].sort()),
    `${label}: incomplete coverage`,
  );
}

const cases = JSON.parse(
  await Deno.readTextFile(`${sceneDir}/cases.json`),
) as ReviewCase[];
requireCondition(
  Array.isArray(cases) && cases.length === 8,
  "expected eight cases",
);
exactly(cases.map((item) => item.play), ["single", "double"], "play type");
exactly(cases.map((item) => item.difficulty), [
  "beginner",
  "normal",
  "hyper",
  "another",
  "leggendaria",
], "difficulty");
exactly(cases.map((item) => item.rank), ["A", "AA", "AAA"], "DJ LEVEL");
exactly(cases.map((item) => item.clear), [
  "NO PLAY",
  "FAILED",
  "ASSIST",
  "EASY",
  "CLEAR",
  "HARD",
  "EX HARD",
  "FULL COMBO",
], "clear type");

const numericFields = [
  "level",
  "notes",
  "score",
  "bestMiss",
  "pgreat",
  "great",
  "good",
  "bad",
  "poor",
  "fast",
  "slow",
  "combo_break",
  "historyScore",
  "graphScore",
  "graphMiss",
];
const numbers = new Map(numericFields.map((field) => [field, [] as number[]]));
let hasThreeDigitNotes = false;
let hasThreeDigitScore = false;
let hasMaxMinus = false;
for (const item of cases) {
  const id = item.id;
  const scene = JSON.parse(
    await Deno.readTextFile(`${sceneDir}/${id}.json`),
  ) as ReviewScene;
  requireCondition(
    scene.logical_size?.[0] === 1920 && scene.logical_size?.[1] === 1440,
    `${id}: review image must be 4:3`,
  );
  const widgetKinds = scene.canvases?.[0]?.widgets?.map((widget) =>
    widget.kind
  ) ?? [];
  exactly(widgetKinds, [
    "status",
    "selection",
    "score",
    "history-list",
    "history-graph",
    "empty",
  ], `${id} widgets`);
  const state = scene.actions?.find((action) => action.action === "set_state")
    ?.state;
  requireCondition(state, `${id}: missing synthetic state`);
  requireCondition(
    state.chart.play_type === item.play &&
      state.chart.difficulty === item.difficulty &&
      state.best.dj_level === item.rank &&
      state.best.clear === item.clear,
    `${id}: case metadata disagrees with state`,
  );
  const notes = Number(state.chart.notes);
  const score = Number(state.best.score);
  hasThreeDigitNotes ||= notes >= 100 && notes <= 999;
  hasThreeDigitScore ||= score >= 100 && score <= 999;
  hasMaxMinus ||= item.rank === "AAA" && 2 * notes - score <
      score - Math.ceil(16 * notes / 9);
  if (id === "01") {
    requireCondition(
      state.system === "error" && state.result_signal === "inactive",
      "01: status must show error and inactive lamps",
    );
  }
  const rankIndex = Math.min(8, Math.floor(9 * score / (2 * notes)));
  requireCondition(
    (rankIndex === 6
      ? "A"
      : rankIndex === 7
      ? "AA"
      : rankIndex === 8
      ? "AAA"
      : "other") === item.rank,
    `${id}: SCORE, notes, and DJ LEVEL disagree`,
  );
  const fields: Record<string, number> = {
    level: Number(state.chart.level),
    notes,
    score,
    bestMiss: Number(state.best.miss),
    pgreat: Number(state.detail.pgreat),
    great: Number(state.detail.great),
    good: Number(state.detail.good),
    bad: Number(state.detail.bad),
    poor: Number(state.detail.poor),
    fast: Number(state.detail.fast),
    slow: Number(state.detail.slow),
    combo_break: Number(state.detail.combo_break),
    historyScore: Number(state.history.plays?.[0]?.score),
    graphScore: Number(state.history.graph?.[0]?.score_ratio),
    graphMiss: Number(state.history.graph?.[0]?.miss_ratio),
  };
  for (const [field, value] of Object.entries(fields)) {
    requireCondition(Number.isFinite(value), `${id}: invalid ${field}`);
    numbers.get(field)?.push(value);
  }
  const manifest = JSON.parse(
    await Deno.readTextFile(`${nativeRoot}/${id}/manifest.json`),
  ) as NativeManifest;
  requireCondition(
    manifest.status === "complete" && manifest.completeness === "complete",
    `${id}: incomplete native render`,
  );
  requireCondition(
    manifest.logical_size?.[0] === 1920 && manifest.logical_size?.[1] === 1440,
    `${id}: native size differs from scene`,
  );
  const initial = manifest.operations?.find((operation) =>
    operation.action === "initial"
  );
  requireCondition(
    initial?.status === "success" && initial.image && initial.layout,
    `${id}: missing native initial render`,
  );
  requireCondition(
    (await Deno.stat(`${nativeRoot}/${id}/${initial.image}`)).size > 0,
    `${id}: empty native initial image`,
  );
  const initialLayout = JSON.parse(
    await Deno.readTextFile(`${nativeRoot}/${id}/${initial.layout}`),
  ) as NativeLayout;
  const initialRectangles =
    initialLayout.elements?.find((element) => element.selector === "*")
      ?.matches ?? [];
  for (const widget of scene.canvases[0].widgets) {
    requireCondition(
      initialRectangles.some((rect) =>
        [widget.x, widget.y, widget.width, widget.height].every((
          value,
          index,
        ) => Number.isFinite(rect[index]) && Math.abs(rect[index] - value) <= 1)
      ),
      `${id}: no initial native DOM rectangle for ${widget.kind} widget bounds`,
    );
  }
  const capture = manifest.operations?.find((operation) =>
    operation.action === `capture-review-${id}`
  );
  requireCondition(
    capture?.status === "success" && capture?.image && capture?.layout,
    `${id}: missing native capture`,
  );
  requireCondition(
    (await Deno.stat(`${nativeRoot}/${id}/${capture.image}`)).size > 0,
    `${id}: empty native image`,
  );
  const layout = JSON.parse(
    await Deno.readTextFile(`${nativeRoot}/${id}/${capture.layout}`),
  ) as NativeLayout;
  const rectangles =
    layout.elements?.find((element) => element.selector === "*")
      ?.matches ?? [];
  requireCondition(rectangles.length > 0, `${id}: empty native DOM layout`);
  for (const widget of scene.canvases[0].widgets) {
    requireCondition(
      rectangles.some((rect) =>
        [widget.x, widget.y, widget.width, widget.height].every((
          value,
          index,
        ) => Number.isFinite(rect[index]) && Math.abs(rect[index] - value) <= 1)
      ),
      `${id}: no native DOM rectangle for ${widget.kind} widget bounds`,
    );
  }
  for (const seconds of item.media.native_motion_seconds) {
    const motion = manifest.operations?.find((operation) =>
      operation.action ===
        `capture-review-${id}-at-${String(seconds).replaceAll(".", "-")}`
    );
    requireCondition(
      motion?.status === "success" && motion.image && motion.layout,
      `${id}: missing native motion capture at ${seconds}s`,
    );
    requireCondition(
      (await Deno.stat(`${nativeRoot}/${id}/${motion.image}`)).size > 0,
      `${id}: empty native motion image at ${seconds}s`,
    );
    requireCondition(
      (await Deno.stat(`${nativeRoot}/${id}/${motion.layout}`)).size > 0,
      `${id}: empty native motion layout at ${seconds}s`,
    );
  }
}
requireCondition(hasThreeDigitNotes, "missing three-digit NOTES variation");
requireCondition(hasThreeDigitScore, "missing three-digit SCORE variation");
requireCondition(hasMaxMinus, "missing MAX- variation");
for (const [field, values] of numbers) {
  requireCondition(
    new Set(values).size === 8,
    `${field}: repeated numeric value across cases`,
  );
}
requireCondition((await Deno.stat(reviewVideo)).size > 0, "empty review video");
const assembly = JSON.parse(await Deno.readTextFile(`${reviewVideo}.json`)) as {
  schema: string;
  source: string;
  size: number[];
  timing: {
    fps: number;
    duration_ms: number;
    frame_count: number;
    monotonic_base_ms: number;
    paint_checks: number;
  };
  overlays: { id: string; kind: string }[];
  video: { codec_name: string; width: number; height: number };
};
requireCondition(
  assembly.schema === "scorepeek-skin-browser-review-v1" &&
    assembly.source === "production browser captures",
  "non-browser review source",
);
requireCondition(
  assembly.size[0] === 1920 && assembly.size[1] === 1440 &&
    assembly.video.codec_name === "vp9" && assembly.video.width === 1920 &&
    assembly.video.height === 1440 && assembly.timing.duration_ms > 0 &&
    assembly.timing.monotonic_base_ms === 0 &&
    assembly.timing.paint_checks === 6 &&
    assembly.timing.frame_count ===
      assembly.timing.duration_ms * assembly.timing.fps / 1000,
  "invalid review video metadata",
);
for (const item of cases) {
  requireCondition(
    item.paint_padding >= 0 && Number.isInteger(item.paint_padding),
    `${item.id}: invalid paint padding`,
  );
  requireCondition(
    item.media.duration_ms === assembly.timing.duration_ms &&
      item.media.fps === assembly.timing.fps,
    `${item.id}: review timing mismatch`,
  );
  for (const period of item.media.motion_periods_ms) {
    requireCondition(
      period + 1000 <= assembly.timing.duration_ms,
      `${item.id}: loop boundary outside video`,
    );
    requireCondition(
      [
        period - Math.round(1000 / item.media.fps),
        period,
        period + Math.round(1000 / item.media.fps),
      ].every((ms) =>
        item.media.native_motion_seconds.includes(
          Number((ms / 1000).toFixed(3)),
        )
      ),
      `${item.id}: missing loop-boundary native samples`,
    );
  }
}
for (const { id } of cases.filter((item) => item.id !== "01")) {
  exactly(
    assembly.overlays.filter((item) => item.id === id).map((item) => item.kind),
    ["selection", "score"],
    `${id}: review placements`,
  );
}
let controlledRankState: ReviewState | undefined;
const browserRoot = reviewVideo.includes("/")
  ? reviewVideo.slice(0, reviewVideo.lastIndexOf("/")) || "/"
  : ".";
for (const [rank, rankIndex] of [["A", 6], ["AA", 7], ["AAA", 8]] as const) {
  const id = `rank-${rank.toLowerCase()}`;
  const scene = JSON.parse(
    await Deno.readTextFile(`${sceneDir}/${id}.json`),
  ) as ReviewScene;
  const state = scene.actions.find((action) => action.action === "set_state")
    ?.state;
  requireCondition(state, `${id}: missing controlled state`);
  const score = Number(state.best.score);
  const notes = Number(state.chart.notes);
  requireCondition(
    state.best.dj_level === rank &&
      score === Math.ceil(2 * notes * rankIndex / 9) + 12 &&
      state.best.clear === "CLEAR" && state.best.miss === "7" &&
      state.history.plays[0].score === state.best.score &&
      state.history.plays[0].dj_level === rank &&
      state.history.plays[0].clear === "CLEAR" &&
      state.history.plays[0].miss === "7",
    `${id}: rank comparison is not score-consistent and controlled`,
  );
  const normalized = structuredClone(state);
  normalized.best.score = "<rank score>";
  normalized.best.dj_level = "<rank>";
  normalized.history.plays[0].score = "<rank score>";
  normalized.history.plays[0].dj_level = "<rank>";
  if (controlledRankState) {
    requireCondition(
      JSON.stringify(normalized) === JSON.stringify(controlledRankState),
      `${id}: non-rank state differs from other rank comparison scenes`,
    );
  } else controlledRankState = normalized;
  const manifest = JSON.parse(
    await Deno.readTextFile(`${nativeRoot}/${id}/manifest.json`),
  ) as NativeManifest;
  const capture = manifest.operations?.find((operation) =>
    operation.action === `capture-${id}`
  );
  requireCondition(
    manifest.status === "complete" &&
      manifest.completeness === "complete" &&
      capture?.status === "success" && capture.image && capture.layout,
    `${id}: missing controlled native capture`,
  );
  requireCondition(
    (await Deno.stat(`${nativeRoot}/${id}/${capture.image}`)).size > 0 &&
      (await Deno.stat(`${browserRoot}/${id}.png`)).size > 0,
    `${id}: empty native or browser comparison image`,
  );
}
const referenceScene = JSON.parse(
  await Deno.readTextFile(`${sceneDir}/01.json`),
) as ReviewScene;
const referenceState = referenceScene.actions.find((action) =>
  action.action === "set_state"
)?.state;
requireCondition(referenceState, "01: missing reference state");
requireCondition(
  ["animated", "still"].includes(cases[0].background_motion) &&
    cases.every((item) =>
      item.background_motion === cases[0].background_motion
    ),
  "inconsistent background motion declaration",
);
for (
  const background of [
    "off",
    "static",
    ...(cases[0].background_motion === "animated" ? ["animated"] : []),
  ]
) {
  const id = `background-${background}`;
  const scene = JSON.parse(
    await Deno.readTextFile(`${sceneDir}/${id}.json`),
  ) as ReviewScene;
  const state = scene.actions.find((action) => action.action === "set_state")
    ?.state;
  requireCondition(
    (background === "off"
      ? ["none", "off"].includes(
        scene.canvases[0]?.skin_properties?.background ?? "",
      )
      : scene.canvases[0]?.skin_properties?.background === background) &&
      JSON.stringify(scene.canvases[0].widgets) ===
        JSON.stringify(referenceScene.canvases[0].widgets) &&
      JSON.stringify(state) === JSON.stringify(referenceState),
    `${id}: background comparison changed widget content or layout`,
  );
  const manifest = JSON.parse(
    await Deno.readTextFile(`${nativeRoot}/${id}/manifest.json`),
  ) as NativeManifest;
  requireCondition(
    manifest.status === "complete" && manifest.completeness === "complete",
    `${id}: incomplete native background render`,
  );
  const expectedCaptures = scene.actions.filter((action) =>
    action.action === "capture"
  );
  const sampledSeconds = scene.actions.filter((action) =>
    action.action === "advance_animation"
  ).map((action) => action.seconds);
  requireCondition(
    JSON.stringify(sampledSeconds) ===
        JSON.stringify(cases[0].media.native_motion_seconds) &&
      (cases[0].background_motion === "still" ||
        cases[0].media.motion_periods_ms.length > 0) &&
      cases[0].media.motion_periods_ms.every((period) =>
        [1, 2, 3].every((quarter) =>
          sampledSeconds.includes(
            Number((Math.round(quarter * period / 4) / 1000).toFixed(3)),
          )
        )
      ) &&
      JSON.stringify(expectedCaptures.map((action) => action.name)) ===
        JSON.stringify([
          id,
          ...sampledSeconds.map((seconds) => `${id}-at-${seconds}`),
        ]),
    `${id}: missing phase-sensitive native background samples`,
  );
  for (const expected of expectedCaptures) {
    requireCondition(expected.name, `${id}: unnamed native background capture`);
    const action = `capture-${expected.name.replace(/[^A-Za-z0-9_-]/g, "-")}`;
    const capture = manifest.operations?.find((operation) =>
      operation.action === action
    );
    requireCondition(
      capture?.status === "success" && capture.image && capture.layout,
      `${id}: missing native background capture ${action}`,
    );
    requireCondition(
      (await Deno.stat(`${nativeRoot}/${id}/${capture.image}`)).size > 0 &&
        (await Deno.stat(`${nativeRoot}/${id}/${capture.layout}`)).size > 0,
      `${id}: empty native background image or layout ${action}`,
    );
  }
  requireCondition(
    (await Deno.stat(`${browserRoot}/${id}.png`)).size > 0 &&
      (await Deno.stat(`${browserRoot}/${id}/frame-000.png`)).size > 0 &&
      (await Deno.stat(
          `${browserRoot}/${id}/frame-${
            String(Math.floor(assembly.timing.frame_count / 2)).padStart(3, "0")
          }.png`,
        )).size > 0,
    `${id}: missing browser background frames`,
  );
}
const boundaries = JSON.parse(
  await Deno.readTextFile(`${sceneDir}/boundary-cases.json`),
) as { opacityProperty: string; ids: string[] };
const expectedBoundaries = [
  ...[5, 10, 20, 50].map((count) => `boundary-history-${count}`),
  ...[1, 3, 6, 12].map((months) => `boundary-graph-${months}`),
  ...["wide", "tall", "titleless", "opacity-zero", "opacity-half"].map((name) =>
    `boundary-empty-${name}`
  ),
  "boundary-score-zero",
  "boundary-score-unknown",
  "boundary-score-invalid-notes",
];
exactly(boundaries.ids, expectedBoundaries, "boundary scenes");
requireCondition(boundaries.opacityProperty, "missing EMPTY opacity property");
for (const id of expectedBoundaries) {
  const scene = JSON.parse(
    await Deno.readTextFile(`${sceneDir}/${id}.json`),
  ) as ReviewScene;
  const widgets = scene.canvases[0]?.widgets ?? [];
  exactly(widgets.map((item) => item.kind), [
    "status",
    "selection",
    "score",
    "history-list",
    "history-graph",
    "empty",
  ], `${id} widgets`);
  const state = scene.actions.find((action) => action.action === "set_state")
    ?.state;
  requireCondition(state, `${id}: missing state`);
  if (id.startsWith("boundary-history-")) {
    const count = Number(id.slice("boundary-history-".length));
    requireCondition(
      widgets.find((item) => item.kind === "history-list")?.settings
            ?.history_count === count && state.history.plays.length === 50,
      `${id}: wrong row count or insufficient supplied rows`,
    );
  } else if (id.startsWith("boundary-graph-")) {
    const months = Number(id.slice("boundary-graph-".length));
    requireCondition(
      widgets.find((item) => item.kind === "history-graph")?.settings
        ?.graph_months === months,
      `${id}: wrong graph window`,
    );
  } else if (id.startsWith("boundary-empty-")) {
    const empty = widgets.find((item) => item.kind === "empty");
    requireCondition(empty, `${id}: missing EMPTY`);
    requireCondition(
      empty.skin_properties?.[boundaries.opacityProperty] ===
        (id === "boundary-empty-opacity-half" ? 0.5 : 0),
      `${id}: wrong EMPTY opacity`,
    );
    if (id === "boundary-empty-titleless") {
      requireCondition(empty.settings?.title === "", `${id}: title remains`);
    }
  } else {
    if (id === "boundary-score-invalid-notes") {
      requireCondition(
        state.chart.notes === 0 &&
          state.best.score === referenceState.best.score,
        `${id}: missing invalid-notes comparison`,
      );
    } else {
      requireCondition(
        state.best.score === (id === "boundary-score-zero" ? "0" : "") &&
          state.best.dj_level ===
            (id === "boundary-score-zero" ? "F" : ""),
        `${id}: missing zero/unknown comparison`,
      );
    }
  }
  const manifest = JSON.parse(
    await Deno.readTextFile(`${nativeRoot}/${id}/manifest.json`),
  ) as NativeManifest;
  const capture = manifest.operations.find((item) =>
    item.action === `capture-${id}`
  );
  requireCondition(
    manifest.status === "complete" &&
      manifest.completeness === "complete" &&
      capture?.status === "success" && capture.image && capture.layout &&
      (await Deno.stat(`${nativeRoot}/${id}/${capture.image}`)).size > 0 &&
      (await Deno.stat(`${nativeRoot}/${id}/${capture.layout}`)).size > 0 &&
      (await Deno.stat(`${browserRoot}/${id}.png`)).size > 0,
    `${id}: missing native/browser boundary evidence`,
  );
}
const emptyZero = JSON.parse(
  await Deno.readTextFile(`${sceneDir}/boundary-empty-opacity-zero.json`),
) as ReviewScene;
const emptyHalf = JSON.parse(
  await Deno.readTextFile(`${sceneDir}/boundary-empty-opacity-half.json`),
) as ReviewScene;
const halfWidgets = structuredClone(emptyHalf.canvases[0].widgets);
const halfEmpty = halfWidgets.find((item) => item.kind === "empty");
requireCondition(halfEmpty?.skin_properties, "missing half-opacity EMPTY");
halfEmpty.skin_properties[boundaries.opacityProperty] = 0;
const empty = emptyZero.canvases[0].widgets.find((item) =>
  item.kind === "empty"
);
requireCondition(empty, "missing zero-opacity EMPTY");
requireCondition(
  JSON.stringify(emptyZero.canvases[0].widgets) ===
      JSON.stringify(halfWidgets) &&
    [emptyZero, emptyHalf].every((scene) =>
      ["off", "none"].includes(
        scene.canvases[0].skin_properties?.background ?? "",
      )
    ) &&
    JSON.stringify(emptyZero.actions[0].state) ===
      JSON.stringify(emptyHalf.actions[0].state),
  "EMPTY opacity comparison changed another input",
);
const apertureWidth = Math.max(1, Math.floor(empty.width / 3));
const apertureHeight = Math.max(1, Math.floor(empty.height / 3));
const apertureX = empty.x + Math.floor((empty.width - apertureWidth) / 2);
const apertureY = empty.y + Math.floor((empty.height - apertureHeight) / 2);
async function aperturePixels(path: string): Promise<Uint8Array> {
  const result = await new Deno.Command("magick", {
    args: [
      path,
      "-crop",
      `${apertureWidth}x${apertureHeight}+${apertureX}+${apertureY}`,
      "+repage",
      "-depth",
      "8",
      "rgba:-",
    ],
    stdout: "piped",
    stderr: "piped",
  }).output();
  requireCondition(result.success, `cannot crop EMPTY aperture from ${path}`);
  return result.stdout;
}
for (const root of [nativeRoot, browserRoot]) {
  const paths = [];
  for (const suffix of ["zero", "half"]) {
    const id = `boundary-empty-opacity-${suffix}`;
    if (root === browserRoot) {
      paths.push(`${root}/${id}.png`);
    } else {
      const manifest = JSON.parse(
        await Deno.readTextFile(`${root}/${id}/manifest.json`),
      ) as NativeManifest;
      const image = manifest.operations.find((item) =>
        item.action === `capture-${id}`
      )?.image;
      requireCondition(image, `${id}: missing native opacity capture`);
      paths.push(`${root}/${id}/${image}`);
    }
  }
  const zero = await aperturePixels(paths[0]);
  const half = await aperturePixels(paths[1]);
  requireCondition(
    zero.length === half.length &&
      zero.some((value, index) => value !== half[index]),
    `EMPTY opacity paints identical aperture pixels in ${root}`,
  );
}
console.log(
  `one 4:3 browser review video; eight selection/score pairs, all common widgets, ${expectedBoundaries.length} boundary inputs with native/browser captures, controlled A/AA/AAA and ${
    cases[0].background_motion === "animated" ? "three" : "two"
  } background modes checked mechanically; painted zero/unknown, material and readability still require visual review`,
);
