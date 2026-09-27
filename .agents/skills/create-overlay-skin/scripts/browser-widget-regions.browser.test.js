import { expect } from "playwright/test";
import { Buffer } from "node:buffer";
import { browserTest } from "../../../../tests/browser-test.js";
import {
  backdropDigest,
  captureCanvasBackdrop,
  captureWidgetOnly,
  widgetTextVisibility,
} from "./browser-widget-regions.js";

browserTest(
  "text ending at a node boundary stays within the visible title",
  async ({ page }) => {
    const widget = { id: "selection", x: 20, y: 20, width: 440, height: 126 };
    await page.setContent(`
      <style>
        body { margin: 0; }
        .panel { position: absolute; left: 20px; top: 20px; width: 440px;
          height: 126px; overflow: hidden; }
        .title { display: block; height: 38px; overflow: hidden; }
        .artist { display: block; height: 20px; }
      </style>
      <div class="panel"><span class="title">検証用 SONG 01</span><span class="artist">Example Artist</span></div>
    `);
    expect(await widgetTextVisibility(page, widget, "検証用 SONG 01"))
      .toEqual({ visible: true });
  },
);

browserTest(
  "supplied text must fit the widget and its clipping ancestors",
  async ({ page }) => {
    const title = "検証用の長い和英混在タイトル SONG 08 EXTENDED MIX";
    const artist = "Example Artist / 架空アーティスト 08";
    const options = "MIRROR + FLIP";
    const selection = {
      id: "selection",
      x: 20,
      y: 20,
      width: 544,
      height: 124,
    };
    const score = { id: "score", x: 20, y: 170, width: 544, height: 200 };
    await page.setContent(`
      <style>
        body { margin: 0; font: 17px/1.3 sans-serif; }
        .panel { position: absolute; left: 20px; width: 544px; overflow: hidden; }
        .selection { top: 20px; height: 124px; }
        .score { top: 170px; height: 200px; }
        .title, .artist, .options { display: block; margin: 8px; }
        .title { width: 500px; }
        .artist { width: 500px; }
        .options { width: 500px; }
        .clipped { width: 180px; white-space: nowrap; overflow: hidden;
          text-overflow: ellipsis; }
        .options.clipped { width: 80px; text-overflow: clip; }
      </style>
      <div class="panel selection">
        <div class="title clipped"><span>検証用の長い和英混在タイトル </span><span>SONG 08 EXTENDED MIX</span></div>
        <div class="artist">${artist}</div>
      </div>
      <div class="panel score"><div class="options clipped">${options}</div></div>
    `);
    expect((await widgetTextVisibility(page, selection, title)).reason).toBe(
      "text is clipped",
    );
    expect((await widgetTextVisibility(page, score, options)).reason).toBe(
      "text is clipped",
    );
    await page.evaluate(() => {
      for (const element of document.querySelectorAll(".clipped")) {
        element.classList.remove("clipped");
      }
    });
    for (
      const [widget, value] of [
        [selection, title],
        [selection, artist],
        [score, options],
      ]
    ) {
      expect(await widgetTextVisibility(page, widget, value)).toEqual({
        visible: true,
      });
    }
  },
);

browserTest(
  "backdrop capture preserves alpha and rejects mixed backdrops",
  async ({ page }) => {
    const widget = { id: "target", x: 20, y: 20, width: 12, height: 12 };
    const folder = await Deno.makeTempDir({ prefix: "scorepeek-blend-probe-" });
    const referencePath = `${folder}/backdrop.sha256`;
    const outputPath = `${folder}/target.png`;
    try {
      await page.setContent(`
      <style>
        html, body { margin: 0; background: transparent; }
        #target { position: absolute; left: 20px; top: 20px; width: 12px;
          height: 12px; }
        #target::before { content: ""; position: absolute; inset: 0;
          background: white; opacity: .5; mix-blend-mode: screen; }
      </style>
      <div id="target"></div>
    `);
      const transparentDigest = await backdropDigest(
        await captureCanvasBackdrop(page, [widget]),
      );
      await Deno.writeTextFile(referencePath, "0".repeat(64));
      await expect(captureWidgetOnly(
        page,
        widget,
        widget,
        outputPath,
        false,
        { widgets: [widget], referencePath },
      )).rejects.toThrow("canvas backdrop differs");
      await Deno.writeTextFile(referencePath, transparentDigest);
      await expect(captureWidgetOnly(
        page,
        widget,
        widget,
        outputPath,
      )).rejects.toThrow("backdrop reference is required");
      const image = await captureWidgetOnly(
        page,
        widget,
        widget,
        outputPath,
        false,
        { widgets: [widget], referencePath },
      );
      const alpha = await page.evaluate(async (base64) => {
        const bytes = Uint8Array.from(
          atob(base64),
          (char) => char.charCodeAt(0),
        );
        const bitmap = await createImageBitmap(
          new Blob([bytes], { type: "image/png" }),
        );
        const canvas = document.createElement("canvas");
        canvas.width = bitmap.width;
        canvas.height = bitmap.height;
        const context = canvas.getContext("2d", { willReadFrequently: true });
        context.drawImage(bitmap, 0, 0);
        bitmap.close();
        return context.getImageData(6, 6, 1, 1).data[3];
      }, Buffer.from(image).toString("base64"));
      expect(alpha).toBeGreaterThanOrEqual(126);
      expect(alpha).toBeLessThanOrEqual(129);

      await page.evaluate(() => {
        document.documentElement.style.background = "rgba(10, 20, 30, .5)";
      });
      await Deno.writeTextFile(
        referencePath,
        await backdropDigest(await captureCanvasBackdrop(page, [widget])),
      );
      await expect(captureWidgetOnly(
        page,
        widget,
        widget,
        outputPath,
        false,
        { widgets: [widget], referencePath },
      )).rejects.toThrow("partial alpha");
    } finally {
      await Deno.remove(folder, { recursive: true });
    }
  },
);
