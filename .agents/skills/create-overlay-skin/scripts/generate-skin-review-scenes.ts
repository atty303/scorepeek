// Generate synthetic, all-widget native review scenarios for any installed skin.
// Usage: deno run --allow-write scripts/generate-skin-review-scenes.ts <skin-id> <new-output-dir> <motion-periods-ms> <paint-padding-px> <no-background-value> <animated|still>

const [
  skinId,
  outputDir,
  periodsArgument,
  paddingArgument,
  backgroundOffValue,
  backgroundMotion,
] = Deno.args;
if (!skinId || !outputDir || Deno.args.length !== 6) {
  throw new Error(
    "usage: generate-skin-review-scenes.ts <skin-id> <new-output-dir> <motion-periods-ms> <paint-padding-px> <none|off> <animated|still>",
  );
}
if (backgroundOffValue !== "none" && backgroundOffValue !== "off") {
  throw new Error(
    "no-background value must be none or off, as declared by the skin manifest",
  );
}
if (backgroundMotion !== "animated" && backgroundMotion !== "still") {
  throw new Error(
    "background motion must be animated or still, as declared by the concept",
  );
}
const paintPadding = Number(paddingArgument);
if (!Number.isSafeInteger(paintPadding) || paintPadding < 0) {
  throw new Error("paint padding must be a nonnegative integer in pixels");
}
const motionPeriodsMs = periodsArgument === ""
  ? []
  : periodsArgument.split(",").map(Number);
if (
  motionPeriodsMs.some((period) => !Number.isSafeInteger(period) || period <= 0)
) {
  throw new Error("motion periods must be positive integer milliseconds");
}
if (backgroundMotion === "animated" && motionPeriodsMs.length === 0) {
  throw new Error("animated background requires its declared motion period");
}
const reviewFps = 15;
const reviewDurationMs = Math.ceil(
  Math.max(8000, ...motionPeriodsMs.map((period) => period + 1000)) / 1000,
) * 1000;
const nativeMotionMs = [
  ...new Set([
    250,
    1000,
    2000,
    4000,
    reviewDurationMs - Math.round(1000 / reviewFps),
    reviewDurationMs,
    ...motionPeriodsMs.flatMap((
      period,
    ) => [
      Math.round(period / 4),
      Math.round(period / 2),
      Math.round(3 * period / 4),
      period - Math.round(1000 / reviewFps),
      period,
      period + Math.round(1000 / reviewFps),
    ]),
  ].filter((value) => value > 0)),
].sort((a, b) => a - b);
const nativeMotionSeconds = nativeMotionMs.map((value) =>
  Number((value / 1000).toFixed(3))
);

const cases = [
  {
    play: "single",
    difficulty: "beginner",
    rank: "A",
    clear: "NO PLAY",
    level: 1,
  },
  {
    play: "double",
    difficulty: "normal",
    rank: "AAA",
    clear: "FAILED",
    level: 4,
  },
  {
    play: "single",
    difficulty: "hyper",
    rank: "AA",
    clear: "ASSIST",
    level: 7,
  },
  {
    play: "double",
    difficulty: "another",
    rank: "AAA",
    clear: "EASY",
    level: 10,
  },
  {
    play: "single",
    difficulty: "leggendaria",
    rank: "AA",
    clear: "CLEAR",
    level: 12,
  },
  {
    play: "double",
    difficulty: "beginner",
    rank: "AAA",
    clear: "HARD",
    level: 3,
  },
  {
    play: "single",
    difficulty: "normal",
    rank: "AAA",
    clear: "EX HARD",
    level: 6,
  },
  {
    play: "double",
    difficulty: "hyper",
    rank: "AA",
    clear: "FULL COMBO",
    level: 9,
  },
] as const;

const commonWidgets = [
  { id: "status", kind: "status", x: 40, y: 18, width: 1840, height: 52 },
  {
    id: "history",
    kind: "history-list",
    x: 40,
    y: 1050,
    width: 600,
    height: 200,
  },
  {
    id: "graph",
    kind: "history-graph",
    x: 660,
    y: 1050,
    width: 600,
    height: 200,
  },
  {
    id: "aperture",
    kind: "empty",
    x: 1280,
    y: 1050,
    width: 300,
    height: 80,
    settings: { title: "HAND CAM" },
  },
];

const ranks = ["F", "F", "E", "D", "C", "B", "A", "AA", "AAA"];
function djLevel(score: number, notes: number): string {
  return ranks[Math.min(8, Math.floor(9 * score / (2 * notes)))];
}
function lowerThreshold(rank: string, notes: number): number {
  const index = ranks.lastIndexOf(rank);
  return Math.ceil(2 * notes * index / 9);
}

await Deno.mkdir(outputDir);
const summary = [];
for (const [index, variant] of cases.entries()) {
  const id = String(index + 1).padStart(2, "0");
  const column = index % 4;
  const row = Math.floor(index / 4);
  const x = 40 + column * 460;
  const widgets = [
    ...commonWidgets,
    {
      id: "selection",
      kind: "selection",
      x,
      y: 82 + row * 168,
      width: 420,
      height: 126,
    },
    {
      id: "score",
      kind: "score",
      x,
      y: 430 + row * 260,
      width: 420,
      height: 194,
    },
  ];
  const notes = [430, 1180, 520, 1411, 1537, 600, 905, 1782][index];
  const score = index === 5
    ? 2 * notes - 8
    : lowerThreshold(variant.rank, notes) + 12 + index * 13;
  if (djLevel(score, notes) !== variant.rank) {
    throw new Error(`${id}: score/rank mismatch`);
  }
  const graphStart = 1772323200000;
  const graphEnd = 1788134400000;
  const historyScores = Array.from(
    { length: 5 },
    (_, row) => score - row * (37 + index),
  );
  const state = {
    connected: true,
    chart: {
      song_id: `synthetic-review-${id}`,
      play_type: variant.play,
      difficulty: variant.difficulty,
      level: variant.level,
      notes,
      title: index === 7
        ? `検証用の長い和英混在タイトル SONG ${id} EXTENDED MIX`
        : `検証用 SONG ${id}`,
      artist: index === 7
        ? `Example Artist / 架空アーティスト ${id}`
        : `Example ARTIST ${id}`,
    },
    system: index === 0 || index === 6 ? "error" : "active",
    result_signal: index === 0 || index % 2 ? "inactive" : "active",
    best: {
      score: String(score),
      dj_level: variant.rank,
      miss: variant.clear === "FULL COMBO" ? "0" : String(2 + index * 4),
      clear: variant.clear,
    },
    detail: {
      pgreat: String(500 + index * 43),
      great: String(160 + index * 11),
      good: String(12 + index * 3),
      bad: String(2 + index * 2),
      poor: String(3 + index * 2),
      fast: String(80 + index * 17),
      slow: String(93 + index * 19),
      combo_break: variant.clear === "FULL COMBO" ? "0" : String(1 + index * 3),
      play_options: [
        "RANDOM",
        "MIRROR",
        "S-RANDOM",
        "FLIP",
        "R-RANDOM",
        "OFF",
        "RANDOM + FLIP",
        "MIRROR + FLIP",
      ][index],
    },
    history: {
      recorded: true,
      plays: historyScores.map((rowScore, row) => ({
        notified_at: `2026-09-${String(25 - row - index).padStart(2, "0")} 20:${
          String(row * 7).padStart(2, "0")
        }:00`,
        score: String(rowScore),
        dj_level: djLevel(rowScore, notes),
        miss: row === 0 && variant.clear === "FULL COMBO"
          ? "0"
          : String(3 + index * 4 + row * 5),
        clear: [variant.clear, "HARD", "CLEAR", "EASY", "FAILED"][row],
      })),
      graph_start_unix_ms: [
        1785542400000,
        1780358400000,
        graphStart,
        1756598400000,
      ],
      graph_end_unix_ms: graphEnd,
      graph_ticks: ["MAR", "APR", "MAY", "JUN", "JUL", "AUG"].map((
        label,
        tick,
      ) => ({
        unix_ms: graphStart + tick * 2635200000,
        label,
      })),
      graph: Array.from({ length: 18 }, (_, point) => ({
        received_unix_ms: graphStart + point * 864000000,
        score_ratio: Math.min(
          1,
          [
            0.23,
            0.29,
            0.42,
            0.36,
            0.49,
            0.62,
            0.57,
            0.68,
            0.72,
            0.64,
            0.78,
            0.86,
            0.74,
            0.92,
            0.85,
            0.95,
            0.88,
            0.96,
          ][point] + index * 0.004,
        ),
        miss_ratio: point === 7 ? null : Math.max(
          0,
          [
            0.75,
            0.61,
            0.72,
            0.58,
            0.64,
            0.48,
            0.54,
            0.46,
            0.38,
            0.45,
            0.35,
            0.29,
            0.33,
            0.2,
            0.26,
            0.14,
            0.19,
            0.1,
          ][point] - index * 0.006,
        ),
      })),
    },
    screen: {
      kind: "music-select",
      suspended_since_unix_ms: null,
      revision: index,
    },
  };
  const scene = {
    skin: skinId,
    monotonic_base_ms: 0,
    logical_size: [1920, 1440],
    scale: 1,
    canvases: [{
      id: `review-${id}`,
      name: `Review ${id}`,
      skin: skinId,
      x: 0,
      y: 0,
      width: 1920,
      height: 1440,
      widgets,
    }],
    editing: false,
    selectors: ["*"],
    actions: [
      { action: "set_state", state },
      { action: "capture", name: `review-${id}` },
      ...nativeMotionSeconds.flatMap((seconds) => [
        { action: "advance_animation", seconds },
        { action: "capture", name: `review-${id}-at-${seconds}` },
      ]),
    ],
  };
  await Deno.writeTextFile(
    `${outputDir}/${id}.json`,
    `${JSON.stringify(scene, null, 2)}\n`,
  );
  if (index === 0) {
    // Internal canvas comparison. Keep every widget and its content fixed so
    // the surface and the information-bearing panels can be read separately.
    for (
      const background of [
        "off",
        "static",
        ...(backgroundMotion === "animated" ? ["animated"] : []),
      ]
    ) {
      const comparisonId = `background-${background}`;
      const comparisonScene = {
        ...scene,
        canvases: [{
          ...scene.canvases[0],
          id: comparisonId,
          skin_properties: {
            background: background === "off" ? backgroundOffValue : background,
          },
        }],
        actions: [
          { action: "set_state", state },
          { action: "capture", name: comparisonId },
          ...nativeMotionSeconds.flatMap((seconds) => [
            { action: "advance_animation", seconds },
            { action: "capture", name: `${comparisonId}-at-${seconds}` },
          ]),
        ],
      };
      await Deno.writeTextFile(
        `${outputDir}/${comparisonId}.json`,
        `${JSON.stringify(comparisonScene, null, 2)}\n`,
      );
    }
    // Internal meaning comparison. Keep chart, clear, judgments, graph and
    // widget dimensions fixed; change only the rank-consistent best score and
    // the first History row's matching score/rank.
    for (const rank of ["A", "AA", "AAA"] as const) {
      const comparisonId = `rank-${rank.toLowerCase()}`;
      const comparisonState = structuredClone(state);
      const comparisonScore = lowerThreshold(rank, notes) + 12;
      if (djLevel(comparisonScore, notes) !== rank) {
        throw new Error(`${comparisonId}: score/rank mismatch`);
      }
      comparisonState.best.score = String(comparisonScore);
      comparisonState.best.dj_level = rank;
      comparisonState.best.clear = "CLEAR";
      comparisonState.best.miss = "7";
      comparisonState.history.plays[0].score = String(comparisonScore);
      comparisonState.history.plays[0].dj_level = rank;
      comparisonState.history.plays[0].clear = "CLEAR";
      comparisonState.history.plays[0].miss = "7";
      const comparisonScene = {
        ...scene,
        canvases: [{ ...scene.canvases[0], id: comparisonId }],
        actions: [
          { action: "set_state", state: comparisonState },
          { action: "capture", name: comparisonId },
        ],
      };
      await Deno.writeTextFile(
        `${outputDir}/${comparisonId}.json`,
        `${JSON.stringify(comparisonScene, null, 2)}\n`,
      );
    }
  }
  summary.push({
    id,
    skinId,
    play: variant.play,
    difficulty: variant.difficulty,
    rank: variant.rank,
    clear: variant.clear,
    level: variant.level,
    notes,
    score,
    bestMiss: state.best.miss,
    scenario: `${id}.json`,
    background_motion: backgroundMotion,
    paint_padding: paintPadding,
    media: {
      fps: reviewFps,
      duration_ms: reviewDurationMs,
      motion_periods_ms: motionPeriodsMs,
      native_motion_seconds: nativeMotionSeconds,
    },
  });
}
await Deno.writeTextFile(
  `${outputDir}/cases.json`,
  `${JSON.stringify(summary, null, 2)}\n`,
);
