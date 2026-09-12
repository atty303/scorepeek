const { test, expect } = require("playwright/test");

const baseURL = process.env.SCOREPEEK_BROWSER_TEST_URL;
const executablePath = process.env.SCOREPEEK_PLAYWRIGHT_CHROMIUM;

if (executablePath) {
  test.use({ launchOptions: { executablePath } });
}

test("editor replicas follow drag, stale websocket delivery, scroll, and lifecycle", async ({ page }) => {
  const pageErrors = [];
  let delayedCanvasMessages = 0;
  page.on("pageerror", (error) => pageErrors.push(error.message));
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
  await page.mouse.move(before.x + before.width / 2, before.y + before.height / 2);
  await page.mouse.down();
  await page.mouse.move(before.x + before.width / 2 + 140, before.y + before.height / 2 + 108);
  await page.mouse.up();

  const expected = { x: before.x + 140, y: before.y + 108, width: before.width, height: before.height };
  await expect.poll(async () => handle.boundingBox()).toEqual(expected);
  await expect.poll(async () => slot.boundingBox()).toEqual(expected);
  await page.waitForTimeout(400);
  expect(await slot.boundingBox()).toEqual(expected);

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
  await visibility.getByRole("button", { name: "None", exact: true }).click();
  await expect(visibility.locator(".editor-toggle.selected")).toHaveCount(0);
  await visibility.getByRole("button", { name: "All", exact: true }).click();
  await expect(visibility.locator(".editor-toggle.selected")).toHaveCount(6);

  const appearance = page
    .locator(".editor-accordion-heading[data-section='canvas:canvas-1:appearance']")
    .locator("..");
  const initialFrameURL = await page.locator("#scorepeek-replica-canvas-1").getAttribute("src");
  await appearance.locator(".skin-option").nth(1).click();
  await expect(page.locator("#scorepeek-replica-canvas-1")).not.toHaveAttribute(
    "src",
    initialFrameURL,
  );
  await expect(slot).toBeVisible();

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

  await page.getByRole("button", { name: "Canvas 1", exact: true }).click();
  await page.getByRole("button", { name: "Delete canvas", exact: true }).click();
  await expect(page.locator("#scorepeek-replica-canvas-1")).toHaveCount(0);
  await expect(page.locator("#scorepeek-replica-canvas-2")).toHaveCount(1);
  expect(delayedCanvasMessages).toBeGreaterThan(0);
  expect(pageErrors).toEqual([]);
});
