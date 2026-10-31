// "Wie ist es hier?" (rate.ts): after the second claim the app asks by itself
// (a card, with the city's question first if one runs there); one tap answers.
// Then the mood map button. Screenshots at 360x640.
//   node e2e/rate.mjs [base, default http://127.0.0.1:5173] [out dir]
// Server: ELEMENTS_ALLOW_SIM=1, ELEMENTS_POW_BITS=0, ELEMENTS_RULES_OVERRIDE='claim_cooldown_s=1'.
import { chromium } from "playwright";
import { tmpdir } from "node:os";
import { AT, REGION } from "./where.mjs";

const [base = "http://127.0.0.1:5173", out = tmpdir()] = process.argv.slice(2);
let failed = 0;
const check = (ok, what, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}${detail ? ` (${detail})` : ""}`);
  if (!ok) failed++;
};

const browser = await chromium.launch({ args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader"] });
const ctx = await browser.newContext({ viewport: { width: 360, height: 640 }, deviceScaleFactor: 2 });
const page = await ctx.newPage();
page.on("pageerror", (e) => console.log(`[pageerror] ${e.message}`));
await page.goto(`${base}/?sim&at=${AT}&region=${REGION}`);
await page.waitForFunction(() => document.getElementById("me-sub")?.textContent?.match(/\d/), null, { timeout: 60_000 });
// The first-visit card goes first.
if (await page.isVisible("#intro")) await page.click("#intro-ok");
await page.waitForTimeout(500);

const claim = async () => {
  await page.waitForFunction(() => !(document.getElementById("claim")).disabled, null, { timeout: 15_000 });
  await page.click("#claim");
  await page.waitForTimeout(1300);
  return page.textContent("#toast");
};
// Two claims that take (a spot may run out of junctions to take).
let took = 0;
for (let i = 0; i < 6 && took < 2; i++) {
  const msg = await claim();
  console.log(`claim ${i + 1}:`, msg);
  if (!/\+\d/.test(msg ?? "")) continue;
  took++;
  if (took === 1) check(await page.isHidden("#sheet"), "no card after the first claim");
}
check(took === 2, "two claims took", String(took));
// A story card (the side card after joining) holds the question back until closed.
while (await page.isVisible("#story")) await page.click("#story-close");
await page.waitForTimeout(1500);
const asked = await page.evaluate(() => {
  const s = document.getElementById("sheet");
  return !s.hidden && s.classList.contains("asked");
});
check(asked, "card after the second claim");
const q = await page.evaluate(() => (document.getElementById("sheet-q").hidden ? null : document.getElementById("sheet-q-title").textContent));
console.log("city question:", q ?? "(none here)");
await page.screenshot({ path: `${out}/rate-card.png` });
const box = await page.locator("#sheet").boundingBox();
const bar = await page.locator("#actions").boundingBox();
console.log("card", box, "actions", bar);
const btns = await page.locator("#claim").boundingBox();
check(box && box.y >= 0 && box.y + box.height <= btns.y, "card fits above the buttons", `${Math.round(box.y + box.height)} <= ${Math.round(btns.y)}`);
await page.click(q ? "#sheet-q-chips button" : "#sheet button[data-cat]");
await page.waitForTimeout(800);
// The thanks may give way to a streak note at once; the server's rows are checked by the caller.
console.log("after the answer:", await page.textContent("#toast"));
check(await page.isHidden("#sheet"), "card gone after the answer");

while (await page.isVisible("#story")) await page.click("#story-close");
await page.click("#nav-more");
await page.waitForTimeout(300);
await page.click("#nav-mood");
await page.waitForTimeout(1500);
console.log("mood:", await page.textContent("#toast"));
await page.screenshot({ path: `${out}/rate-mood.png` });
await browser.close();
console.log(failed ? `${failed} failed` : "all ok");
process.exit(failed ? 1 : 0);
