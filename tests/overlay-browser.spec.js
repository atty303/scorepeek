const { test, expect } = require("playwright/test");

const baseURL = process.env.SCOREPEEK_BROWSER_TEST_URL;
const executablePath = process.env.SCOREPEEK_PLAYWRIGHT_CHROMIUM;

if (executablePath) {
  test.use({ launchOptions: { executablePath } });
}

test("asset version mismatch reloads the editor automatically", async ({ page }) => {
  const stageConnections = [];
  let mainFrameNavigations = 0;
  page.on("framenavigated", (frame) => {
    if (frame === page.mainFrame() && frame.url().includes("/overlay")) {
      mainFrameNavigations += 1;
    }
  });
  await page.routeWebSocket("**/ws/stage?**", (client) => {
    stageConnections.push(client);
    if (stageConnections.length === 1) {
      client.send(JSON.stringify({
        type: "version_mismatch",
        asset_version: "replacement-build",
      }));
      return;
    }
    const server = client.connectToServer();
    client.onMessage((message) => server.send(message));
    server.onMessage((message) => client.send(message));
  });

  await page.goto(`${baseURL}/overlay`);

  await expect.poll(() => stageConnections.length).toBe(2);
  await expect.poll(() => mainFrameNavigations).toBe(2);
  await expect(page.locator(".version-mismatch")).toHaveCount(0);
  await expect(page.locator("#stage")).toBeVisible();

  stageConnections[1].send(JSON.stringify({
    type: "version_mismatch",
    asset_version: "another-replacement-build",
  }));

  await expect.poll(() => stageConnections.length).toBe(3);
  await expect.poll(() => mainFrameNavigations).toBe(3);
  await expect(page.locator(".version-mismatch")).toHaveCount(0);
  await expect(page.locator("#stage")).toBeVisible();
});

test("repeated asset version mismatch stops after one automatic reload", async ({ page }) => {
  let stageConnections = 0;
  await page.routeWebSocket("**/ws/stage?**", (client) => {
    stageConnections += 1;
    client.send(JSON.stringify({
      type: "version_mismatch",
      asset_version: "replacement-build",
    }));
  });

  await page.goto(`${baseURL}/overlay`);

  await expect.poll(() => stageConnections).toBe(2);
  await expect(page.locator(".version-mismatch")).toBeVisible();
  await page.waitForTimeout(750);
  expect(stageConnections).toBe(2);
});

test("editor replicas reconnect and follow drag, stale delivery, scroll, and lifecycle", async ({ page }) => {
  test.setTimeout(90_000);
  const pageErrors = [];
  const replicaLifecycle = [];
  const skinWasmRequests = [];
  const canvasPresentations = [];
  const canvasStates = [];
  const canvasConnections = [];
  let rejectedCanvasConnections = 0;
  let delayedCanvasMessages = 0;
  let delayNextStageDraftReply = false;
  let delayedStageDraftReplies = 0;
  const delayedStageDraftRequests = new Set();
  const stageConnections = [];
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
  await page.routeWebSocket("**/ws/canvas-*", async (client) => {
    const connection = { client, server: undefined, url: client.url() };
    canvasConnections.push(connection);
    if (rejectedCanvasConnections > 0) {
      rejectedCanvasConnections -= 1;
      await client.close({ code: 1012, reason: "backend restarting" });
      return;
    }
    const server = client.connectToServer();
    connection.server = server;
    client.onMessage((message) => server.send(message));
    server.onMessage((message) => {
      delayedCanvasMessages += 1;
      try {
        const envelope = JSON.parse(typeof message === "string" ? message : message.toString());
        if (envelope.type === "presentation") canvasPresentations.push(envelope.specification);
        if (envelope.type === "state") canvasStates.push(envelope.state);
      } catch (_) {}
      setTimeout(() => client.send(message), 250);
    });
  });
  await page.routeWebSocket("**/ws/stage?**", (client) => {
    const server = client.connectToServer();
    let holdInitialMessages = true;
    const initialMessages = [];
    stageConnections.push({
      release() {
        holdInitialMessages = false;
        for (const message of initialMessages.splice(0)) client.send(message);
      },
    });
    client.onMessage((message) => {
      try {
        const envelope = JSON.parse(typeof message === "string" ? message : message.toString());
        if (delayNextStageDraftReply
          && envelope.request?.command === "update_backend_draft") {
          delayedStageDraftRequests.add(envelope.request_id);
          delayNextStageDraftReply = false;
        }
      } catch (_) {}
      server.send(message);
    });
    server.onMessage((message) => {
      if (holdInitialMessages) {
        initialMessages.push(message);
        return;
      }
      try {
        const envelope = JSON.parse(typeof message === "string" ? message : message.toString());
        if (delayedStageDraftRequests.delete(envelope.request_id)) {
          delayedStageDraftReplies += 1;
          setTimeout(() => client.send(message), 500);
          return;
        }
      } catch (_) {}
      client.send(message);
    });
  });

  await page.goto(`${baseURL}/overlay`);
  await expect.poll(() => stageConnections.length).toBe(1);
  const addCanvas = page.getByRole("button", { name: "+ Add canvas", exact: true });
  await expect(page.locator(".editor-panel")).toHaveCount(0);
  await page.locator("#stage").click({ button: "right" });
  stageConnections[0].release();
  await expect(addCanvas).toBeEnabled();
  await addCanvas.click();
  const widgetPicker = page.locator("#widget-picker-trigger");
  await widgetPicker.click();
  await expect(widgetPicker).toBeFocused();
  await page.locator(".list-picker-option[data-index='0']").click();

  for (let index = 0; index < 7; index += 1) {
    await page.locator("#widget-picker-trigger").click();
    await page.locator(".list-picker-option[data-index='0']").click();
  }
  await page.getByRole("button", { name: "obs-output", exact: true }).click();
  await page.getByRole("button", { name: "+ Add canvas", exact: true }).click();
  await page.locator("#widget-picker-trigger").click();
  await page.locator(".list-picker-option[data-index='0']").click();
  const secondWidget = page.locator(".editor-canvas[data-canvas='canvas-2'] .editor-widget-hit");
  for (const field of ["x", "y"]) {
    const input = page.locator(`[id='canvas-2:status-1:${field}']`);
    await input.fill("0");
    await input.press("Enter");
    await expect.poll(async () => (await secondWidget.boundingBox())[field]).toBe(0);
  }
  await page.getByRole("button", { name: "Canvas 2", exact: true }).click();
  const secondCanvas = page.locator(".editor-canvas[data-canvas='canvas-2']");
  for (const [field, value] of [["width", "600"], ["height", "300"], ["x", "0"], ["y", "400"]]) {
    const input = page.locator(`[id='canvas-2:${field}']`);
    await input.fill(value);
    await input.blur();
    await expect.poll(async () => (await secondCanvas.boundingBox())[field]).toBe(Number(value));
  }
  const unrelatedFrame = page.frameLocator("#scorepeek-replica-canvas-2");
  await expect(unrelatedFrame.locator(".widget-slot")).toBeVisible();
  await page.getByRole("button", { name: "Canvas 1", exact: true }).click();

  const frame = page.frameLocator("#scorepeek-replica-canvas-1");
  const candidateHandle = page.locator(".editor-canvas[data-canvas='canvas-1'] .editor-widget-hit").last();
  await candidateHandle.click();
  const draggedWidgetId = await page
    .locator(".editor-canvas[data-canvas='canvas-1'] .editor-widget-hit.selected")
    .getAttribute("data-widget");
  expect(draggedWidgetId).toBeTruthy();
  const handle = page.locator(
    `.editor-canvas[data-canvas='canvas-1'] .editor-widget-hit[data-widget='${draggedWidgetId}']`,
  );
  const slot = frame.locator(`.widget-slot[data-widget-id='${draggedWidgetId}']`);
  await expect.poll(async () => [await handle.count(), await slot.count()]).toEqual([1, 1]);
  await expect(slot).toBeVisible();
  const connectionsBeforeRestart = canvasConnections.filter(({ url }) =>
    url.includes("/ws/canvas-1")).length;
  const statesBeforeRestart = canvasStates.length;
  const presentationsBeforeRestart = canvasPresentations.length;
  const replicaLoadsBeforeRestart = replicaLifecycle.filter(({ phase, url }) =>
    phase === "loaded" && url.includes("/canvas/canvas-1")).length;
  const replicaURLBeforeRestart = await page.locator("#scorepeek-replica-canvas-1")
    .getAttribute("src");
  rejectedCanvasConnections = 2;
  const activeCanvasConnection = canvasConnections.findLast(({ server, url }) =>
    server && url.includes("/ws/canvas-1"));
  expect(activeCanvasConnection).toBeDefined();
  await activeCanvasConnection.client.close({ code: 1012, reason: "backend restarting" });
  await expect.poll(() => canvasConnections.filter(({ url }) =>
    url.includes("/ws/canvas-1")).length, { timeout: 10_000 })
    .toBeGreaterThanOrEqual(connectionsBeforeRestart + 3);
  await expect.poll(() => canvasStates.length, { timeout: 10_000 })
    .toBeGreaterThan(statesBeforeRestart);
  await expect.poll(() => canvasPresentations.length, { timeout: 10_000 })
    .toBeGreaterThan(presentationsBeforeRestart);
  await expect(page.locator("#scorepeek-replica-canvas-1")).toHaveAttribute(
    "src",
    replicaURLBeforeRestart,
  );
  expect(replicaLifecycle.filter(({ phase, url }) =>
    phase === "loaded" && url.includes("/canvas/canvas-1")).length)
    .toBe(replicaLoadsBeforeRestart);
  await expect(slot).toBeVisible();
  const unrelatedSameCanvas = frame.locator(
    `.widget-slot:not([data-widget-id='${draggedWidgetId}'])`,
  ).first();
  const unrelatedOtherCanvas = unrelatedFrame.locator(".widget-slot").first();
  const unrelatedBefore = {
    sameCanvas: await unrelatedSameCanvas.boundingBox(),
    otherCanvas: await unrelatedOtherCanvas.boundingBox(),
  };
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
  expect(await unrelatedSameCanvas.boundingBox()).toEqual(unrelatedBefore.sameCanvas);
  expect(await unrelatedOtherCanvas.boundingBox()).toEqual(unrelatedBefore.otherCanvas);
  await page.waitForTimeout(400);
  expect(await slot.boundingBox()).toEqual(expected);
  const dragUpdates = await page.evaluate((start) => window.__scorepeekTest.draftUpdates.slice(start), updateCountBeforeDrag);
  expect(dragUpdates).toHaveLength(1);
  const persistedCanvas = dragUpdates[0].request.canvases.find((candidate) => candidate.id === "canvas-1");
  const persistedWidget = persistedCanvas.widgets.find(({ id }) => id === draggedWidgetId);
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
  const draggedWidgetIndex = persistedCanvas.widgets.findIndex(({ id }) => id === draggedWidgetId);
  expect(draggedWidgetIndex).toBeGreaterThanOrEqual(0);
  expect(new Set(dragReplicaRevisions.map(({ geometry }) => JSON.stringify(geometry[draggedWidgetIndex]))).size)
    .toBeGreaterThanOrEqual(8);
  expect(replicaRevisions.at(-1).geometry[draggedWidgetIndex]).toEqual(persistedGeometry);

  const firstDragEnd = await handle.boundingBox();
  delayNextStageDraftReply = true;
  await page.mouse.move(firstDragEnd.x + firstDragEnd.width / 2, firstDragEnd.y + firstDragEnd.height / 2);
  await page.mouse.down();
  for (let step = 1; step <= 4; step += 1) {
    await page.mouse.move(
      firstDragEnd.x + firstDragEnd.width / 2 + (40 * step) / 4,
      firstDragEnd.y + firstDragEnd.height / 2 + (24 * step) / 4,
    );
    await page.waitForTimeout(10);
  }
  await page.mouse.up();
  const delayedFirstExpected = {
    x: firstDragEnd.x + 40,
    y: firstDragEnd.y + 24,
    width: firstDragEnd.width,
    height: firstDragEnd.height,
  };
  await expect.poll(async () => handle.boundingBox()).toEqual(delayedFirstExpected);
  const secondDragStart = await handle.boundingBox();
  await page.mouse.move(secondDragStart.x + secondDragStart.width / 2, secondDragStart.y + secondDragStart.height / 2);
  await page.mouse.down();
  for (let step = 1; step <= 4; step += 1) {
    await page.mouse.move(
      secondDragStart.x + secondDragStart.width / 2 + (20 * step) / 4,
      secondDragStart.y + secondDragStart.height / 2 + (12 * step) / 4,
    );
    await page.waitForTimeout(10);
  }
  await page.waitForTimeout(600);
  await page.mouse.move(secondDragStart.x + secondDragStart.width / 2 + 64, secondDragStart.y + secondDragStart.height / 2 + 40);
  await page.mouse.up();
  const consecutiveExpected = {
    x: firstDragEnd.x + 40 + 64,
    y: firstDragEnd.y + 24 + 40,
    width: firstDragEnd.width,
    height: firstDragEnd.height,
  };
  await expect.poll(async () => handle.boundingBox()).toEqual(consecutiveExpected);
  await expect.poll(async () => slot.boundingBox()).toEqual(consecutiveExpected);
  await expect.poll(() => {
    const presentation = canvasPresentations.findLast(
      (candidate) => candidate.canvas.id === "canvas-1",
    );
    const widget = presentation?.widgets.find(({ id }) => id === draggedWidgetId);
    return widget && [widget.x, widget.y, widget.width, widget.height];
  }).toEqual([
    consecutiveExpected.x,
    consecutiveExpected.y,
    consecutiveExpected.width,
    consecutiveExpected.height,
  ]);
  expect(delayedStageDraftReplies).toBe(1);

  await widgetPicker.click();
  await widgetPicker.press("End");
  await widgetPicker.press("Enter");

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
  await appearance
    .locator(".skin-property[data-property='background'] .property-option[data-value='static']")
    .click();
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
  const stageConnectionsBeforeReload = stageConnections.length;
  await page.reload();
  await expect.poll(() => stageConnections.length).toBe(stageConnectionsBeforeReload + 1);
  stageConnections.at(-1).release();
  await expect(frame.locator(".canvas-background-art")).toBeVisible();
  await expect(page.locator(".editor-panel")).toHaveCount(0);

  await page.locator("#stage").click({ button: "right", position: { x: 10, y: 10 } });
  await page.getByRole("button", { name: "Canvas 1", exact: true }).click();
  for (const [field, value] of [["x", "2001"], ["y", "-101"], ["width", "701"]]) {
    const input = page.locator(`[id='canvas-1:${field}']`);
    await expect(input).toBeVisible({ timeout: 3000 });
    await input.fill(value);
    await input.blur();
    await expect(input).toHaveValue(value);
  }
  await expect(page.getByRole("button", { name: "Save & Close", exact: true })).toBeEnabled();
  await page.getByRole("button", { name: "Save & Close", exact: true }).click();
  await expect(page.locator(".editor-panel")).toHaveCount(0);

  await page.setViewportSize({ width: 800, height: 600 });
  await page.reload();
  await expect.poll(() => stageConnections.length).toBe(stageConnectionsBeforeReload + 2);
  stageConnections.at(-1).release();
  await page.locator("#stage").click({ button: "right", position: { x: 10, y: 10 } });
  await page.getByRole("button", { name: "Canvas 1", exact: true }).click();
  const offscreenX = page.locator("[id='canvas-1:x']");
  await expect(offscreenX).toHaveValue("2001");
  await expect(page.locator("[id='canvas-1:y']")).toHaveValue("-101");
  await offscreenX.fill("-7");
  await offscreenX.press("Enter");
  const offscreenWidth = page.locator("[id='canvas-1:width']");
  await offscreenWidth.fill("31");
  await expect(page.locator(".save-action")).toBeDisabled();
  await offscreenWidth.fill("701");
  await offscreenWidth.press("Enter");
  await page.locator(".canvas-select[data-canvas-id='canvas-1'] .widget-row .navigator-item-select").first().click();
  const offscreenWidgetX = page.locator(".object-inspector input[id^='canvas-1:'][id$=':x']");
  await expect(offscreenWidgetX).toBeVisible();
  await offscreenWidgetX.fill("-19");
  await offscreenWidgetX.press("Enter");
  await page.getByRole("button", { name: "Canvas 1", exact: true }).click();
  await expect(offscreenX).toHaveValue("-7");
  await expect(page.getByRole("button", { name: "Save & Close", exact: true })).toBeEnabled();
  await page.getByRole("button", { name: "Save & Close", exact: true }).click();
  await expect(page.locator(".editor-panel")).toHaveCount(0);
  expect(delayedCanvasMessages).toBeGreaterThan(0);
  expect(pageErrors).toEqual([]);
});
