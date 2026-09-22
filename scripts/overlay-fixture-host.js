#!/usr/bin/env node
"use strict";

const fs = require("node:fs");
const net = require("node:net");
const path = require("node:path");
const { spawn } = require("node:child_process");

const [root, address, sceneArgument, configArgument, backendArgument] = process.argv.slice(2);
const scenePath = sceneArgument === "-" ? null : sceneArgument;
const backend = backendArgument || "obs";
if (!root || !address || (scenePath && !path.isAbsolute(scenePath)) || (configArgument && !path.isAbsolute(configArgument))) {
  throw new Error("usage: overlay-fixture-host.js ROOT 127.0.0.1:PORT [ABSOLUTE_SCENE.json|-] [ABSOLUTE_CONFIG.toml] [obs|wayland]");
}
if (!["obs", "wayland"].includes(backend) || (scenePath && backend !== "obs")) throw new Error("invalid fixture backend");
if (!/^127\.0\.0\.1:\d+$/.test(address)) throw new Error("fixture listener must be loopback");
const dataHome = process.env.XDG_DATA_HOME;
if (!dataHome || !process.env.XDG_CONFIG_HOME || !process.env.XDG_STATE_HOME || !process.env.XDG_CACHE_HOME || !process.env.XDG_RUNTIME_DIR || !process.env.HOME) {
  throw new Error("fixture requires isolated HOME and every XDG base directory");
}
const scene = scenePath ? JSON.parse(fs.readFileSync(scenePath, "utf8")) : null;
if (scene && scene.schema !== "scorepeek-skin-preview-scene-v1") throw new Error("unsupported preview scene");
const skinStore = path.join(dataHome, "scorepeek", "skins");
const configPath = configArgument || path.join(root, "overlay.toml");
const controlSocket = path.join(root, "control.sock");
const initialCanvases = backend === "wayland" ? ["HEADLESS-1", "HEADLESS-2"].map((output, index) => ({
  id: `wayland-${index + 1}`, name: `Nested output ${index + 1}`, backend: "wayland",
  skin: "dev.atty303.scorepeek.skin.cyan-system", opacity_percent: 100,
  output, x: 20, y: 20, width: 560, height: 72,
  widgets: [{ id: `status-${index + 1}`, kind: "status", x: 8, y: 8, width: 544, height: 56 }],
})) : scene ? scene.skins.map((skin) => ({
  id: `preview-${skin.slug}`, name: skin.slug, backend: "obs", skin: skin.id,
  skin_properties: { background: scene.canvas.background }, opacity_percent: 100,
  output: "obs-output", x: 0, y: 0,
  width: scene.canvas.width, height: scene.canvas.height,
  widgets: scene.canvas.widgets.map((widget) => ({
    ...widget,
    skin_properties: { "frame-width": "m", "fill-opacity-percent": 0 },
    settings: { history_count: 5, graph_months: 6 },
  })),
})) : [];
const saved = initialCanvases.map(({ backend, ...canvas }) => ({ ...canvas, output: canvas.output }));

function tomlValue(value) {
  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(tomlValue).join(", ")}]`;
  throw new Error("unsupported fixture TOML value");
}
function tomlDocument(canvases) {
  const lines = ["schema_version = 9", "unknown_grace_ms = 1000", `obs_listen = ${tomlValue(address)}`];
  for (const canvas of canvases) {
    lines.push("", "[[canvases]]");
    for (const key of ["id", "name", "skin", "opacity_percent", "output", "x", "y", "width", "height", "show_on"]) {
      if (canvas[key] != null) lines.push(`${key} = ${tomlValue(canvas[key])}`);
    }
    lines.push(`backend = ${tomlValue(backend)}`);
    if (canvas.skin_properties && Object.keys(canvas.skin_properties).length) {
      lines.push("", "[canvases.skin_properties]");
      for (const [key, value] of Object.entries(canvas.skin_properties)) lines.push(`${tomlValue(key)} = ${tomlValue(value)}`);
    }
    for (const widget of canvas.widgets || []) {
      lines.push("", "[[canvases.widgets]]");
      for (const key of ["id", "kind", "x", "y", "width", "height"]) lines.push(`${key} = ${tomlValue(widget[key])}`);
      for (const key of ["skin_properties", "settings"]) {
        if (widget[key] && Object.keys(widget[key]).length) {
          lines.push("", `[canvases.widgets.${key}]`);
          for (const [property, value] of Object.entries(widget[key])) lines.push(`${tomlValue(property)} = ${tomlValue(value)}`);
        }
      }
    }
  }
  return `${lines.join("\n")}\n`;
}
fs.writeFileSync(configPath, tomlDocument(saved), { flag: "wx", mode: 0o600 });
let active = saved;
let lease = null;
let generation = 0;
let child;
let stopping = false;
let feedServer;
const eventSocket = path.join(root, "events.sock");

function response(readonly, canvases = active) {
  return { ok: true, readonly, error: null, canvases, generation: lease ? generation : null,
    dirty: Boolean(lease && JSON.stringify(active) !== JSON.stringify(saved)) };
}
function control(request) {
  if (request.backend !== backend) return { ...response(true), ok: false, error: "unsupported backend" };
  switch (request.command) {
    case "acquire_backend":
      if (lease && lease !== request.editor_id) return { ...response(true), ok: false, error: "workspace editor is already active" };
      if (!lease) { lease = request.editor_id; active = structuredClone(saved); generation += 1; }
      return response(false);
    case "keep_alive_backend":
      return response(lease !== request.editor_id);
    case "release_backend":
      if (lease === request.editor_id) { lease = null; active = structuredClone(saved); }
      return response(false, saved);
    case "get_backend":
      return response(true, saved);
    case "update_backend_draft":
    case "commit_backend":
      if (lease !== request.editor_id) return { ...response(true), ok: false, error: "editor lease lost" };
      active = request.canvases;
      generation += 1;
      if (request.command === "commit_backend") {
        saved.splice(0, saved.length, ...structuredClone(active));
        fs.writeFileSync(configPath, tomlDocument(saved), { mode: 0o600 });
      }
      return response(false);
    default:
      return { ...response(true), ok: false, error: "unsupported control command" };
  }
}
const controller = net.createServer((socket) => {
  let input = "";
  socket.setTimeout(2000, () => socket.destroy());
  socket.on("data", (bytes) => {
    input += bytes.toString("utf8");
    if (input.length > 1024 * 1024) { socket.destroy(); return; }
    const end = input.indexOf("\n");
    if (end < 0) return;
    let result;
    try { result = control(JSON.parse(input.slice(0, end))); }
    catch (error) { result = { ...response(true), ok: false, error: String(error) }; }
    socket.end(JSON.stringify(result) + "\n");
  });
});

function stop() {
  if (stopping) return;
  stopping = true;
  controller.close();
  if (feedServer) feedServer.close();
  if (!child || child.exitCode !== null) return;
  child.stdin.end();
  const deadline = setTimeout(() => child.kill("SIGKILL"), 3000);
  child.once("exit", () => clearTimeout(deadline));
}
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
process.stdin.resume();
process.stdin.on("end", stop);
controller.listen(controlSocket, () => {
  if (backend === "wayland") {
    feedServer = net.createServer((socket) => {
      socket.write(JSON.stringify({
        schema: "scorepeek-event-snapshot-v4", invocation_id: "nested-wayland-fixture",
        next_sequence: 1,
        status: { watcher: "session_active", capture: null, catalog: "ready", model: "ready",
          scores: null, recording: null, last_session_outcome: null },
        result: { schema: "scorepeek-event-v4", invocation_id: "nested-wayland-fixture",
          sequence: 0, event_id: "nested-wayland-fixture:0", emitted_monotonic_ms: 0,
          emitted_unix_ms: 1000, capture: null, event: "result_changed", source_sequence: 0,
          state: { status: "inactive" } },
        screen_state: null, music_selection: null, music_select_best: null,
      }) + "\n");
    });
    feedServer.listen(eventSocket);
  }
  const config = {
    backend, canvases: initialCanvases, config_path: configPath,
    control_socket: controlSocket, skin_store: skinStore,
    socket: backend === "wayland" ? eventSocket : path.join(root, "absent-events.sock"), invocation: "overlay-fixture",
    scores_db: null, listen: address, unknown_grace_ms: 1000, edit_on_start: backend === "wayland",
  };
  child = spawn(path.resolve("target/debug/scorepeek"), [backend === "obs" ? "__scorepeek-overlay-web-host" : "__scorepeek-overlay-wayland"], {
    stdio: ["pipe", fs.openSync(path.join(root, "role.stdout"), "w"), fs.openSync(path.join(root, "role.stderr"), "w")],
    env: process.env,
  });
  child.on("error", (error) => { process.stderr.write(`${error}\n`); process.exitCode = 1; stop(); });
  child.on("exit", (code, signal) => {
    if (!stopping) { process.stderr.write(`overlay role exited: ${code ?? signal}\n`); process.exitCode = 1; }
    controller.close();
    process.stdin.pause();
  });
  child.stdin.write(JSON.stringify(config) + "\n");
});
