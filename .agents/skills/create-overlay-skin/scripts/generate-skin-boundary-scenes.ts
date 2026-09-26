// Add static, all-widget boundary scenes to a generated review-scene directory.
// Usage: deno run --allow-read --allow-write generate-skin-boundary-scenes.ts SCENE_DIR EMPTY_OPACITY_PROPERTY

const [directory, opacityProperty] = Deno.args;
if (!directory || !opacityProperty || Deno.args.length !== 2) {
  throw new Error(
    "usage: generate-skin-boundary-scenes.ts SCENE_DIR EMPTY_OPACITY_PROPERTY",
  );
}
const source = JSON.parse(await Deno.readTextFile(`${directory}/01.json`));
const backgroundOffScene = JSON.parse(
  await Deno.readTextFile(`${directory}/background-off.json`),
);
const backgroundOff = backgroundOffScene.canvases?.[0]?.skin_properties
  ?.background;
if (backgroundOff !== "off" && backgroundOff !== "none") {
  throw new Error("missing generated background-off scene");
}
const baseState = source.actions.find((action: { action: string }) =>
  action.action === "set_state"
)?.state;
if (!baseState || !source.canvases?.[0]?.widgets) {
  throw new Error("missing complete 01 review scene");
}

type Scene = typeof source;
type State = typeof baseState;
const boundaryIds: string[] = [];
async function write(
  id: string,
  change: (scene: Scene, state: State) => void,
): Promise<void> {
  const scene = structuredClone(source);
  const state = structuredClone(baseState);
  change(scene, state);
  scene.canvases[0].id = id;
  scene.canvases[0].skin_properties = {
    ...scene.canvases[0].skin_properties,
    background: backgroundOff,
  };
  scene.actions = [
    { action: "set_state", state },
    { action: "capture", name: id },
  ];
  await Deno.writeTextFile(
    `${directory}/${id}.json`,
    `${JSON.stringify(scene, null, 2)}\n`,
  );
  boundaryIds.push(id);
}
function widget(scene: Scene, kind: string) {
  const value = scene.canvases[0].widgets.find((item: { kind: string }) =>
    item.kind === kind
  );
  if (!value) throw new Error(`missing ${kind} widget`);
  return value;
}
function rank(score: number, notes: number): string {
  return ["F", "F", "E", "D", "C", "B", "A", "AA", "AAA"][
    Math.min(8, Math.floor(9 * score / (2 * notes)))
  ];
}

for (const count of [5, 10, 20, 50]) {
  await write(`boundary-history-${count}`, (scene, state) => {
    const history = widget(scene, "history-list");
    history.x = 40;
    history.y = 92;
    history.width = 600;
    history.height = Math.max(200, 62 + count * 24);
    history.settings = { ...history.settings, history_count: count };
    const selection = widget(scene, "selection");
    selection.x = 680;
    selection.y = 100;
    const scoreWidget = widget(scene, "score");
    scoreWidget.x = 680;
    scoreWidget.y = 280;
    const graph = widget(scene, "history-graph");
    graph.x = 1120;
    graph.y = 100;
    const empty = widget(scene, "empty");
    empty.x = 1120;
    empty.y = 400;
    const score = Number(state.best.score);
    const notes = Number(state.chart.notes);
    state.history.plays = Array.from({ length: 50 }, (_, index) => {
      const day = new Date(Date.UTC(2026, 8, 25) - index * 86400000)
        .toISOString().slice(0, 10);
      const rowScore = Math.max(0, score - index * 10);
      return {
        notified_at: `${day} 20:00:00`,
        score: String(rowScore),
        dj_level: rank(rowScore, notes),
        miss: String(index),
        clear: ["CLEAR", "HARD", "EASY", "FAILED"][index % 4],
      };
    });
  });
}
for (const months of [1, 3, 6, 12]) {
  await write(`boundary-graph-${months}`, (scene) => {
    const graph = widget(scene, "history-graph");
    graph.settings = { ...graph.settings, graph_months: months };
    graph.width = 600;
    graph.height = 240;
  });
}
for (
  const [suffix, width, height, title, opacity] of [
    ["wide", 500, 80, "HAND CAM", 0],
    ["tall", 180, 300, "HAND CAM", 0],
    ["titleless", 300, 80, "", 0],
    ["opacity-zero", 300, 80, "HAND CAM", 0],
    ["opacity-half", 300, 80, "HAND CAM", 0.5],
  ] as const
) {
  await write(`boundary-empty-${suffix}`, (scene) => {
    const empty = widget(scene, "empty");
    empty.width = width;
    empty.height = height;
    empty.settings = { ...empty.settings, title };
    empty.skin_properties = {
      ...empty.skin_properties,
      [opacityProperty]: opacity,
    };
  });
}
for (const mode of ["zero", "unknown", "invalid-notes"] as const) {
  await write(`boundary-score-${mode}`, (_scene, state) => {
    if (mode === "zero") {
      state.best.score = "0";
      state.best.dj_level = "F";
    } else if (mode === "unknown") {
      state.best.score = "";
      state.best.dj_level = "";
    } else {
      state.chart.notes = 0;
    }
  });
}
await Deno.writeTextFile(
  `${directory}/boundary-cases.json`,
  `${JSON.stringify({ opacityProperty, ids: boundaryIds }, null, 2)}\n`,
);
