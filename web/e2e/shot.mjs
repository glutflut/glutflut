// Dev check: open the app headless, print console output, save a screenshot.
//   node e2e/shot.mjs "<url>" out.png [waitMs]
import { chromium } from "playwright";
import { tmpdir } from "node:os";

const [url = "http://127.0.0.1:5173/?sim&at=47.66038,9.17639", out = `${tmpdir()}/elements-shot.png`, wait = "8000"] =
  process.argv.slice(2);

const browser = await chromium.launch({ args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader"] });
const page = await browser.newPage({ viewport: { width: 412, height: 860 }, deviceScaleFactor: 2 });
page.on("console", (m) => console.log(`[${m.type()}] ${m.text()}`));
page.on("pageerror", (e) => console.log(`[pageerror] ${e.message}`));
await page.goto(url);
await page.waitForTimeout(Number(wait));
await page.screenshot({ path: out });
await browser.close();
