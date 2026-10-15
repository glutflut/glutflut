// Proof of work for a new identity (server pow.rs): find n so that
// SHA-256("<challenge>:<n>") starts with `bits` zero bits. In a worker, so the
// page stays responsive (about a second on a phone).
import { sha256 } from "@noble/hashes/sha2.js";

const enc = new TextEncoder();

function zeroBits(h: Uint8Array): number {
  let n = 0;
  for (const b of h) {
    if (b === 0) {
      n += 8;
      continue;
    }
    return n + Math.clz32(b) - 24;
  }
  return n;
}

self.onmessage = (e: MessageEvent<{ c: string; bits: number }>) => {
  const { c, bits } = e.data;
  for (let n = 0; ; n++) {
    if (zeroBits(sha256(enc.encode(`${c}:${n}`))) >= bits) {
      self.postMessage(String(n));
      return;
    }
  }
};
