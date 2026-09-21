const fs = require("node:fs");
const path = require("node:path");
const { test, expect } = require("playwright/test");

const siteRoot = path.resolve(process.env.SCOREPEEK_SITE_ROOT);
const captureRoot = process.env.SCOREPEEK_SITE_CAPTURE_DIR;
const executablePath = process.env.SCOREPEEK_PLAYWRIGHT_CHROMIUM;

if (executablePath) {
  test.use({ launchOptions: { executablePath } });
}

const contentTypes = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".png", "image/png"],
]);

async function newSitePage(browser, locale, viewport) {
  const context = await browser.newContext({ locale, viewport });
  await context.route("http://scorepeek.test/**", async (route) => {
    const url = new URL(route.request().url());
    let pathname = decodeURIComponent(url.pathname);
    if (pathname.endsWith("/")) pathname += "index.html";
    const file = path.resolve(siteRoot, `.${pathname}`);
    if (!file.startsWith(`${siteRoot}${path.sep}`)) {
      await route.fulfill({ status: 403, body: "forbidden" });
      return;
    }
    try {
      await route.fulfill({
        status: 200,
        contentType: contentTypes.get(path.extname(file)) || "application/octet-stream",
        body: await fs.promises.readFile(file),
      });
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
      await route.fulfill({ status: 404, body: "not found" });
    }
  });
  const page = await context.newPage();
  return { context, page };
}

test("the root routes Japanese and English browser locales", async ({ browser }) => {
  for (const [locale, suffix] of [["ja-JP", "/ja/"], ["en-US", "/en/"]]) {
    const { context, page } = await newSitePage(browser, locale, { width: 1280, height: 800 });
    await page.goto("http://scorepeek.test/");
    await expect(page).toHaveURL(`http://scorepeek.test${suffix}`);
    await expect(page.locator("h1")).toBeVisible();
    await context.close();
  }
});

for (const specification of [
  {
    locale: "ja-JP",
    path: "/ja/",
    headings: ["IIDXのリザルトを自動記録。", "overlayで成長を可視化。"],
    how: "プレイから、記録、表示まで。",
    languageLink: "../en/",
    capture: "landing-ja",
  },
  {
    locale: "en-US",
    path: "/en/",
    headings: ["Automatic IIDX score tracking.", "Progress, visualized in your overlay."],
    how: "From play, to record, to display.",
    languageLink: "../ja/",
    capture: "landing-en",
  },
]) {
  test(`${specification.path} renders the complete responsive landing page`, async ({ browser }) => {
    const { context, page } = await newSitePage(browser, specification.locale, {
      width: 1440,
      height: 1000,
    });
    const pageErrors = [];
    page.on("pageerror", (error) => pageErrors.push(error.message));

    await page.goto(`http://scorepeek.test${specification.path}`);
    await expect(page.locator("h1 > span")).toHaveText(specification.headings);
    await expect(page.locator("#how h2")).toHaveText(specification.how);
    await expect(page.locator(".skin-card")).toHaveCount(3);
    await expect(page.locator(".technology-node")).toHaveCount(4);
    await expect(page.locator(`.language-switch a[href="${specification.languageLink}"]`).first()).toBeVisible();
    await expect(page.locator('a[href="https://github.com/atty303/scorepeek"]')).toHaveCount(3);
    await expect.poll(() => page.locator(".product-frame img").evaluate((image) => image.naturalWidth)).toBe(640);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);
    expect(pageErrors).toEqual([]);

    if (captureRoot) {
      fs.mkdirSync(captureRoot, { recursive: true });
      await page.screenshot({ path: path.join(captureRoot, `${specification.capture}-desktop.png`), fullPage: true });
    }

    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.locator("h1")).toBeVisible();
    await expect(page.locator(".product-frame img")).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)).toBe(true);

    if (captureRoot) {
      await page.screenshot({ path: path.join(captureRoot, `${specification.capture}-mobile.png`), fullPage: true });
    }

    await context.close();
  });
}
