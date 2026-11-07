// Junction codes indoors (server: indoor.rs; keep both in step). A screen
// computes its code from the junction's secret: `<node>-<window>-<mac>`,
// node and window in base 36, mac the first 6 bytes of
// HMAC-SHA256(secret, "w<window>") in base 32. @noble/hashes, not WebCrypto:
// screens may open the page from a file or over plain http.
import { hmac } from "@noble/hashes/hmac.js";
import { sha256 } from "@noble/hashes/sha2.js";

const B32 = "abcdefghijklmnopqrstuvwxyz234567";

export function unhex(h: string): Uint8Array {
  const out = new Uint8Array(h.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(h.slice(2 * i, 2 * i + 2), 16);
  return out;
}

export function mac(secret: Uint8Array, msg: string): string {
  const h = hmac(sha256, secret, new TextEncoder().encode(msg));
  let v = 0n;
  for (let i = 0; i < 6; i++) v = (v << 8n) | BigInt(h[i]);
  let s = "";
  for (let i = 0; i < 10; i++) {
    s = B32[Number(v & 31n)] + s;
    v >>= 5n;
  }
  return s;
}

export const windowOf = (nowMs: number, windowS: number) => Math.floor(nowMs / 1000 / Math.max(1, windowS));

export const screenCode = (secret: Uint8Array, node: number, w: number) =>
  `${node.toString(36)}-${w.toString(36)}-${mac(secret, `w${w}`)}`;

/** The game link a code goes in (what a phone's camera opens too). */
export const codeLink = (base: string, region: string, code: string) =>
  `${base.replace(/\/$/, "")}/?region=${encodeURIComponent(region)}&q=${code}`;

/** The code from a scanned text: a game link with `q=`, or the bare code. */
export function codeFromScan(text: string): string | null {
  const t = text.trim();
  const i = t.indexOf("q=");
  const code = i >= 0 ? t.slice(i + 2).split(/[&#]/)[0] : t;
  return /^[a-z0-9-]{5,40}$/.test(code) ? code : null;
}
