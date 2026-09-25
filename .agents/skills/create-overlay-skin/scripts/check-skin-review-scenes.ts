// Check scene coverage, changing numeric data, and completed native captures.
// Usage: deno run --allow-read scripts/check-skin-review-scenes.ts <scene-dir> <native-output-root> <review-image>

const [sceneDir, nativeRoot, reviewImage] = Deno.args;
if (!sceneDir || !nativeRoot || !reviewImage || Deno.args.length !== 3) {
  throw new Error(
    "usage: check-skin-review-scenes.ts <scene-dir> <native-output-root> <review-image>",
  );
}

type ReviewCase = {
  id: string;
  play: string;
  difficulty: string;
  rank: string;
  clear: string;
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
    plays: { score: string }[];
    graph: { score_ratio: number; miss_ratio: number | null }[];
  };
};
type ReviewScene = {
  logical_size: number[];
  canvases: { widgets: { kind: string }[] }[];
  actions: { action: string; state?: ReviewState }[];
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
type NativeLayout = { elements: { matches: number[][] }[] };

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
  requireCondition(layout.elements?.length > 0, `${id}: empty selector layout`);
  requireCondition(
    layout.elements.every((element) =>
      element.matches?.every((rect: number[]) => rect[2] > 0 && rect[3] > 0)
    ),
    `${id}: invalid native rectangle`,
  );
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
const file = await Deno.open(reviewImage);
const header = new Uint8Array(24);
try {
  requireCondition(
    await file.read(header) === 24,
    "incomplete review image header",
  );
} finally {
  file.close();
}
const pngMagic = [137, 80, 78, 71, 13, 10, 26, 10];
requireCondition(
  pngMagic.every((byte, index) => header[index] === byte),
  "review image is not PNG",
);
const view = new DataView(header.buffer);
requireCondition(
  view.getUint32(16) === 1920 && view.getUint32(20) === 1440,
  "review image is not one 1920x1440 board",
);
const assembly = JSON.parse(await Deno.readTextFile(`${reviewImage}.json`)) as {
  source: string;
  placements: { caseId: string; kind: string }[];
};
requireCondition(
  assembly.source === "native captures only",
  "non-native review source",
);
requireCondition(
  assembly.placements.length === 16,
  "missing review placements",
);
for (const { id } of cases) {
  exactly(
    assembly.placements.filter((item) => item.caseId === id).map((item) =>
      item.kind
    ),
    ["selection", "score"],
    `${id}: review placements`,
  );
}
console.log(
  "one 4:3 native review image; eight selection/score pairs and all common widgets verified",
);
