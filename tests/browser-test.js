import { chromium } from "playwright";

export function browserTest(
  name,
  scenario,
  { timeoutMs = 30_000, page = true } = {},
) {
  Deno.test(name, async () => {
    const browser = await chromium.launch({
      headless: true,
      ...(Deno.env.get("SCOREPEEK_PLAYWRIGHT_CHROMIUM")
        ? { executablePath: Deno.env.get("SCOREPEEK_PLAYWRIGHT_CHROMIUM") }
        : {}),
    });
    let timer;
    try {
      const target = page
        ? { browser, page: await browser.newPage() }
        : { browser };
      await Promise.race([
        scenario(target),
        new Promise((_, reject) => {
          timer = setTimeout(
            () =>
              reject(
                new Error(`browser scenario timed out after ${timeoutMs} ms`),
              ),
            timeoutMs,
          );
        }),
      ]);
    } finally {
      clearTimeout(timer);
      await browser.close();
    }
  });
}
