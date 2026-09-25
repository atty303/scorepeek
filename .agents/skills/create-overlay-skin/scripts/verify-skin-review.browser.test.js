import { expect } from "playwright/test";
import fs from "node:fs";
import { browserTest } from "../../../../tests/browser-test.js";

const scenePath = Deno.env.get("SCOREPEEK_SKIN_REVIEW_SCENE");
const baseURL = Deno.env.get("SCOREPEEK_SKIN_REVIEW_URL");
const outputPath = Deno.env.get("SCOREPEEK_SKIN_REVIEW_SCREENSHOT");

browserTest("verify all review widgets in the production browser", async ({ browser }) => {
  expect(scenePath).toBeTruthy();
  expect(baseURL).toBeTruthy();
  expect(outputPath).toBeTruthy();
  const scene = JSON.parse(fs.readFileSync(scenePath, "utf8"));
  const context = await browser.newContext({
    viewport: { width: scene.canvas.width, height: scene.canvas.height },
    deviceScaleFactor: 1,
  });
  const page = await context.newPage();
  const failures = [];
  page.on("pageerror", (error) => failures.push(error.message));
  page.on("response", (response) => {
    if (response.url().includes("/skin/") && !response.ok()) {
      failures.push(`${response.status()} ${response.url()}`);
    }
  });
  const slug = scene.skins[0].slug;
  await page.routeWebSocket(`**/ws/preview-${slug}`, (client) => {
    client.send(JSON.stringify({ type: "state", state: scene.state }));
    const server = client.connectToServer();
    client.onMessage((message) => server.send(message));
    server.onMessage((message) => {
      const value = typeof message === "string" ? message : message.toString();
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
    await expect(page.locator(`.widget-slot[data-widget-id="${widget.id}"]`))
      .toBeVisible();
  }
  const slot = (id) => page.locator(`.widget-slot[data-widget-id="${id}"]`);
  const { chart, best, detail, history } = scene.state;
  for (const value of [chart.title, chart.artist, chart.difficulty.toUpperCase(), String(chart.level), String(chart.notes)]) {
    await expect(slot("selection")).toContainText(value);
  }
  for (const value of [best.score, best.dj_level, best.miss, best.clear, detail.play_options]) {
    await expect(slot("score")).toContainText(value);
  }
  for (const field of ["pgreat", "great", "good", "bad", "poor", "fast", "slow", "combo_break"]) {
    await expect(slot("score")).toContainText(field.replace("_", " ").toUpperCase());
    await expect(slot("score")).toContainText(detail[field]);
  }
  for (const value of ["SYSTEM", "RESULT", "SELECT"]) {
    await expect(slot("status")).toContainText(value);
  }
  for (const value of ["DATE", "SCORE", "DJ LEVEL", "MISS", "CLEAR"]) {
    await expect(slot("history")).toContainText(value);
  }
  for (const value of ["DJ LEVEL", "MISS RATE"]) {
    await expect(slot("graph")).toContainText(value);
  }
  const apertureTitle = scene.canvas.widgets.find((widget) =>
    widget.id === "aperture"
  )?.settings?.title;
  if (apertureTitle) await expect(slot("aperture")).toContainText(apertureTitle);
  expect(failures).toEqual([]);
  await page.screenshot({ path: outputPath });
  await context.close();
});
