import { expect } from "playwright/test";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { browserTest } from "../../../../tests/browser-test.js";
import { widgetRect, widgetText } from "./browser-widget-regions.js";

const baseURL = Deno.env.get("SCOREPEEK_SKIN_PREVIEW_URL");
const outputRoot = Deno.env.get("SCOREPEEK_SKIN_PREVIEW_OUTPUT");
const repositoryRoot = Deno.cwd();
const skinsDirectory = Deno.env.get("SCOREPEEK_SKINS_DIRECTORY") ??
  path.join(repositoryRoot, "skins");
const scene = JSON.parse(
  fs.readFileSync(
    Deno.env.get("SCOREPEEK_SKIN_PREVIEW_SCENE") ??
      path.join(
        repositoryRoot,
        ".agents/skills/create-overlay-skin/preview-scene.json",
      ),
    "utf8",
  ),
);

function manifestValue(slug, key) {
  const source = fs.readFileSync(
    path.join(skinsDirectory, slug, "skin.toml"),
    "utf8",
  );
  const match = source.match(new RegExp(`^${key}\\s*=\\s*"([^"]*)"`, "m"));
  if (!match) throw new Error(`${slug}/skin.toml is missing ${key}`);
  return match[1];
}

function sha256(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest(
    "hex",
  );
}

function run(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(
      `${command} failed (${result.status}):\n${result.stdout}\n${result.stderr}`,
    );
  }
  return result.stdout;
}

browserTest(
  "generate catalog previews from production skin rendering",
  async ({ browser }) => {
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
      await page.clock.install({ time: new Date(0) });
      await page.clock.pauseAt(new Date(0));
      await page.addInitScript(() => {
        const OriginalWorker = window.Worker;
        window.__skinPreviewWorker = { lastCompletedMs: null };
        window.Worker = class extends OriginalWorker {
          constructor(...args) {
            super(...args);
            this.sentTimes = new Map();
            this.addEventListener("message", (event) => {
              const time = this.sentTimes.get(event.data?.id);
              if (event.data?.output && time !== undefined) {
                window.__skinPreviewWorker.lastCompletedMs = time;
              }
            });
          }
          postMessage(message, ...args) {
            if (message?.input?.schema === "scorepeek-skin-input-v2") {
              this.sentTimes.set(message.id, message.input.monotonic_ms);
            }
            return super.postMessage(message, ...args);
          }
        };
      });
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
      let previewSocket;
      await page.routeWebSocket(`**/ws/${canvasId}`, (client) => {
        previewSocket = client;
        client.send(JSON.stringify({ type: "state", state }));
        const server = client.connectToServer();
        client.onMessage((message) => server.send(message));
        server.onMessage((message) => {
          const text = typeof message === "string"
            ? message
            : message.toString();
          try {
            if (JSON.parse(text).type === "state") return;
          } catch { /* Ignore non-JSON messages. */ }
          client.send(message);
        });
      });
      await page.goto(`${baseURL}/canvas/${canvasId}?editor=1`, {
        waitUntil: "networkidle",
      });
      for (const widget of scene.canvas.widgets) {
        await expect.poll(() => widgetRect(page, widget)).not.toBeNull();
      }
      const selection = scene.canvas.widgets.find((widget) =>
        widget.kind === "selection"
      );
      const score = scene.canvas.widgets.find((widget) =>
        widget.kind === "score"
      );
      for (const value of [state.chart.title, state.chart.artist]) {
        await expect.poll(() => widgetText(page, selection)).toContain(value);
      }
      for (const value of ["AAA", "FULL COMBO", "RANDOM"]) {
        await expect.poll(() => widgetText(page, score)).toContain(value);
      }
      await page.evaluate(async () => {
        await Promise.all([...document.images].map((image) => image.decode()));
      });
      const geometry = await Promise.all(
        scene.canvas.widgets.map(async (widget) => ({
          id: widget.id,
          ...await widgetRect(page, widget),
        })),
      );
      for (const [index, widget] of scene.canvas.widgets.entries()) {
        expect(geometry[index].id).toBe(widget.id);
        for (const key of ["x", "y", "width", "height"]) {
          expect(Math.abs(geometry[index][key] - widget[key]))
            .toBeLessThanOrEqual(1);
        }
      }
      const overflow = await page.evaluate(() => ({
        width: document.documentElement.scrollWidth,
        height: document.documentElement.scrollHeight,
      }));
      expect(overflow).toEqual({
        width: scene.canvas.width,
        height: scene.canvas.height,
      });
      await page.evaluate(() => {
        for (const animation of document.getAnimations()) animation.pause();
      });
      const setMotionTime = (time) =>
        page.evaluate((value) => {
          for (const animation of document.getAnimations()) {
            animation.currentTime = value;
          }
        }, time);

      const png = path.join(directory, "preview.png");
      const frameCount = scene.media.video_duration_ms * scene.media.video_fps /
        1000;
      expect(Number.isInteger(frameCount)).toBe(true);
      const pngFrame = scene.media.png_time_ms * scene.media.video_fps / 1000;
      expect(
        Number.isInteger(pngFrame) && pngFrame >= 0 && pngFrame < frameCount,
      )
        .toBe(true);
      let previousTime = 0;
      for (let frame = 0; frame < frameCount; frame += 1) {
        const time = Math.round(frame * 1000 / scene.media.video_fps);
        if (time > previousTime) await page.clock.runFor(time - previousTime);
        previousTime = time;
        await setMotionTime(time);
        const actualTime = await page.evaluate(() =>
          Math.round(performance.now())
        );
        expect(actualTime).toBeGreaterThanOrEqual(time);
        expect(actualTime).toBeLessThanOrEqual(time + 10);
        previewSocket.send(JSON.stringify({ type: "state", state }));
        await expect.poll(() =>
          page.evaluate(() => window.__skinPreviewWorker?.lastCompletedMs)
        ).toBe(actualTime);
        // A full-tree Wasm update may replace nodes and restart their CSS
        // animations. Seek the newly rendered tree before recording the frame.
        await page.evaluate((value) => {
          for (const animation of document.getAnimations()) {
            animation.pause();
            animation.currentTime = value;
          }
        }, time);
        if (frame === pngFrame) {
          await page.screenshot({
            path: png,
            clip: {
              x: 0,
              y: 0,
              width: scene.canvas.width,
              height: scene.canvas.height,
            },
          });
        }
        await page.screenshot({
          path: path.join(
            frameDirectory,
            `frame-${String(frame).padStart(3, "0")}.jpg`,
          ),
          type: "jpeg",
          quality: 100,
          clip: {
            x: 0,
            y: 0,
            width: scene.canvas.width,
            height: scene.canvas.height,
          },
        });
      }
      expect(
        await page.evaluate(() => document.documentElement.dataset.skinFailure),
      ).toBeUndefined();
      expect(
        await page.evaluate(() =>
          [...document.images].every((image) =>
            image.complete && image.naturalWidth > 0
          )
        ),
      ).toBe(true);
      await context.close();
      expect(pageErrors).toEqual([]);
      expect(failures).toEqual([]);

      const webm = path.join(directory, "preview.webm");
      run("ffmpeg", [
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-framerate",
        String(scene.media.video_fps),
        "-i",
        path.join(frameDirectory, "frame-%03d.jpg"),
        "-c:v",
        "libvpx-vp9",
        "-crf",
        "30",
        "-b:v",
        "0",
        "-threads",
        "1",
        "-row-mt",
        "0",
        "-fflags",
        "+bitexact",
        "-flags:v",
        "+bitexact",
        "-map_metadata",
        "-1",
        "-pix_fmt",
        "yuv420p",
        "-an",
        "-t",
        String(scene.media.video_duration_ms / 1000),
        webm,
      ]);
      const probe = JSON.parse(run("ffprobe", [
        "-v",
        "error",
        "-show_entries",
        "stream=codec_name,codec_type,width,height,r_frame_rate:format=duration",
        "-of",
        "json",
        webm,
      ]));
      const video = probe.streams.find(({ codec_type }) =>
        codec_type === "video"
      );
      expect(video).toMatchObject({
        codec_name: scene.media.video_codec,
        width: scene.canvas.width,
        height: scene.canvas.height,
        r_frame_rate: `${scene.media.video_fps}/1`,
      });
      expect(probe.streams.some(({ codec_type }) => codec_type === "audio"))
        .toBe(false);
      expect(Number(probe.format.duration)).toBeCloseTo(
        scene.media.video_duration_ms / 1000,
        2,
      );
      fs.rmSync(frameDirectory, { recursive: true });
      report.outputs.push({
        slug: skin.slug,
        skin_id: skin.id,
        title: state.chart.title,
        artist: state.chart.artist,
        geometry,
        resources_failed: failures,
        png: {
          path: `${skin.slug}/preview.png`,
          sha256: sha256(png),
          width: scene.canvas.width,
          height: scene.canvas.height,
        },
        webm: {
          path: `${skin.slug}/preview.webm`,
          sha256: sha256(webm),
          ...video,
          duration: Number(probe.format.duration),
          audio_streams: 0,
        },
      });
    }
    report.stage = "complete";
    fs.writeFileSync(
      path.join(outputRoot, "manifest.json"),
      `${JSON.stringify(report, null, 2)}\n`,
    );
  },
  { timeoutMs: 15 * 60_000, page: false },
);
