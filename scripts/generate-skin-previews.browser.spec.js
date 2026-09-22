const { test, expect } = require("playwright/test");
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const baseURL = process.env.SCOREPEEK_SKIN_PREVIEW_URL;
const outputRoot = process.env.SCOREPEEK_SKIN_PREVIEW_OUTPUT;
const repositoryRoot = process.cwd();
const scene = JSON.parse(fs.readFileSync(path.join(repositoryRoot, "skins/preview-scene.json"), "utf8"));

function manifestValue(slug, key) {
  const source = fs.readFileSync(path.join(repositoryRoot, "skins", slug, "skin.toml"), "utf8");
  const match = source.match(new RegExp(`^${key}\\s*=\\s*"([^"]*)"`, "m"));
  if (!match) throw new Error(`${slug}/skin.toml is missing ${key}`);
  return match[1];
}

function sha256(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function run(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`${command} failed (${result.status}):\n${result.stdout}\n${result.stderr}`);
  }
  return result.stdout;
}

test("generate catalog previews from production skin rendering", async ({ browser }) => {
  test.setTimeout(15 * 60_000);
  expect(baseURL).toBeTruthy();
  expect(outputRoot).toBeTruthy();
  const report = {
    schema: "scorepeek-skin-preview-generation-v1",
    scene: scene.schema,
    stage: "render",
    media: scene.media,
    outputs: [],
  };

  for (const skin of scene.skins) {
    const directory = path.join(outputRoot, skin.slug);
    const frameDirectory = path.join(directory, "frames");
    fs.mkdirSync(frameDirectory, { recursive: true });
    const failures = [];
    const pageErrors = [];
    const context = await browser.newContext({
      viewport: { width: scene.canvas.width, height: scene.canvas.height },
      deviceScaleFactor: 1,
      reducedMotion: "no-preference",
    });
    const page = await context.newPage();
    page.on("pageerror", (error) => pageErrors.push(error.message));
    page.on("response", (response) => {
      if (response.url().includes("/skin/") && !response.ok()) {
        failures.push(`${response.status()} ${response.url()}`);
      }
    });
    const state = structuredClone(scene.state);
    state.chart.title = manifestValue(skin.slug, "name");
    state.chart.artist = manifestValue(skin.slug, "author");
    const canvasId = `preview-${skin.slug}`;
    await page.routeWebSocket(`**/ws/${canvasId}`, (client) => {
      client.send(JSON.stringify({ type: "state", state }));
      const server = client.connectToServer();
      client.onMessage((message) => server.send(message));
      server.onMessage((message) => {
        const text = typeof message === "string" ? message : message.toString();
        try {
          if (JSON.parse(text).type === "state") return;
        } catch (_) {}
        client.send(message);
      });
    });
    await page.goto(`${baseURL}/canvas/${canvasId}?editor=1`, { waitUntil: "networkidle" });
    await expect(page.locator(".selection-widget")).toBeVisible();
    await expect(page.locator(".score-widget .detail-row.options")).toBeVisible();
    await expect(page.locator(".history-graph-widget .plot")).toBeVisible();
    await expect(page.locator("body")).toContainText(state.chart.title);
    await expect(page.locator("body")).toContainText(state.chart.artist);
    await expect(page.locator("body")).toContainText("AAA");
    await expect(page.locator("body")).toContainText("FULL COMBO");
    await expect(page.locator("body")).toContainText("RANDOM");
    await page.waitForFunction(() => [...document.images].every((image) => image.complete && image.naturalWidth > 0));
    const geometry = await page.locator(".widget-slot").evaluateAll((nodes) => nodes.map((node) => {
      const rect = node.getBoundingClientRect();
      return { id: node.dataset.widgetId, x: rect.x, y: rect.y, width: rect.width, height: rect.height };
    }));
    expect(geometry).toEqual(scene.canvas.widgets.map(({ kind: _, ...widget }) => widget));
    const overflow = await page.evaluate(() => ({
      width: document.documentElement.scrollWidth,
      height: document.documentElement.scrollHeight,
    }));
    expect(overflow).toEqual({ width: scene.canvas.width, height: scene.canvas.height });
    await page.evaluate(() => {
      for (const animation of document.getAnimations()) animation.pause();
    });
    const setMotionTime = (time) => page.evaluate((value) => {
      for (const animation of document.getAnimations()) animation.currentTime = value;
    }, time);

    await setMotionTime(scene.media.png_time_ms);
    const png = path.join(directory, "preview.png");
    await page.screenshot({ path: png, clip: { x: 0, y: 0, width: scene.canvas.width, height: scene.canvas.height } });
    const frameCount = scene.media.video_duration_ms * scene.media.video_fps / 1000;
    expect(Number.isInteger(frameCount)).toBe(true);
    for (let frame = 0; frame < frameCount; frame += 1) {
      await setMotionTime(frame * 1000 / scene.media.video_fps);
      await page.screenshot({
        path: path.join(frameDirectory, `frame-${String(frame).padStart(3, "0")}.jpg`),
        type: "jpeg",
        quality: 100,
        clip: { x: 0, y: 0, width: scene.canvas.width, height: scene.canvas.height },
      });
    }
    await context.close();
    expect(pageErrors).toEqual([]);
    expect(failures).toEqual([]);

    const webm = path.join(directory, "preview.webm");
    run("ffmpeg", [
      "-hide_banner", "-loglevel", "error", "-y",
      "-framerate", String(scene.media.video_fps),
      "-i", path.join(frameDirectory, "frame-%03d.jpg"),
      "-c:v", "libvpx-vp9", "-crf", "30", "-b:v", "0",
      "-threads", "1", "-row-mt", "0", "-fflags", "+bitexact", "-flags:v", "+bitexact",
      "-map_metadata", "-1", "-pix_fmt", "yuv420p", "-an",
      "-t", String(scene.media.video_duration_ms / 1000),
      webm,
    ]);
    const probe = JSON.parse(run("ffprobe", [
      "-v", "error", "-show_entries", "stream=codec_name,codec_type,width,height,r_frame_rate:format=duration",
      "-of", "json", webm,
    ]));
    const video = probe.streams.find(({ codec_type }) => codec_type === "video");
    expect(video).toMatchObject({
      codec_name: scene.media.video_codec,
      width: scene.canvas.width,
      height: scene.canvas.height,
      r_frame_rate: `${scene.media.video_fps}/1`,
    });
    expect(probe.streams.some(({ codec_type }) => codec_type === "audio")).toBe(false);
    expect(Number(probe.format.duration)).toBeCloseTo(scene.media.video_duration_ms / 1000, 2);
    fs.rmSync(frameDirectory, { recursive: true });
    report.outputs.push({
      slug: skin.slug,
      skin_id: skin.id,
      title: state.chart.title,
      artist: state.chart.artist,
      geometry,
      resources_failed: failures,
      png: { path: `${skin.slug}/preview.png`, sha256: sha256(png), width: scene.canvas.width, height: scene.canvas.height },
      webm: { path: `${skin.slug}/preview.webm`, sha256: sha256(webm), ...video, duration: Number(probe.format.duration), audio_streams: 0 },
    });
  }
  report.stage = "complete";
  fs.writeFileSync(path.join(outputRoot, "manifest.json"), `${JSON.stringify(report, null, 2)}\n`);
});
