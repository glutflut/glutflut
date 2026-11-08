// Indoor QR sheets (A4, each junction's fixed code) and screen links
// (knoten.html) from `GET /api/ops/indoor` (admin, same origin only: screen
// secrets). Used by indoor.html and the admin page; styles in indoor.css.
import qrcode from "qrcode-generator";
import { codeLink } from "./indoor-code";

type Node = { id: number; lat: number; lon: number; label: string; screen: boolean; code?: string; secret?: string };
export type IndoorReply = { region: string; base: string; window_s: number; hold_s: number; nodes: Node[] };

const esc = (s: string) => s.replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);
const qr = (text: string) => {
  const q = qrcode(0, "M");
  q.addData(text);
  q.make();
  return q.createSvgTag({ cellSize: 4, margin: 4, scalable: true });
};

/** Fills `sheets` with the A4 pages and `screens` (a list) with the screen
 * links; returns the summary line. */
export function renderIndoor(r: IndoorReply, sheets: HTMLElement, screens: HTMLElement): { sheets: number; screens: number; text: string } {
  const fixed = r.nodes.filter((n) => !n.screen && n.code);
  sheets.innerHTML = fixed
    .map(
      (n) => `<section class="sheet">
        <div class="kicker"><span class="f">Glut</span> &amp; <span class="w">Flut</span></div>
        <div class="name">${esc(n.label || `Knoten ${n.id}`)}</div>
        <div class="qr">${qr(codeLink(r.base, r.region, n.code!))}</div>
        <div class="how">Spielst du schon? In der App „Scannen“ tippen.<br>Neu? Mit der Kamera scannen und mitspielen.<br><small>Already playing? Tap “Scan” in the app. New? Scan with your camera.</small></div>
        <div class="id">${esc(r.region)} · ${n.id} · glutflut.com</div>
      </section>`,
    )
    .join("");
  const scr = r.nodes.filter((n) => n.screen && n.secret);
  screens.innerHTML = scr
    .map((n) => {
      const f = new URLSearchParams({ r: r.region, n: String(n.id), k: n.secret!, w: String(r.window_s), b: r.base, l: n.label || `Knoten ${n.id}` });
      const url = `${location.origin}/knoten.html#${f}`;
      return `<li><b>${esc(n.label || String(n.id))}</b>: <a href="${esc(url)}" target="_blank" rel="noopener">${esc(url)}</a></li>`;
    })
    .join("");
  const text = `${fixed.length} Aushänge, ${scr.length} Bildschirme · Code gilt ${r.window_s} s, man steht ${Math.round(r.hold_s / 60)} min am Knoten.`;
  return { sheets: fixed.length, screens: scr.length, text };
}
