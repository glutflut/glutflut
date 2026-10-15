// Help and legal pages open over the game and close without a reload: the
// tapped position stays, back (link or button) closes, and on test servers
// a reload continues at the tapped position. Test server (?sim) only.
//   node e2e/pages.mjs [base] [shots-dir]
import { chromium } from "playwright";
import { AT, REGION } from "./where.mjs";

const [base = "http://127.0.0.1:5080", S = ""] = process.argv.slice(2);
let failed = 0;
const b = await chromium.launch();
const p = await (await b.newContext({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, locale: "de-DE" })).newPage();
p.on("pageerror", (e) => console.log("[pageerror]", e.message));
const check = (ok, what) => {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}`);
  if (!ok) failed++;
};
await p.goto(`${base}/?sim&region=${REGION}&at=${AT}`);
await p.waitForFunction(() => /Punkte/.test(document.getElementById("me-sub")?.textContent ?? ""), null, { timeout: 60000 });
await p.locator("#intro-ok").click().catch(() => {});
await p.evaluate(() => (window.__marker = 42));
// Move by tapping the map.
await p.mouse.click(250, 450);
await p.waitForTimeout(800);
const at1 = new URL(p.url()).searchParams.get("at");
check(at1 !== AT, `tap moved the player, address has at=${at1}`);
await p.locator("#help").click();
await p.locator("#page").waitFor({ state: "visible" });
const f = p.frameLocator("#page-frame");
await f.locator("h1").first().waitFor();
await p.waitForTimeout(500);
if (S) await p.screenshot({ path: `${S}/page-info.png` });
check(true, `info in overlay: ${await f.locator("article:not([hidden]) h1").textContent()}`);
// Inside: to the legal notice, then "back to the game".
await f.locator('article:not([hidden]) a[href^="/impressum.html"]').first().click();
await p.waitForTimeout(800);
check((await p.locator("#page-frame").evaluate((fr) => fr.contentWindow.location.pathname)) === "/impressum.html", "legal notice opens in place");
await f.locator('article:not([hidden]) a[href^="/"]').first().click();
await p.waitForTimeout(500);
check(await p.locator("#page").isHidden(), "back to the game closes the overlay");
check((await p.evaluate(() => window.__marker)) === 42, "no reload");
check(new URL(p.url()).searchParams.get("at") === at1, "position kept");
// Phone back button closes it too.
await p.locator("#help").click();
await p.locator("#page").waitFor({ state: "visible" });
const len = await p.evaluate(() => history.length);
await p.evaluate(() => history.back());
await p.waitForTimeout(500);
check(await p.locator("#page").isHidden(), "back button closes the overlay");
console.log("  history length while open:", len);
check((await p.evaluate(() => window.__marker)) === 42, "still no reload");
// Footer link and close button.
await p.locator("#actions .legal a").nth(2).click();
await p.locator("#page").waitFor({ state: "visible" });
await p.frameLocator("#page-frame").locator('article:not([hidden]) a[href^="/"]').first().click();
await p.waitForTimeout(400);
check(await p.locator("#page").isHidden(), "privacy page from the footer, back link closes it");
// Reload (language, graphics) keeps the tapped position on test servers.
await p.reload();
await p.waitForFunction(() => /Punkte/.test(document.getElementById("me-sub")?.textContent ?? ""), null, { timeout: 60000 });
check(new URL(p.url()).searchParams.get("at") === at1, "reload continues at the tapped position");
await p.locator("#me").click();
await p.locator('.seg [data-pane="me"]').click();
await p.locator("#forget").click();
await p.locator("#ask-yes").click();
await p.waitForTimeout(500);
await b.close();
console.log(failed ? `${failed} failed` : "all ok");
process.exit(failed ? 1 : 0);
