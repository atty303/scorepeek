const fs = require("node:fs");

const [scenePath, outputPath] = process.argv.slice(2);
if (!scenePath || !outputPath) {
  throw new Error("usage: node scripts/write-skin-preview-config.js SCENE.json OUTPUT.toml");
}
const scene = JSON.parse(fs.readFileSync(scenePath, "utf8"));
if (scene.schema !== "scorepeek-skin-preview-scene-v1") {
  throw new Error(`unsupported preview scene schema: ${scene.schema}`);
}
const quote = (value) => JSON.stringify(value);
const lines = [
  "schema_version = 9",
  "unknown_grace_ms = 1000",
  'obs_listen = "127.0.0.1:3939"',
];
for (const skin of scene.skins) {
  const canvasId = `preview-${skin.slug}`;
  lines.push(
    "",
    "[[canvases]]",
    `id = ${quote(canvasId)}`,
    `name = ${quote(skin.slug)}`,
    'backend = "obs"',
    `skin = ${quote(skin.id)}`,
    "opacity_percent = 100",
    'output = "obs-output"',
    "x = 0",
    "y = 0",
    `width = ${scene.canvas.width}`,
    `height = ${scene.canvas.height}`,
    "",
    "[canvases.skin_properties]",
    `background = ${quote(scene.canvas.background)}`,
  );
  for (const widget of scene.canvas.widgets) {
    lines.push(
      "",
      "[[canvases.widgets]]",
      `id = ${quote(widget.id)}`,
      `kind = ${quote(widget.kind)}`,
      `x = ${widget.x}`,
      `y = ${widget.y}`,
      `width = ${widget.width}`,
      `height = ${widget.height}`,
      "",
      "[canvases.widgets.skin_properties]",
      'frame-width = "m"',
      "fill-opacity-percent = 0",
      "",
      "[canvases.widgets.settings]",
      "history_count = 5",
      "graph_months = 6",
    );
  }
}
fs.writeFileSync(outputPath, `${lines.join("\n")}\n`, { flag: "wx" });
