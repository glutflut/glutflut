// Ops page (indoor.html): A4 sheets with each junction's fixed code, and the
// links for screen junctions (knoten.html), from `GET /api/ops/indoor` (admin).
import qrcode from "qrcode-generator";
import { codeLink } from "./indoor-code";
import { authHeaders } from "./opsauth";

type Node = { id: number; lat: number; lon: number; label: string; screen: boolean; code?: string; secret?: string };
type Reply = { region: string; base: string; window_s: number; hold_s: number; nodes: Node[] };

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const esc = (s: string) => s.replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);
const qr = (text: string) => {
  const q = qrcode(0, "M");
  q.addData(text);
  q.make();
  return q.createSvgTag({ cellSize: 4, margin: 4, scalable: true });
};

$<HTMLInputElement>("region").value = new URLSearchParams(location.search).get("region") ?? "";
$("print").onclick = () => print();
$("form").onsubmit = async (e) => {
  e.preventDefault();
  $("msg").textContent = "";
  const region = $<HTMLInputElement>("region").value.trim();
  const res = await fetch(`/api/ops/indoor?region=${encodeURIComponent(region)}`, { headers: authHeaders($<HTMLInputElement>("token").value.trim()) }).catch(() => null);
  if (!res?.ok) return void ($("msg").textContent = `Fehler ${res?.status ?? "Netz"}: ${(await res?.text().catch(() => "")) ?? ""}`);
  const r = (await res.json()) as Reply;
  const sheets = r.nodes.filter((n) => !n.screen && n.code);
  $("sheets").innerHTML = sheets
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
  const screens = r.nodes.filter((n) => n.screen && n.secret);
  $("screens").hidden = !screens.length;
  $("screen-list").innerHTML = screens
    .map((n) => {
      const f = new URLSearchParams({ r: r.region, n: String(n.id), k: n.secret!, w: String(r.window_s), b: r.base, l: n.label || `Knoten ${n.id}` });
      const url = `${location.origin}/knoten.html#${f}`;
      return `<li><b>${esc(n.label || String(n.id))}</b>: <a href="${esc(url)}" target="_blank" rel="noopener">${esc(url)}</a></li>`;
    })
    .join("");
  $("msg").textContent = `${sheets.length} Aushänge, ${screens.length} Bildschirme · Code gilt ${r.window_s} s, man steht ${Math.round(r.hold_s / 60)} min am Knoten.`;
};
