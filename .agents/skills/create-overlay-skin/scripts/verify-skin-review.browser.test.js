import { expect } from "playwright/test";
import fs from "node:fs";
import path from "node:path";
import { browserTest } from "../../../../tests/browser-test.js";
import {
  backdropDigest,
  captureCanvasBackdrop,
  captureWidgetOnly,
  widgetRect,
  widgetRoot,
  widgetText,
} from "./browser-widget-regions.js";

const scenePath = Deno.env.get("SCOREPEEK_SKIN_REVIEW_SCENE");
const baseURL = Deno.env.get("SCOREPEEK_SKIN_REVIEW_URL");
const outputPath = Deno.env.get("SCOREPEEK_SKIN_REVIEW_SCREENSHOT");

browserTest(
  "verify all review widgets in the production browser",
  async ({ browser }) => {
    expect(scenePath).toBeTruthy();
    expect(baseURL).toBeTruthy();
    expect(outputPath).toBeTruthy();
    const scene = JSON.parse(fs.readFileSync(scenePath, "utf8"));
    const context = await browser.newContext({
      viewport: { width: scene.canvas.width, height: scene.canvas.height },
      deviceScaleFactor: 1,
    });
    const page = await context.newPage();
    await page.clock.install({ time: new Date(0) });
    await page.clock.pauseAt(new Date(0));
    await page.addInitScript(() => {
      const OriginalWorker = window.Worker;
      window.__skinReviewWorker = { lastCompletedMs: null };
      window.Worker = class extends OriginalWorker {
        constructor(...args) {
          super(...args);
          this.sentTimes = new Map();
          this.addEventListener("message", (event) => {
            const time = this.sentTimes.get(event.data?.id);
            if (event.data?.output && time !== undefined) {
              window.__skinReviewWorker.lastCompletedMs = time;
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
    const failures = [];
    page.on("pageerror", (error) => failures.push(error.message));
    page.on("response", (response) => {
      if (response.url().includes("/skin/") && !response.ok()) {
        failures.push(`${response.status()} ${response.url()}`);
      }
    });
    const slug = scene.skins[0].slug;
    let reviewSocket;
    await page.routeWebSocket(`**/ws/preview-${slug}`, (client) => {
      reviewSocket = client;
      client.send(JSON.stringify({ type: "state", state: scene.state }));
      const server = client.connectToServer();
      client.onMessage((message) => server.send(message));
      server.onMessage((message) => {
        const value = typeof message === "string"
          ? message
          : message.toString();
        try {
          if (JSON.parse(value).type === "state") return;
        } catch { /* Ignore non-JSON messages. */ }
        client.send(message);
      });
    });
    await page.goto(`${baseURL}/canvas/preview-${slug}?editor=1`, {
      waitUntil: "networkidle",
    });
    for (const widget of scene.canvas.widgets) {
      await expect.poll(() => widgetRect(page, widget)).not.toBeNull();
    }
    const widget = (id) => scene.canvas.widgets.find((item) => item.id === id);
    const contains = async (id, value) => {
      await expect.poll(() => widgetText(page, widget(id))).toContain(
        String(value),
      );
    };
    const { chart, best, detail, history } = scene.state;
    for (
      const value of [
        chart.title,
        chart.artist,
        chart.play_type === "single" ? "SP" : "DP",
        chart.difficulty.toUpperCase(),
        String(chart.level),
        String(chart.notes),
      ]
    ) {
      await contains("selection", value);
    }
    for (
      const value of [
        best.score,
        best.dj_level,
        best.miss,
        best.clear,
        detail.play_options,
      ]
    ) {
      await contains("score", value);
    }
    for (
      const field of [
        "pgreat",
        "great",
        "good",
        "bad",
        "poor",
        "fast",
        "slow",
        "combo_break",
      ]
    ) {
      await contains("score", field.replace("_", " ").toUpperCase());
      await contains("score", detail[field]);
    }
    for (const value of ["SYSTEM", "RESULT", "SELECT"]) {
      await contains("status", value);
    }
    for (const value of ["DATE", "SCORE", "DJ LEVEL", "MISS", "CLEAR"]) {
      await contains("history", value);
    }
    const historyCount =
      scene.canvas.widgets.find((item) => item.kind === "history-list")
        ?.settings?.history_count ?? 5;
    for (const play of history.plays.slice(0, historyCount)) {
      for (
        const value of [
          play.notified_at,
          play.score,
          play.dj_level,
          play.miss,
          play.clear,
        ]
      ) await contains("history", value);
    }
    for (const value of ["DJ LEVEL", "MISS RATE"]) {
      await contains("graph", value);
    }
    const apertureTitle = scene.canvas.widgets.find((widget) =>
      widget.id === "aperture"
    )?.settings?.title;
    if (apertureTitle) {
      await contains("aperture", apertureTitle);
    }
    expect(failures).toEqual([]);
    await page.screenshot({ path: outputPath, omitBackground: true });
    const caseId = path.basename(outputPath, ".png");
    const framesDirectory = path.join(path.dirname(outputPath), caseId);
    fs.mkdirSync(framesDirectory, { recursive: true });
    const fps = scene.media.fps;
    const durationMs = scene.media.duration_ms;
    const paintPadding = scene.canvas.paint_padding;
    expect(Number.isInteger(paintPadding) && paintPadding >= 0).toBe(true);
    async function assertPaintFits(widget) {
      const extra = 8;
      const outerPad = paintPadding + extra;
      const clip = {
        x: Math.max(0, widget.x - outerPad),
        y: Math.max(0, widget.y - outerPad),
        width:
          Math.min(scene.canvas.width, widget.x + widget.width + outerPad) -
          Math.max(0, widget.x - outerPad),
        height:
          Math.min(scene.canvas.height, widget.y + widget.height + outerPad) -
          Math.max(0, widget.y - outerPad),
      };
      const before = await page.screenshot({ clip, omitBackground: true });
      const element = await widgetRoot(page, widget);
      expect(await element.evaluate((node) => node !== null)).toBe(true);
      const rootInfo = await element.evaluate((node) => ({
        tag: node.tagName,
        className: node.getAttribute("class"),
      }));
      let without;
      try {
        await element.evaluate((node) => {
          node.style.visibility = "hidden";
        });
        without = await page.screenshot({ clip, omitBackground: true });
      } finally {
        await element.evaluate((node) => {
          node.style.visibility = "";
        });
        await element.dispose();
      }
      // Restoring the widget distinguishes its paint from a moving canvas
      // background or a neighboring animation between the first two captures.
      const restored = await page.screenshot({ clip, omitBackground: true });
      const overflowPixels = await page.evaluate(
        async (
          {
            first,
            second,
            restored,
            width,
            height,
            widget,
            paintPadding,
            clipX,
            clipY,
            neighbors,
          },
        ) => {
          async function pixels(base64) {
            const bytes = Uint8Array.from(
              atob(base64),
              (char) => char.charCodeAt(0),
            );
            const bitmap = await createImageBitmap(
              new Blob([bytes], { type: "image/png" }),
            );
            const canvas = document.createElement("canvas");
            canvas.width = width;
            canvas.height = height;
            const context = canvas.getContext("2d", {
              willReadFrequently: true,
            });
            context.drawImage(bitmap, 0, 0);
            bitmap.close();
            return context.getImageData(0, 0, width, height).data;
          }
          const a = await pixels(first);
          const b = await pixels(second);
          const c = await pixels(restored);
          let changed = 0;
          for (let y = 0; y < height; y += 1) {
            for (let x = 0; x < width; x += 1) {
              const pageX = clipX + x;
              const pageY = clipY + y;
              if (
                pageX >= widget.x - paintPadding &&
                pageX < widget.x + widget.width + paintPadding &&
                pageY >= widget.y - paintPadding &&
                pageY < widget.y + widget.height + paintPadding
              ) continue;
              // An adjacent widget can occupy the review margin. Its own
              // animation is unrelated to the widget being hidden here.
              if (
                neighbors.some((other) =>
                  pageX >= other.x && pageX < other.x + other.width &&
                  pageY >= other.y && pageY < other.y + other.height
                )
              ) continue;
              const offset = (y * width + x) * 4;
              if (
                [0, 1, 2, 3].some((channel) =>
                  Math.abs(a[offset + channel] - b[offset + channel]) > 8 &&
                  Math.abs(c[offset + channel] - b[offset + channel]) > 8 &&
                  Math.abs(a[offset + channel] - c[offset + channel]) <= 8
                )
              ) changed += 1;
            }
          }
          return changed;
        },
        {
          first: before.toString("base64"),
          second: without.toString("base64"),
          restored: restored.toString("base64"),
          width: clip.width,
          height: clip.height,
          widget,
          paintPadding,
          clipX: clip.x,
          clipY: clip.y,
          neighbors: scene.canvas.widgets.filter((other) =>
            other.id !== widget.id
          ),
        },
      );
      if (overflowPixels > 0) {
        fs.writeFileSync(
          path.join(framesDirectory, `${widget.id}-paint-before.png`),
          before,
        );
        fs.writeFileSync(
          path.join(framesDirectory, `${widget.id}-paint-hidden.png`),
          without,
        );
        fs.writeFileSync(
          path.join(framesDirectory, `${widget.id}-paint-restored.png`),
          restored,
        );
      }
      expect(
        overflowPixels,
        `${widget.id} paints beyond ${paintPadding}px review padding; root=${
          JSON.stringify(rootInfo)
        }`,
      ).toBe(0);
    }
    if (caseId.startsWith("boundary-")) {
      const kind = caseId.startsWith("boundary-history-")
        ? "history-list"
        : caseId.startsWith("boundary-graph-")
        ? "history-graph"
        : caseId.startsWith("boundary-empty-")
        ? "empty"
        : "score";
      await assertPaintFits(
        scene.canvas.widgets.find((item) => item.kind === kind),
      );
      expect(failures).toEqual([]);
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
      return;
    }
    const frameCount = durationMs * fps / 1000;
    let paintChecks = 0;
    await page.evaluate(() => {
      for (const animation of document.getAnimations()) animation.pause();
    });
    let previousTime = 0;
    for (let frame = 0; frame < frameCount; frame += 1) {
      const time = Math.round(frame * 1000 / fps);
      if (time > previousTime) await page.clock.runFor(time - previousTime);
      previousTime = time;
      await page.evaluate((time) => {
        for (const animation of document.getAnimations()) {
          animation.currentTime = time;
        }
      }, time);
      const actualTime = await page.evaluate(() =>
        Math.round(performance.now())
      );
      expect(actualTime).toBeGreaterThanOrEqual(time);
      expect(actualTime).toBeLessThanOrEqual(time + 10);
      reviewSocket.send(JSON.stringify({ type: "state", state: scene.state }));
      await expect.poll(() =>
        page.evaluate(() => window.__skinReviewWorker?.lastCompletedMs)
      ).toBe(actualTime);
      // A Wasm render can replace DOM nodes and start fresh CSS animations.
      // Seek the resulting tree as well as the tree that existed before render.
      await page.evaluate((time) => {
        for (const animation of document.getAnimations()) {
          animation.pause();
          animation.currentTime = time;
        }
      }, time);
      if (
        frame === 0 || frame === Math.floor(frameCount / 2) ||
        frame === frameCount - 1
      ) {
        for (const kind of ["selection", "score"]) {
          const widget = scene.canvas.widgets.find((item) =>
            item.kind === kind
          );
          await assertPaintFits(widget);
          paintChecks += 1;
        }
      }
      const number = String(frame).padStart(3, "0");
      if (caseId === "01" || caseId.startsWith("background-")) {
        await page.screenshot({
          path: path.join(framesDirectory, `frame-${number}.png`),
          omitBackground: true,
        });
        if (caseId === "01") {
          fs.writeFileSync(
            path.join(framesDirectory, `backdrop-${number}.sha256`),
            await backdropDigest(
              await captureCanvasBackdrop(page, scene.canvas.widgets),
            ),
          );
        }
      } else {
        for (const kind of ["selection", "score"]) {
          const widget = scene.canvas.widgets.find((item) =>
            item.kind === kind
          );
          expect(widget).toBeTruthy();
          const pad = paintPadding;
          const left = Math.max(0, widget.x - pad);
          const top = Math.max(0, widget.y - pad);
          const clip = {
            x: left,
            y: top,
            width: Math.min(scene.canvas.width, widget.x + widget.width + pad) -
              left,
            height:
              Math.min(scene.canvas.height, widget.y + widget.height + pad) -
              top,
          };
          await captureWidgetOnly(
            page,
            widget,
            clip,
            path.join(framesDirectory, `${kind}-${number}.png`),
            frame === 0 || frame === Math.floor(frameCount / 2) ||
              frame === frameCount - 1,
            {
              widgets: scene.canvas.widgets,
              referencePath: path.join(
                path.dirname(framesDirectory),
                "01",
                `backdrop-${number}.sha256`,
              ),
            },
          );
        }
      }
    }
    expect(failures).toEqual([]);
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
    fs.writeFileSync(
      path.join(framesDirectory, "timing.json"),
      JSON.stringify({
        fps,
        duration_ms: durationMs,
        frame_count: frameCount,
        monotonic_base_ms: 0,
        paint_checks: paintChecks,
      }),
    );
    await context.close();
  },
  { timeoutMs: 15 * 60_000 },
);
