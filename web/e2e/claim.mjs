// Dev check: two players; the second claims repeatedly and screenshots.
//   node e2e/claim.mjs [base] [out.png]
import { chromium } from "playwright";
import { tmpdir } from "node:os";
import { AT, REGION } from "./where.mjs";

const [base = "http://127.0.0.1:5173", out = `${tmpdir()}/elements-claim.png`] = process.argv.slice(2);

const browser = await chromium.launch({ args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader"] });
const open = async () => {
  const ctx = await browser.newContext({ viewport: { width: 412, height: 860 }, deviceScaleFactor: 2 });
  const page = await ctx.newPage();
  page.on("pageerror", (e) => console.log(`[pageerror] ${e.message}`));
  await page.goto(`${base}/?sim&at=${AT}&region=${REGION}`);
  await page.waitForFunction(() => document.getElementById("me-sub")?.textContent?.includes("Punkte"));
  return page;
};

const a = await open();
const b = await open();
console.log("A:", await a.textContent("#me-label"), "| B:", await b.textContent("#me-label"));

for (let i = 0; i < 6; i++) {
  await b.click("#claim");
  await b.waitForTimeout(1300);
  console.log(`claim ${i + 1}:`, await b.textContent("#toast"), "| score", await b.textContent("#score"));
}
await b.waitForTimeout(4000);
await b.screenshot({ path: out });
await browser.close();
