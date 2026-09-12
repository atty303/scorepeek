const { test, expect } = require("playwright/test");

const baseURL = process.env.SCOREPEEK_BROWSER_TEST_URL;
const executablePath = process.env.SCOREPEEK_PLAYWRIGHT_CHROMIUM;

if (executablePath) {
  test.use({ launchOptions: { executablePath } });
}

test("editor replicas follow drag, stale websocket delivery, scroll, and lifecycle", async ({ page }) => {
  const pageErrors = [];
  const replicaLifecycle = [];
  const skinWasmRequests = [];
  let delayedCanvasMessages = 0;
  page.on("pageerror", (error) => pageErrors.push(error.message));
  page.on("framenavigated", (frame) => {
    if (frame.url().includes("/canvas/")) replicaLifecycle.push({ phase: "loaded", url: frame.url() });
  });
  page.on("framedetached", (frame) => {
    if (frame.url().includes("/canvas/")) replicaLifecycle.push({ phase: "destroyed", url: frame.url() });
  });
  page.on("request", (request) => {
    if (request.url().includes("/skin/") && request.url().endsWith("/skin.wasm")) {
      skinWasmRequests.push(request.url());
    }
  });
  await page.addInitScript(() => {
    window.__scorepeekTest = {
      presentations: [],
      acceptedPresentations: [],
      draftUpdates: [],
      lastSession: undefined,
      lastRevision: -1,
    };
    const browserSend = WebSocket.prototype.send;
    WebSocket.prototype.send = function scorepeekTestSend(message) {
      if (typeof message === "string") {
        try {
          const envelope = JSON.parse(message);
          if (envelope.request?.command === "update_backend_draft") {
            window.__scorepeekTest.draftUpdates.push(envelope);
          }
        } catch (_) {}
      }
      return browserSend.call(this, message);
    };
    addEventListener("message", (event) => {
      if (typeof event.data !== "string") return;
      try {
        const message = JSON.parse(event.data);
        if (message.type === "scorepeek-editor-presentation") {
          window.__scorepeekTest.presentations.push(message);
          if (message.session_id !== window.__scorepeekTest.lastSession
            || message.revision > window.__scorepeekTest.lastRevision) {
            window.__scorepeekTest.lastSession = message.session_id;
            window.__scorepeekTest.lastRevision = message.revision;
            window.__scorepeekTest.acceptedPresentations.push(message);
          }
        }
      } catch (_) {}
    }, { capture: true });
  });
  await page.routeWebSocket("**/ws/canvas-*", (client) => {
    const server = client.connectToServer();
    client.onMessage((message) => server.send(message));
    server.onMessage((message) => {
      delayedCanvasMessages += 1;
      setTimeout(() => client.send(message), 250);
    });
  });

  await page.goto(`${baseURL}/overlay`);
  const addCanvas = page.getByRole("button", { name: "+ Add canvas", exact: true });
  await expect(addCanvas).toBeEnabled();
  await addCanvas.click();
  const widgetPicker = page.locator("#widget-picker-trigger");
  await widgetPicker.click();
  await expect(widgetPicker).toBeFocused();
  await page.locator(".list-picker-option[data-index='0']").click();

  const frame = page.frameLocator("#scorepeek-replica-canvas-1");
  const handle = page.locator(".editor-widget-hit");
  const slot = frame.locator(".widget-slot").first();
  await expect(slot).toBeVisible();
  const before = await handle.boundingBox();
  expect(before).not.toBeNull();
  const updateCountBeforeDrag = await page.evaluate(() => window.__scorepeekTest.draftUpdates.length);
  const initialReplicaFrame = page.frames().find((candidate) => candidate.url().includes("/canvas/canvas-1"));
  expect(initialReplicaFrame).toBeDefined();
  const presentationCountBeforeDrag = await initialReplicaFrame.evaluate(
    () => window.__scorepeekTest.acceptedPresentations.length,
  );
  await page.mouse.move(before.x + before.width / 2, before.y + before.height / 2);
  await page.mouse.down();
  for (let step = 1; step <= 8; step += 1) {
    await page.mouse.move(
      before.x + before.width / 2 + (140 * step) / 8,
      before.y + before.height / 2 + (108 * step) / 8,
    );
    await page.waitForTimeout(10);
  }
  await page.mouse.up();

  const expected = { x: before.x + 140, y: before.y + 108, width: before.width, height: before.height };
  await expect.poll(async () => handle.boundingBox()).toEqual(expected);
  await expect.poll(async () => slot.boundingBox()).toEqual(expected);
  await page.waitForTimeout(400);
  expect(await slot.boundingBox()).toEqual(expected);
  const dragUpdates = await page.evaluate((start) => window.__scorepeekTest.draftUpdates.slice(start), updateCountBeforeDrag);
  expect(dragUpdates).toHaveLength(1);
  const persistedCanvas = dragUpdates[0].request.canvases.find((candidate) => candidate.id === "canvas-1");
  const persistedWidget = persistedCanvas.widgets[0];
  const persistedGeometry = [
    persistedWidget.x,
    persistedWidget.y,
    persistedWidget.width,
    persistedWidget.height,
  ];
  expect(persistedGeometry).toEqual([
    expected.x,
    expected.y,
    expected.width,
    expected.height,
  ]);
  const replicaFrame = page.frames().find((candidate) => candidate.url().includes("/canvas/canvas-1"));
  expect(replicaFrame).toBeDefined();
  const replicaRevisions = await replicaFrame.evaluate(
    () => window.__scorepeekTest.acceptedPresentations.map(({ session_id, revision, specification }) => ({
      session_id,
      revision,
      geometry: specification.widgets.map(({ x, y, width, height }) => [x, y, width, height]),
    })),
  );
  expect(replicaRevisions.length).toBeGreaterThan(0);
  expect(replicaRevisions.every((current, index, values) => index === 0
    || current.session_id !== values[index - 1].session_id
    || current.revision > values[index - 1].revision)).toBe(true);
  const dragReplicaRevisions = replicaRevisions.slice(presentationCountBeforeDrag);
  expect(dragReplicaRevisions.length).toBeGreaterThanOrEqual(8);
  expect(new Set(dragReplicaRevisions.map(({ revision }) => revision)).size)
    .toBeGreaterThanOrEqual(8);
  expect(new Set(dragReplicaRevisions.map(({ geometry }) => JSON.stringify(geometry[0]))).size)
    .toBeGreaterThanOrEqual(8);
  expect(replicaRevisions.at(-1).geometry[0]).toEqual(persistedGeometry);

  await widgetPicker.click();
  await widgetPicker.press("End");
  await widgetPicker.press("Enter");

  await page.getByRole("button", { name: "obs-output", exact: true }).click();
  await page.getByRole("button", { name: "+ Add canvas", exact: true }).click();
  await page.locator("#widget-picker-trigger").click();
  await page.locator(".list-picker-option[data-index='0']").click();
  await expect(
    page.frameLocator("#scorepeek-replica-canvas-2").locator(".widget-slot"),
  ).toBeVisible();
  await page.getByRole("button", { name: "Canvas 1", exact: true }).click();

  for (let index = 0; index < 7; index += 1) {
    await page.locator("#widget-picker-trigger").click();
    await page.locator(".list-picker-option[data-index='0']").click();
  }
  const navigator = page.locator(".navigator-scroll");
  await navigator.evaluate((node) => { node.scrollTop = 0; });
  const canvasScrollPoint = await page.locator(".canvas-select").first().evaluate((node) => {
    const row = node.getBoundingClientRect();
    const viewport = node.closest(".navigator-scroll").getBoundingClientRect();
    return {
      x: row.left + row.width / 2,
      y: Math.max(viewport.top + 2, Math.min(row.top + row.height / 2, viewport.bottom - 2)),
    };
  });
  await page.mouse.move(canvasScrollPoint.x, canvasScrollPoint.y);
  await page.mouse.wheel(0, 600);
  await expect.poll(async () => navigator.evaluate((node) => node.scrollTop)).toBeGreaterThan(0);
  await navigator.evaluate((node) => { node.scrollTop = 0; });
  await page.locator(".widget-row").first().hover();
  await page.mouse.wheel(0, 600);
  await expect.poll(async () => navigator.evaluate((node) => node.scrollTop)).toBeGreaterThan(0);

  await page.getByRole("button", { name: "Canvas 1", exact: true }).click();
  const inspector = page.locator(".inspector-scroll");
  await page.locator("#canvas-1\\:x").hover();
  await page.mouse.wheel(0, 700);
  await expect.poll(async () => inspector.evaluate((node) => node.scrollTop)).toBeGreaterThan(0);

  const visibility = page
    .locator(".editor-accordion-heading[data-section='canvas:canvas-1:visibility']")
    .locator("..");
  const lifecycleBeforeVisibility = replicaLifecycle.length;
  const wasmBeforeVisibility = skinWasmRequests.length;
  await visibility.getByRole("button", { name: "None", exact: true }).click();
  await expect(visibility.locator(".editor-toggle.selected")).toHaveCount(0);
  await expect(page.locator("#scorepeek-replica-canvas-1")).toHaveCount(0);
  await expect.poll(() => replicaLifecycle.slice(lifecycleBeforeVisibility)
    .some((event) => event.phase === "destroyed")).toBe(true);
  await visibility.getByRole("button", { name: "All", exact: true }).click();
  await expect(visibility.locator(".editor-toggle.selected")).toHaveCount(6);
  await expect(page.locator("#scorepeek-replica-canvas-1")).toHaveCount(1);
  await expect.poll(() => skinWasmRequests.length).toBeGreaterThan(wasmBeforeVisibility);

  const appearance = page
    .locator(".editor-accordion-heading[data-section='canvas:canvas-1:appearance']")
    .locator("..");
  await appearance.getByRole("button", { name: "Static", exact: true }).click();
  await expect(frame.locator(".canvas-background-art")).toBeVisible();
  await expect(frame.locator(".canvas-background")).toHaveCSS(
    "background-color",
    "rgb(14, 25, 37)",
  );
  await expect.poll(async () => frame.locator(".canvas-background").boundingBox()).toEqual({
    x: 0,
    y: 0,
    width: 1280,
    height: 720,
  });
  await expect.poll(async () => frame.locator(".canvas-background-art").evaluate((node) =>
    getComputedStyle(node).backgroundImage)).toContain("/skin/");
  if (process.env.SCOREPEEK_BROWSER_SCREENSHOT) {
    await page.screenshot({
      path: process.env.SCOREPEEK_BROWSER_SCREENSHOT,
      animations: "allow",
    });
    const currentReplicaFrame = page.frames().find(
      (candidate) => candidate.url().includes("/canvas/canvas-1"),
    );
    expect(currentReplicaFrame).toBeDefined();
    await currentReplicaFrame.locator("html").screenshot({
      path: process.env.SCOREPEEK_BROWSER_SCREENSHOT.replace(/\.png$/, "-iframe.png"),
      animations: "allow",
    });
  }
  const initialFrameURL = await page.locator("#scorepeek-replica-canvas-1").getAttribute("src");
  const lifecycleBeforeSkin = replicaLifecycle.length;
  const wasmBeforeSkin = skinWasmRequests.length;
  await appearance.locator(".skin-option").nth(1).click();
  await expect(page.locator("#scorepeek-replica-canvas-1")).not.toHaveAttribute(
    "src",
    initialFrameURL,
  );
  await expect(slot).toBeVisible();
  await expect.poll(() => replicaLifecycle.slice(lifecycleBeforeSkin)
    .filter((event) => event.phase === "loaded").length).toBeGreaterThan(0);
  await expect.poll(() => skinWasmRequests.length).toBeGreaterThan(wasmBeforeSkin);

  await page.locator(".widget-row[data-widget-id='empty-1']").click();
  const opacity = page.locator("#canvas-1\\:empty-1\\:property\\:fill-opacity-percent");
  await opacity.fill("50");
  await opacity.press("Enter");
  await expect(frame.locator(".empty-fill")).toHaveAttribute(
    "style",
    /rgba\(0,\s*0,\s*0,\s*0\.5\)/,
  );
  await page.getByRole("button", { name: "DELETE WIDGET", exact: true }).click();
  await expect(frame.locator(".empty-widget")).toHaveCount(0);

  await page.getByRole("button", { name: "Canvas 2", exact: true }).click();
  await page.getByRole("button", { name: "Delete canvas", exact: true }).click();
  await expect(page.locator("#scorepeek-replica-canvas-2")).toHaveCount(0);
  await expect(page.locator("#scorepeek-replica-canvas-1")).toHaveCount(1);
  await page.getByRole("button", { name: "Canvas 1", exact: true }).click();
  await page.getByRole("button", { name: "Save & Close", exact: true }).click();
  await expect(page.locator(".editor-panel")).toHaveCount(0);
  await expect(frame.locator(".canvas-background-art")).toBeVisible();
  expect(await frame.locator(".canvas-background-art").evaluate((node) =>
    getComputedStyle(node).backgroundImage)).toContain("/skin/");
  expect(delayedCanvasMessages).toBeGreaterThan(0);
  expect(pageErrors).toEqual([]);
});
