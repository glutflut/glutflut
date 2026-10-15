// Raw-socket tests as new players: answer the server's proof-of-work
// challenge (server pow.rs) by saying hello again with the solution.
import { createHash } from "node:crypto";

export function solve(c, bits) {
  for (let n = 0; ; n++) {
    const h = createHash("sha256").update(`${c}:${n}`).digest();
    let z = 0;
    for (const b of h) {
      if (b === 0) z += 8;
      else {
        z += Math.clz32(b) - 24;
        break;
      }
    }
    if (z >= bits) return String(n);
  }
}

/** Resend `hello` with the solution whenever the server asks for one. */
export function answerPow(ws, hello) {
  ws.addEventListener("message", (e) => {
    const m = JSON.parse(e.data);
    if (m.t === "pow") ws.send(JSON.stringify({ ...hello, pow: { c: m.c, n: solve(m.c, m.bits) } }));
  });
}
