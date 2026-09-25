import { expect } from "playwright/test";
import { Buffer } from "node:buffer";
import { browserTest } from "../../../../tests/browser-test.js";
import {
  backdropDigest,
  captureCanvasBackdrop,
  captureWidgetOnly,
} from "./browser-widget-regions.js";

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
