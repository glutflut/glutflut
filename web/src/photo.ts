// A picture of the game as the app draws it, without its UI (posts, editor):
// the same map, layers and live state as the game view, watched like a
// visitor (`?watch`), or a chapter's end from its snapshot. Parameters:
//   region, lat, lon, width_m   the frame (its width at the centre)
//   layers   comma list: 3d, flow, sources, natural, areas, clouds, radar, labels
//   chapter  its end instead of now (editor login: /api/ops/nodes)
//   title, note   texts on the picture, as cards like the app's
// The JPEG (1080×1350) goes to the opener (`postMessage` {t: "photo", blob}
// or {t: "photo", error}) and is shown when the page is opened alone.
import * as maplibregl from "maplibre-gl";
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url";
import { Net, type ServerMsg } from "./net";
import { TileLayer, loadMeta } from "./tiles";
import { FlowLayer } from "./flow";
import { createMap } from "./layers";
import { setupWorld } from "./world";
import { setupSources } from "./sources";
import { setupRadar } from "./radar";
import { withRegion } from "./region";

maplibregl.setWorkerUrl(workerUrl);

/** CSS size; the picture is twice that (Instagram 4:5). */
const W = 540;
const H = 675;
const DPR = 2;
const BG = "#0b0e13";

const q = new URLSearchParams(location.search);
const layers = new Set((q.get("layers") ?? "flow,sources,natural,areas").split(",").filter(Boolean));
const num = (k: string) => (q.has(k) ? Number(q.get(k)) : NaN);

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** The app's zoom for `width_m` across W CSS px (MapLibre's 512 px tiles). */
const zoomFor = (lat: number, widthM: number) => Math.log2((78_271.5 * Math.cos((lat * Math.PI) / 180) * W) / widthM);

async function shoot(): Promise<{ blob: Blob; credit: string }> {
  const meta = await loadMeta(() => {});
  const lat = Number.isFinite(num("lat")) ? num("lat") : (meta.hotspot ?? meta.region.centroid)[1];
  const lon = Number.isFinite(num("lon")) ? num("lon") : (meta.hotspot ?? meta.region.centroid)[0];
  const widthM = Number.isFinite(num("width_m")) ? Math.min(5000, Math.max(200, num("width_m"))) : 900;
  const box = document.getElementById("map")!;
  Object.assign(box.style, { width: `${W}px`, height: `${H}px` });
  const full = layers.has("3d");
  const map = await createMap(meta, true, full, {
    center: [lon, lat],
    zoom: zoomFor(lat, widthM),
    pitch: full ? 35 : 0,
    bearing: 0,
    bounds: undefined,
    interactive: false,
    pixelRatio: DPR,
    canvasContextAttributes: { preserveDrawingBuffer: true, antialias: true },
    fadeDuration: 0,
  });

  // Only the game's picture: no claim range; the rest by choice.
  const hide = (id: string, off: boolean) => off && map.getLayer(id) && map.setLayoutProperty(id, "visibility", "none");
  hide("range", true);
  hide("range-line", true);
  hide("natural", !layers.has("natural"));
  hide("blocks", !layers.has("areas"));
  hide("blocks-players", !layers.has("areas"));
  if (!layers.has("labels"))
    for (const l of map.getStyle().layers) if (l.type === "symbol" && !["natural", "mottos"].includes(l.id)) hide(l.id, true);

  let flow: FlowLayer | null = null;
  const world = setupWorld(map, () => flow, layers.has("areas") ? (meta.area ?? null) : null);
  let lastView: [number, number][] = [];
  let net: Net | null = null;
  const tiles = new TileLayer(
    map,
    meta,
    () => {},
    (gone) => world.drop(gone),
    (view) => {
      lastView = view;
      net?.send({ t: "view", tiles: view });
    },
  );
  tiles.onBlocks = (ids) => world.blocks(ids, tiles);
  tiles.blockProps = world.blockProps;
  if (layers.has("flow")) {
    flow = new FlowLayer(tiles);
    map.addLayer(flow, "nodes");
  }
  const sources = layers.has("sources") ? setupSources(map) : null;
  if (!sources) hide("mottos", true);

  await tiles.refresh();
  if (tiles.tooMany) throw new Error("frame too large: too many streets for one picture");

  // The state: a chapter's end, or now as a visitor sees it.
  const chapter = q.get("chapter");
  if (chapter) {
    const res = await fetch(withRegion(`/api/ops/nodes?chapter=${encodeURIComponent(chapter)}`));
    if (!res.ok) throw new Error(`chapter snapshot: ${res.status}`);
    const s = (await res.json()) as { fire: number[]; water: number[]; front: number[] };
    world.apply(
      [...s.fire.map((id): [number, number, number] => [id, 0.7, 0]), ...s.water.map((id): [number, number, number] => [id, 0, 0.7]), ...s.front.map((id): [number, number, number] => [id, 0.4, 0.4])],
      tiles,
    );
  } else {
    // Ready once the messages after the view have stopped for a moment.
    let last = 0;
    let welcomed = false;
    net = new Net(
      (m: ServerMsg) => {
        last = Date.now();
        if (m.t === "welcome") {
          welcomed = true;
          if (lastView.length) net?.send({ t: "view", tiles: lastView });
        } else if (m.t === "state") world.apply(m.nodes, tiles);
        else if (m.t === "areas") world.areas(m.player, m.mine, tiles);
        else if (m.t === "sources") sources?.set(m.items, new Set());
        else if (m.t === "sourcesdiff") sources?.apply(m.set, m.gone, new Set());
      },
      () => {},
    );
    const t0 = Date.now();
    while (!(welcomed && Date.now() - last > 1500) && Date.now() - t0 < 15_000) await wait(200);
    if (!welcomed) throw new Error("no connection to the game");
  }

  const credits: Record<string, string> = {};
  if (layers.has("clouds") || layers.has("radar"))
    await setupRadar(map, () => {}, { on: { clouds: layers.has("clouds"), radar: layers.has("radar") }, credits });

  // Map tiles loaded and drawn (the flow keeps repainting, so no `idle`).
  sources?.draw();
  const t1 = Date.now();
  for (let calm = 0; calm < 3 && Date.now() - t1 < 20_000; ) {
    await wait(250);
    calm = map.loaded() && map.areTilesLoaded() ? calm + 1 : 0;
  }
  map.triggerRepaint();
  await new Promise((r) => map.once("render", r));

  const credit = ["© OpenStreetMap", "Protomaps", ...Object.values(credits).filter(Boolean)].join(" · ");
  const canvas = await compose(map, box, credit);
  const blob = await new Promise<Blob | null>((r) => canvas.toBlob(r, "image/jpeg", 0.9));
  if (!blob) throw new Error("no picture");
  return { blob, credit };
}

/** The map, the source markers (DOM, outside the map's canvas) and the texts. */
async function compose(map: maplibregl.Map, box: HTMLElement, credit: string): Promise<HTMLCanvasElement> {
  const c = document.createElement("canvas");
  c.width = W * DPR;
  c.height = H * DPR;
  const g = c.getContext("2d")!;
  g.fillStyle = BG;
  g.fillRect(0, 0, c.width, c.height);
  g.drawImage(map.getCanvas(), 0, 0, c.width, c.height);
  // Read the markers first (their images decode asynchronously), then draw in order.
  const at = box.getBoundingClientRect();
  const rect = (e: Element) => {
    const r = e.getBoundingClientRect();
    return [(r.left - at.left) * DPR, (r.top - at.top) * DPR, r.width * DPR, r.height * DPR] as const;
  };
  const marks = [...box.querySelectorAll<HTMLElement>(".source")].map((el) => ({
    el,
    svgs: [...el.querySelectorAll("svg")].map((svg) => ({ at: rect(svg), img: svgImage(svg) })),
  }));
  await Promise.all(marks.flatMap((m) => m.svgs.map((s) => s.img.decode().catch(() => {}))));
  for (const { el, svgs } of marks) {
    const glow = el.querySelector<HTMLElement>(".src-glow");
    if (glow) {
      const [x, y, w] = rect(glow);
      const rgb = getComputedStyle(el).getPropertyValue("--glow").trim() || "255, 122, 40";
      const op = Number(getComputedStyle(glow).opacity) || 0.8;
      const grad = g.createRadialGradient(x + w / 2, y + w / 2, 0, x + w / 2, y + w / 2, w / 2);
      grad.addColorStop(0, `rgba(${rgb}, ${0.75 * op})`);
      grad.addColorStop(0.38, `rgba(${rgb}, ${0.3 * op})`);
      grad.addColorStop(0.7, `rgba(${rgb}, 0)`);
      g.fillStyle = grad;
      g.fillRect(x, y, w, w);
    }
    g.save();
    g.shadowColor = "rgba(255, 255, 255, 0.35)";
    g.shadowBlur = 6 * DPR;
    for (const { at: [x, y, w, h], img } of svgs) if (img.complete && img.naturalWidth) g.drawImage(img, x, y, w, h);
    g.restore();
  }
  texts(g, q.get("title") ?? "", q.get("note") ?? "", credit);
  return c;
}

/** An SVG element as it stands now, as an image. */
function svgImage(svg: SVGSVGElement): HTMLImageElement {
  const copy = svg.cloneNode(true) as SVGSVGElement;
  copy.setAttribute("xmlns", "http://www.w3.org/2000/svg");
  const img = new Image();
  img.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(new XMLSerializer().serializeToString(copy))}`;
  return img;
}

/** Title and note as dark cards (like the app's), the credit small at the bottom. */
function texts(g: CanvasRenderingContext2D, title: string, note: string, credit: string) {
  const font = 'system-ui, -apple-system, "Segoe UI", sans-serif';
  const card = (x: number, y: number, w: number, h: number) => {
    g.fillStyle = "rgba(11, 14, 19, 0.82)";
    g.strokeStyle = "rgba(255, 255, 255, 0.12)";
    g.lineWidth = 2;
    g.beginPath();
    g.roundRect(x, y, w, h, 18);
    g.fill();
    g.stroke();
  };
  const cw = W * DPR;
  const ch = H * DPR;
  if (title.trim()) {
    g.font = `700 52px ${font}`;
    const tw = Math.min(cw - 80, g.measureText(title).width + 64);
    card((cw - tw) / 2, 56, tw, 92);
    g.fillStyle = "#f1f3f5";
    g.textAlign = "center";
    g.textBaseline = "middle";
    g.fillText(title, cw / 2, 102, cw - 120);
  }
  const lines = note.split("\n").map((l) => l.trim()).filter(Boolean);
  if (lines.length) {
    g.font = `400 32px ${font}`;
    const lh = 42;
    const w = Math.min(cw - 120, Math.max(...lines.map((l) => g.measureText(l).width)) + 48);
    const h = lines.length * lh + 30;
    const y = ch - 72 - h;
    card(64, y, w, h);
    g.fillStyle = "#f1f3f5";
    g.textAlign = "left";
    g.textBaseline = "alphabetic";
    lines.forEach((l, i) => g.fillText(l, 88, y + 15 + lh * (i + 0.78), w - 48));
  }
  g.font = `400 17px ${font}`;
  g.fillStyle = "rgba(200, 206, 214, 0.85)";
  g.textAlign = "right";
  g.textBaseline = "alphabetic";
  g.fillText(credit, cw - 22, ch - 16, cw - 44);
}

const done = (msg: { t: "photo"; blob?: Blob; credit?: string; error?: string }) => {
  if (window.parent !== window) window.parent.postMessage(msg, location.origin);
  else if (msg.blob) {
    const img = new Image();
    img.src = URL.createObjectURL(msg.blob);
    img.id = "out";
    img.style.width = `${W}px`;
    document.body.append(img);
  } else document.body.append(msg.error ?? "error");
  // For a headless browser later: the result on the page.
  Object.assign(window, { __photo: msg });
};

shoot()
  .then(({ blob, credit }) => done({ t: "photo", blob, credit }))
  .catch((e: unknown) => done({ t: "photo", error: e instanceof Error ? e.message : String(e) }));
