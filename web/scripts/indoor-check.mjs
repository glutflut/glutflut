// Junction codes (src/indoor-code.ts) against the server's vectors (indoor.rs tests).
//   node scripts/indoor-check.mjs   (from web/)
import { runnerImport } from "vite";

const { module: M } = await runnerImport(new URL("../src/indoor-code.ts", import.meta.url).pathname, { configFile: false, logLevel: "error" });
const secret = M.unhex("1b9f1369bbdfa34c2352de083f6fca462180c34dd4a87013e4d48fc3f0f705f8");
const eq = (a, b) => {
  if (a !== b) throw new Error(`${a} != ${b}`);
};
eq(`${(42).toString(36)}-${M.mac(secret, "s")}`, "16-gbxyoghmce");
eq(M.screenCode(secret, 42, 1000), "16-rs-cb2cntiewg");
eq(M.codeFromScan("https://glutflut.com/?region=r&q=16-gbxyoghmce"), "16-gbxyoghmce");
eq(M.codeFromScan("https://example.com/"), null);
eq(M.windowOf(60_000, 30), 2);
console.log("indoor codes: ok");
