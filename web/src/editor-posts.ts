// Redaktion: posts from the game (server: posts.rs). A post has a trigger (a
// time, a chapter's start or end, or by hand), texts per outcome and channel,
// and pictures: own uploads and/or the game state's map, in an order. When it is due the server makes a
// draft with the numbers filled in; here it is copied, posted by hand and
// marked, or sent from here to the channels the server has an account for
// (Mastodon, Instagram).
import { ask, clean, errorOf, fmtDate, fromLocal, mayLeave, toast, toLocal, watchForm } from "./editor-ui";
import { t, type EKey } from "./editor-i18n";
import * as maplibregl from "maplibre-gl";
import { basemapStyle } from "./basemap";
import { listRegions } from "./region";

/** A post's texts per outcome: `text` for all channels, a channel's own text, the alt text. */
type Texts = { text?: string; instagram?: string; mastodon?: string; alt?: string };
type Variant = "any" | "fire" | "water" | "tie";
type Card = { lat?: number; lon?: number; width_m: number; title: string; note: string; look?: "plan" | "game"; layers?: string[] };
/** Layers of the app's picture (photo.ts); the default ones ticked for a new post. */
const LAYERS = ["flow", "sources", "natural", "areas", "3d", "labels", "radar", "clouds"] as const;
const LAYERS_ON = ["flow", "sources", "natural", "areas"];
/** The frame as the server filled it in for a draft (`results.card`). */
type Shot = { lat: number; lon: number; width_m: number; title: string; note: string; look: string; layers: string[] | null; chapter: string | null };
/** A post's picture: the game state's map, or an own upload (made 4:5 by cropping or framing). */
type Pic = "card" | { id: number; alt: string; fit: "crop" | "frame" };
const PICS_MAX = 10;
const MASTODON_PICS = 4;
type Post = {
  id: number;
  title: string;
  trigger: "at" | "chapter_start" | "chapter_end" | "manual";
  at: number | null;
  chapter: string | null;
  offset_min: number;
  texts: Partial<Record<Variant, Texts>>;
  card: Card | null;
  channels: string[];
  by: string;
  tags: string[];
  location: Place | null;
  pictures: Pic[] | null;
};
type Place = { id: string; name: string };
type Run = {
  id: number;
  post: number;
  auto: boolean;
  created: number;
  state: "draft" | "sent" | "discarded";
  outcome: "fire" | "water" | "tie" | null;
  numbers: { fire: number; water: number } | null;
  texts: Texts;
  image: boolean;
  note: string | null;
  decided_by: string | null;
  decided: number | null;
  results: Record<string, { url?: string; error?: string; note?: string; sending?: number }> | null;
  /** Own pictures besides the map; the map's place among them. */
  extra: number;
  card_at: number | null;
};
export type ChapterRef = { key: string; names: Record<string, { title: string }>; starts: number; ends: number };
type Api = (path: string, init?: RequestInit) => Promise<Response>;

const VARIANTS: Variant[] = ["any", "fire", "water", "tie"];
const FIELDS = ["instagram", "mastodon", "alt"] as const;
const MAX = { text: 2200, instagram: 2200, mastodon: 500, alt: 1000 };
const CHANNELS = ["instagram", "mastodon"] as const;
/** The form's fields (a draft has only the channels and alt). */
const FORM = ["text", "instagram", "mastodon", "alt"] as const;
const LABEL: Record<string, EKey> = { instagram: "po.f.instagram", mastodon: "po.f.mastodon", alt: "po.f.alt" };

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const input = (id: string) => $<HTMLInputElement>(id);
const area = (id: string) => $<HTMLTextAreaElement>(id);
const el = <K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}) => Object.assign(document.createElement(tag), props);

let api: Api;
let region = "";
let primary = "de";
let chapters: ChapterRef[] = [];
let posts: Post[] = [];
let runs: Run[] = [];
/** Channels the server can send to for this region. */
let ready: string[] = [];
/** The Instagram account as the server sees it. */
let insta: { username: string | null; expires: number | null; error: string | null } | null = null;
let editing: Post | null = null;
/** The form's texts per variant (the shown one lives in the text areas). */
let texts: Partial<Record<Variant, Texts>> = {};
let shown: Variant = "any";
/** The region's hashtags, and the ones the form's post uses. */
let hashtags: { tag: string; preset: boolean }[] = [];
let picked: string[] = [];
/** The region's places for Instagram (preset: new posts start with it). */
let places: (Place & { preset: boolean })[] = [];
/** Pictures fetched through the token: a draft's map (`<run>`), a draft's n-th (`<run>:<n>`), an upload (`u<id>`). */
const pictures = new Map<string, string>();
/** The form's pictures in order. */
let pics: Pic[] = [];
/** The picker for the picture's area: the map moves under a fixed 4:5 frame. */
let map: maplibregl.Map | null = null;
let mapRegion = "";
/** Set while the code moves or sizes the map (not the user); also until placed. */
let placing = true;

const q = () => `region=${encodeURIComponent(region)}`;
const chapterName = (key: string | null) => {
  const c = chapters.find((x) => x.key === key);
  return c ? (c.names[primary]?.title ?? Object.values(c.names)[0]?.title ?? key!) : (key ?? "");
};

function keepShown() {
  const v: Texts = {};
  for (const f of FORM) {
    // A channel's own text only while its box is ticked.
    if ((f === "instagram" || f === "mastodon") && !input(`post-own-${f}`).checked) continue;
    const x = area(`post-${f}`).value.trim();
    if (x) v[f] = x;
  }
  texts[shown] = v;
}

function showVariant(v: Variant) {
  keepShown();
  shown = v;
  for (const f of FORM) area(`post-${f}`).value = texts[v]?.[f] ?? "";
  for (const c of CHANNELS) input(`post-own-${c}`).checked = !!texts[v]?.[c];
  for (const b of document.querySelectorAll<HTMLButtonElement>("#post-variants button")) b.classList.toggle("on", b.dataset.v === v);
  syncOwn();
}

/** Own-text fields shown when ticked; a new one starts from the shared text. */
function syncOwn(from?: (typeof CHANNELS)[number]) {
  for (const c of CHANNELS) {
    const on = input(`post-own-${c}`).checked;
    $(`post-${c}-row`).style.display = on ? "" : "none";
    if (c === from && on && !area(`post-${c}`).value.trim()) area(`post-${c}`).value = area("post-text").value;
  }
  counts();
}

/** The hashtags as the server appends them. */
const tagLine = () => (picked.length ? `\n\n${picked.map((t) => `#${t}`).join(" ")}` : "");

function counts() {
  const extra = tagLine().length;
  const chosen = CHANNELS.filter((c) => input(`post-ch-${c}`).checked);
  const label = (c: string) => t(LABEL[c]);
  // The shared text: what it comes to per channel that uses it.
  const shared = chosen.filter((c) => !input(`post-own-${c}`).checked);
  const n = area("post-text").value.length;
  const parts = shared.map((c) => `${label(c)} ${n + extra} / ${MAX[c]}`);
  const tc = $("post-text-count");
  tc.textContent = parts.length ? `${parts.join(" · ")}${extra ? ` (${t("po.count.with")})` : ""}` : `${n} / ${MAX.text}`;
  tc.classList.toggle("over", shared.some((c) => n + extra > MAX[c]));
  for (const c of CHANNELS) {
    const m = area(`post-${c}`).value.length + extra;
    const el2 = $(`post-${c}-count`);
    el2.textContent = `${m} / ${MAX[c]}${extra ? ` (${t("po.count.with")})` : ""}`;
    el2.classList.toggle("over", m > MAX[c]);
  }
  $("post-alt-count").textContent = `${area("post-alt").value.length} / ${MAX.alt}`;
  for (const b of document.querySelectorAll<HTMLButtonElement>("#post-variants button")) {
    const v = b.dataset.v as Variant;
    const has = v === shown ? ["text", ...CHANNELS].some((f) => area(`post-${f}`).value.trim()) : !!(texts[v]?.text || texts[v]?.instagram || texts[v]?.mastodon);
    b.classList.toggle("has", has);
  }
}

// --- Instagram's place ----------------------------------------------------------

/** The form's chips: the region's places; a click takes one (or drops the one set). */
function drawSaved() {
  const host = $("post-place-saved");
  const cur = input("post-place-id").value;
  host.replaceChildren(
    ...places.map((pl) =>
      el("button", {
        type: "button",
        className: `tag-chip${pl.id === cur ? " on" : ""}`,
        textContent: `📍 ${pl.name || pl.id}`,
        title: pl.id,
        onclick: () => {
          setPlace(pl.id === cur ? null : { id: pl.id, name: pl.name });
          input("post-place-id").dispatchEvent(new Event("input", { bubbles: true }));
        },
      }),
    ),
  );
}

/** The region's list, below the posts: name, default or not, delete. */
function drawPlaces() {
  const host = $("post-place-list");
  if (!places.length) return void host.replaceChildren(el("p", { className: "empty", textContent: t("po.places.none") }));
  host.replaceChildren(
    ...places.map((pl) => {
      const row = el("div", { className: "tag-row" });
      const name = document.createElement("input");
      Object.assign(name, { value: pl.name, maxLength: 80 });
      name.onchange = () => void placeCall("POST", { ...pl, name: name.value.trim() });
      const box = document.createElement("input");
      box.type = "radio";
      box.name = "post-place-preset";
      box.checked = pl.preset;
      box.onclick = () => void placeCall("POST", { ...pl, name: name.value.trim(), preset: !pl.preset });
      const lab = el("label", { className: "check" });
      const preset: string = t("po.places.preset");
      lab.append(box, ` ${preset}`);
      row.append(name, el("small", { textContent: pl.id }), lab, el("button", { type: "button", textContent: t("ui.delete"), onclick: () => void placeCall("DELETE", pl) }));
      return row;
    }),
  );
}

async function placeCall(method: "POST" | "DELETE", pl: Place & { preset?: boolean }): Promise<boolean> {
  const res =
    method === "POST"
      ? await api(`/api/ops/posts/places?${q()}`, { method, body: JSON.stringify({ id: pl.id, name: pl.name, preset: !!pl.preset }) }).catch(() => null)
      : await api(`/api/ops/posts/places?${q()}&id=${encodeURIComponent(pl.id)}`, { method }).catch(() => null);
  if (!res?.ok) toast(await errorOf(res, t("ui.savefail")), "error");
  await loadPosts(region, primary, chapters);
  return !!res?.ok;
}

function setPlace(p: Place | null, note = "") {
  input("post-place-id").value = p?.id ?? "";
  input("post-place-name").value = p?.name ?? "";
  const shown = p ? `📍 ${p.id}` : t("po.f.place.none");
  $("post-place-info").textContent = note ? `${shown} · ${note}` : shown;
  $("post-place-clear").style.display = p ? "" : "none";
  input("post-place-name").style.display = p ? "" : "none";
  drawSaved();
}

type Found = { id: string; name: string; city: string; street: string; km: number | null };

/** Search results: one button per page with a place; a click takes it. */
function drawFound(list: Found[]) {
  const host = $("post-place-results");
  host.hidden = false;
  if (!list.length) return void host.replaceChildren(el("small", { textContent: t("po.f.place.found0") }));
  host.replaceChildren(el("small", { textContent: t("po.f.place.pick") }));
  for (const f of list) {
    const where = [f.street, f.city].filter(Boolean).join(", ");
    const b = el("button", { type: "button", textContent: `📍 ${f.name}${where ? ` · ${where}` : ""}${f.km != null ? ` · ${f.km} km` : ""}` });
    b.onclick = () => {
      const before = input("post-place-id").value;
      setPlace({ id: f.id, name: f.city && !f.name.includes(f.city) ? `${f.name}, ${f.city}` : f.name });
      host.hidden = true;
      input("post-place-q").value = "";
      if (input("post-place-id").value !== before) input("post-place-id").dispatchEvent(new Event("input", { bubbles: true }));
    };
    host.append(b);
  }
}

async function checkPlace() {
  const raw = input("post-place-q").value.trim();
  if (!raw) return;
  const btn = $<HTMLButtonElement>("post-place-check");
  btn.disabled = true;
  $("post-place-results").hidden = true;
  const res = await api(`/api/ops/posts/place?${q()}&q=${encodeURIComponent(raw)}`).catch(() => null);
  btn.disabled = false;
  const before = input("post-place-id").value;
  if (res?.ok) {
    const v = (await res.json()) as { results?: Found[]; id: string; name: string; city: string; has_place: boolean; checked: "page" | "container" };
    if (v.results) return drawFound(v.results);
    const name = v.name ? (v.city && !v.name.includes(v.city) ? `${v.name}, ${v.city}` : v.name) : input("post-place-name").value;
    setPlace({ id: v.id, name }, v.checked === "container" ? t("po.f.place.tried") : v.has_place ? "" : t("po.f.place.noloc"));
    if (!name) input("post-place-name").focus();
  } else {
    // The server's words as they are (Meta's message), else the usual text.
    const msg: string | undefined = res && res.status !== 401 && res.status !== 403 ? (await res.clone().json().catch(() => null))?.error : undefined;
    const err = msg ?? (await errorOf(res, t("ui.savefail")));
    // A bare id stays usable when Meta won't say (that is what sending shows).
    // 422: Instagram refused the id itself; anything else may be passing.
    if (res?.status !== 422 && /^\d+$/.test(raw)) setPlace({ id: raw, name: "" }, t("po.f.place.unchecked", { err }));
    else return void toast(err, "error");
  }
  input("post-place-q").value = "";
  if (input("post-place-id").value !== before) input("post-place-id").dispatchEvent(new Event("input", { bubbles: true }));
}

const placeOf = (): Place | null => (input("post-place-id").value ? { id: input("post-place-id").value, name: input("post-place-name").value } : null);

// --- Hashtags --------------------------------------------------------------------

const cleanTag = (s: string) => {
  const x = s.trim().replace(/^#+/, "");
  return /^[\p{L}\p{N}_]{1,60}$/u.test(x) && !/^\d+$/.test(x) ? x : null;
};

/** The form's chips: the region's hashtags and the post's own, ticked when used. */
function drawPicked() {
  const host = $("post-tags");
  const all = [...hashtags.map((h) => h.tag)];
  for (const p of picked) if (!all.some((x) => x.toLowerCase() === p.toLowerCase())) all.push(p);
  host.replaceChildren(
    ...all.map((tag) => {
      const on = picked.some((p) => p.toLowerCase() === tag.toLowerCase());
      return el("button", {
        type: "button",
        className: `tag-chip${on ? " on" : ""}`,
        textContent: `#${tag}`,
        onclick: () => {
          picked = on ? picked.filter((p) => p.toLowerCase() !== tag.toLowerCase()) : [...picked, tag];
          input("post-tag-new").dispatchEvent(new Event("input", { bubbles: true }));
          drawPicked();
        },
      });
    }),
  );
  counts();
}

/** The region's list, below the posts: preset or not, delete. */
function drawHashtags() {
  const host = $("post-hashtag-list");
  if (!hashtags.length) {
    host.replaceChildren(el("p", { className: "empty", textContent: t("po.tags.none") }));
    return;
  }
  host.replaceChildren(
    ...hashtags.map((h) => {
      const row = document.createElement("div");
      row.className = "tag-row";
      const box = document.createElement("input");
      box.type = "checkbox";
      box.checked = h.preset;
      box.onchange = () => void tagCall("POST", h.tag, box.checked);
      const lab = document.createElement("label");
      lab.className = "check";
      const preset: string = t("po.tags.preset");
      lab.append(box, ` ${preset}`);
      row.append(el("b", { textContent: `#${h.tag}` }), lab, el("button", { type: "button", textContent: t("ui.delete"), onclick: () => void tagCall("DELETE", h.tag) }));
      return row;
    }),
  );
}

async function tagCall(method: "POST" | "DELETE", tag: string, preset = false) {
  const res =
    method === "POST"
      ? await api(`/api/ops/posts/tags?${q()}`, { method, body: JSON.stringify({ tag, preset }) }).catch(() => null)
      : await api(`/api/ops/posts/tags?${q()}&tag=${encodeURIComponent(tag)}`, { method }).catch(() => null);
  if (!res?.ok) toast(await errorOf(res, t("ui.savefail")), "error");
  await loadPosts(region, primary, chapters);
}

function syncTrigger() {
  const tr = $<HTMLSelectElement>("post-trigger").value;
  // style, not `hidden`: the form's label and row rules would win.
  $("post-at-row").style.display = tr === "at" ? "" : "none";
  $("post-chapter-row").style.display = tr.startsWith("chapter_") ? "" : "none";
  $("post-card-fields").style.display = input("post-card-on").checked ? "" : "none";
  void showMap();
}

// --- The picture's area on a map ----------------------------------------------

const EARTH = 40075016.686;
const W_MIN = 200;
const W_MAX = 5000;
/** Metres across the frame at zoom `z` (MapLibre: 512 px tiles). */
const widthAt = (z: number, lat: number, px: number) => (EARTH * Math.cos((lat * Math.PI) / 180) * px) / (512 * 2 ** z);
const zoomFor = (w: number, lat: number, px: number) => Math.log2((EARTH * Math.cos((lat * Math.PI) / 180) * px) / (512 * w));
const framePx = () => (document.querySelector(".post-frame") as HTMLElement).clientWidth;

function frameInfo() {
  const w = Math.round(Number(input("post-card-width").value) || 900);
  const auto = !input("post-card-lat").value;
  $("post-frame-info").textContent = t(auto ? "po.f.frame.auto" : "po.f.frame.set", { w });
  $("post-frame-auto").style.display = auto ? "none" : "";
}

/** A field set by the map, marked as changed for the form. */
function setField(id: string, v: string) {
  const i = input(id);
  if (i.value === v) return;
  i.value = v;
  i.dispatchEvent(new Event("input", { bubbles: true }));
}

/** Moves the map to the fields (or the region when automatic). */
async function place() {
  if (!map) return;
  const px = framePx();
  if (!px) return;
  placing = true;
  const w = Number(input("post-card-width").value) || 900;
  let lat = Number(input("post-card-lat").value);
  let lon = Number(input("post-card-lon").value);
  if (!input("post-card-lat").value) {
    const r = (await listRegions().catch(() => [])).find((x) => x.slug === region);
    if (!r || !map) return void (placing = false);
    lon = (r.bounds[0] + r.bounds[2]) / 2;
    lat = (r.bounds[1] + r.bounds[3]) / 2;
  }
  placing = true;
  map.jumpTo({ center: [lon, lat], zoom: zoomFor(w, lat, px) });
  placing = false;
  frameInfo();
}

/** The user moved the map: the frame's centre and width go to the fields. */
function moved() {
  if (placing || !map) return;
  const px = framePx();
  const c = map.getCenter();
  const raw = widthAt(map.getZoom(), c.lat, px);
  const w = Math.min(W_MAX, Math.max(W_MIN, Math.round(raw / 10) * 10));
  if (Math.abs(raw - w) / w > 0.05) {
    placing = true;
    map.jumpTo({ zoom: zoomFor(w, c.lat, px) });
    placing = false;
  }
  setField("post-card-lat", c.lat.toFixed(5));
  setField("post-card-lon", c.lng.toFixed(5));
  setField("post-card-width", String(w));
  frameInfo();
}

/** The map, once the form shows the picture's fields (it needs its size). */
async function showMap() {
  if ($("post-form").hidden || !input("post-card-on").checked) return;
  if (!map || mapRegion !== region) {
    map?.remove();
    mapRegion = region;
    map = new maplibregl.Map({
      container: "post-map",
      style: await basemapStyle("grayscale", false, region),
      center: [0, 0],
      zoom: 1,
      dragRotate: false,
      pitchWithRotate: false,
      attributionControl: { compact: true },
    });
    map.touchZoomRotate.disableRotation();
    map.keyboard.disableRotation();
    map.on("moveend", moved);
  }
  placing = true;
  map.resize();
  await place();
}

/** The pictures were changed: the form counts as edited. */
const picsChanged = () => $("post-pics").dispatchEvent(new Event("input", { bubbles: true }));

function drawPics() {
  const host = $("post-pics");
  const masto = input("post-ch-mastodon").checked && pics.length > MASTODON_PICS;
  const cnt = $("post-pics-count");
  cnt.textContent = t("po.pics.count", { n: pics.length }) + (masto ? ` · ${t("po.pics.masto")}` : "");
  cnt.classList.toggle("over", masto);
  $<HTMLButtonElement>("post-pic-add").disabled = pics.length >= PICS_MAX;
  host.replaceChildren(
    ...pics.map((p, i) => {
      const tile = el("div", { className: "post-pic-tile" });
      const fig = el("div", { className: `post-pic-thumb${p !== "card" && p.fit === "frame" ? " frame" : ""}` });
      if (p === "card") fig.append(el("span", { textContent: `🗺 ${t("po.pic.card")}` }));
      else {
        const img = el("img", { alt: "" });
        const key = `u${p.id}`;
        const known = pictures.get(key);
        if (known) img.src = known;
        else
          void picture(`/api/ops/posts/image?${q()}&id=${p.id}`).then((u) => {
            if (u) pictures.set(key, u), (img.src = u);
          });
        fig.append(img);
      }
      const move = (d: number) => {
        const j = i + d;
        if (j < 0 || j >= pics.length) return;
        [pics[i], pics[j]] = [pics[j], pics[i]];
        drawPics();
        picsChanged();
      };
      const tool = (text: string, title: string, off: boolean, fn: () => void) => {
        const b = document.createElement("button");
        Object.assign(b, { type: "button", textContent: text, title, disabled: off });
        b.onclick = fn;
        return b;
      };
      const tools = el("div", { className: "post-pic-tools" });
      tools.append(
        tool("◀", t("po.pic.left"), i === 0, () => move(-1)),
        tool("▶", t("po.pic.right"), i === pics.length - 1, () => move(1)),
        tool("✕", t("po.pic.remove"), false, () => {
          pics.splice(i, 1);
          if (p === "card") (input("post-card-on").checked = false), syncTrigger();
          drawPics();
          picsChanged();
        }),
      );
      tile.append(fig, tools);
      if (p !== "card") {
        const fit = document.createElement("select");
        const crop: string = t("po.pic.crop");
        const frame: string = t("po.pic.frame");
        fit.append(new Option(crop, "crop"), new Option(frame, "frame"));
        fit.value = p.fit;
        fit.onchange = () => {
          p.fit = fit.value as "crop" | "frame";
          fig.classList.toggle("frame", p.fit === "frame");
        };
        const ph: string = t("po.pic.alt");
        const alt = document.createElement("textarea");
        Object.assign(alt, { value: p.alt, maxLength: 1000, rows: 2, placeholder: ph });
        alt.oninput = () => (p.alt = alt.value);
        tile.append(fit, alt);
      }
      return tile;
    }),
  );
}

async function upload(files: FileList) {
  const btn = $<HTMLButtonElement>("post-pic-add");
  const label = btn.textContent;
  btn.disabled = true;
  btn.textContent = t("po.pic.wait");
  try {
    for (const f of files) {
      if (pics.length >= PICS_MAX) break;
      const res = await api(`/api/ops/posts/image?${q()}`, { method: "POST", body: f }).catch(() => null);
      if (!res?.ok) {
        toast(t("po.pic.fail", { err: `${f.name}: ${await errorOf(res, t("ui.savefail"))}` }), "error");
        continue;
      }
      const v = (await res.json()) as { id: number };
      pictures.set(`u${v.id}`, URL.createObjectURL(f));
      pics.push({ id: v.id, alt: "", fit: "crop" });
      drawPics();
      picsChanged();
    }
  } finally {
    btn.textContent = label;
    drawPics();
  }
}

function open(p: Post | null) {
  clean("post-form");
  editing = p;
  $("post-form-title").textContent = t(p ? "po.edit" : "po.new");
  input("post-name").value = p?.title ?? "";
  $<HTMLSelectElement>("post-trigger").value = p?.trigger ?? "chapter_end";
  const sel = $<HTMLSelectElement>("post-chapter");
  sel.replaceChildren(...chapters.map((c) => new Option(`${chapterName(c.key)} (${fmtDate(c.starts, false)})`, c.key)));
  if (p?.chapter) sel.value = p.chapter;
  input("post-offset").value = String(p?.offset_min ?? 0);
  input("post-at").value = toLocal(p?.at ?? Math.floor(Date.now() / 3_600_000 + 1) * 3600);
  for (const c of ["instagram", "mastodon"]) input(`post-ch-${c}`).checked = p ? p.channels.includes(c) : true;
  texts = structuredClone(p?.texts ?? {});
  shown = "any";
  for (const f of FORM) area(`post-${f}`).value = texts.any?.[f] ?? "";
  for (const c of CHANNELS) input(`post-own-${c}`).checked = !!texts.any?.[c];
  picked = p ? [...p.tags] : hashtags.filter((h) => h.preset).map((h) => h.tag);
  input("post-tag-new").value = "";
  drawPicked();
  // A new post: the place of the latest post that has one.
  input("post-place-q").value = "";
  const preset = places.find((x) => x.preset);
  setPlace(p ? p.location : preset ? { id: preset.id, name: preset.name } : (posts.find((x) => x.location)?.location ?? null));
  showVariant(p && !p.texts.any ? (VARIANTS.find((v) => p.texts[v]) ?? "any") : "any");
  input("post-card-on").checked = !!p?.card;
  pics = p?.pictures ? structuredClone(p.pictures) : p?.card ? ["card"] : [];
  drawPics();
  input("post-card-lat").value = p?.card?.lat != null ? String(p.card.lat) : "";
  input("post-card-lon").value = p?.card?.lon != null ? String(p.card.lon) : "";
  input("post-card-width").value = String(p?.card?.width_m ?? 900);
  // Stored cards without a look are the old map; new ones look like the game.
  $<HTMLSelectElement>("post-card-look").value = p?.card ? (p.card.look ?? "plan") : "game";
  for (const l of LAYERS) input(`post-layer-${l}`).checked = (p?.card?.layers ?? LAYERS_ON).includes(l);
  syncLook();
  input("post-card-title").value = p?.card?.title ?? "";
  input("post-card-note").value = p?.card?.note ?? "";
  $("post-preview").replaceChildren();
  $("post-error").textContent = "";
  $("post-form").hidden = false;
  syncTrigger();
  input("post-name").focus();
}

function cardOf(): Card | null {
  if (!input("post-card-on").checked) return null;
  const c: Card = {
    width_m: Number(input("post-card-width").value) || 900,
    title: input("post-card-title").value.trim(),
    note: input("post-card-note").value.trim(),
    look: $<HTMLSelectElement>("post-card-look").value as "plan" | "game",
    layers: LAYERS.filter((l) => input(`post-layer-${l}`).checked),
  };
  const lat = input("post-card-lat").value.trim();
  const lon = input("post-card-lon").value.trim();
  if (lat || lon) Object.assign(c, { lat: Number(lat.replace(",", ".")), lon: Number(lon.replace(",", ".")) });
  return c;
}

async function save() {
  keepShown();
  const tr = $<HTMLSelectElement>("post-trigger").value;
  const body = {
    id: editing?.id,
    title: input("post-name").value.trim(),
    trigger: tr,
    at: tr === "at" ? fromLocal(input("post-at").value) : null,
    chapter: tr.startsWith("chapter_") ? $<HTMLSelectElement>("post-chapter").value : null,
    offset_min: Math.round(Number(input("post-offset").value) || 0),
    texts: Object.fromEntries(Object.entries(texts).filter(([, v]) => v && (v.text || v.instagram || v.mastodon))),
    card: cardOf(),
    channels: ["instagram", "mastodon"].filter((c) => input(`post-ch-${c}`).checked),
    tags: picked,
    location: placeOf(),
    pictures: pics,
  };
  const res = await api(`/api/ops/posts?${q()}`, { method: "POST", body: JSON.stringify(body) }).catch(() => null);
  if (!res?.ok) {
    $("post-error").textContent = await errorOf(res, t("ui.savefail"));
    return;
  }
  clean("post-form");
  toast(t("po.saved"));
  $("post-form").hidden = true;
  await loadPosts(region, primary, chapters);
}

/** A picture through the token: fetched as a blob. */
async function picture(path: string): Promise<string | null> {
  const res = await api(path).catch(() => null);
  if (!res?.ok) return null;
  return URL.createObjectURL(await res.blob());
}

/** The app's picture (photo.html) in a hidden frame: the game as players see
 *  it, without its buttons. Live, or a chapter's end. */
function photo(c: { lat: number | null; lon: number | null; width_m: number; title: string; note: string; layers?: string[] | null; chapter?: string | null }): Promise<Blob> {
  const p = new URLSearchParams({ region, width_m: String(c.width_m), title: c.title, note: c.note, layers: (c.layers ?? LAYERS_ON).join(",") });
  p.set("watch", "");
  if (c.lat != null && c.lon != null) p.set("lat", String(c.lat)), p.set("lon", String(c.lon));
  if (c.chapter) p.set("chapter", c.chapter);
  const frame = el("iframe", { src: `/photo.html?${p}`, title: "photo" });
  // Off screen but laid out and drawn (a hidden frame would not render WebGL).
  Object.assign(frame.style, { position: "fixed", left: "-10000px", top: "0", width: "540px", height: "675px", border: "0" });
  return new Promise<Blob>((resolve, reject) => {
    const stop = (why?: string, blob?: Blob) => {
      clearTimeout(timer);
      window.removeEventListener("message", on);
      frame.remove();
      if (blob) resolve(blob);
      else reject(new Error(why ?? "?"));
    };
    const on = (e: MessageEvent) => {
      if (e.source !== frame.contentWindow || e.origin !== location.origin || e.data?.t !== "photo") return;
      stop(e.data.error, e.data.blob);
    };
    const timer = setTimeout(() => stop("timeout"), 120_000);
    window.addEventListener("message", on);
    document.body.append(frame);
  });
}

/** Layers only for the app's look. */
function syncLook() {
  $("post-layers").hidden = $<HTMLSelectElement>("post-card-look").value !== "game";
}

/** Beside the channels' results: the filled-in frame and whether the app took the picture. */
const extra = (r: Run) => (r.results ?? {}) as unknown as { card?: Shot; photo?: boolean };

/** A draft's picture from the app (photo.html) to the server, then shown. */
const shooting = new Set<number>();
async function shootRun(r: Run, img: HTMLImageElement, dl: HTMLAnchorElement, btn: HTMLButtonElement, layers?: string[]) {
  const base = extra(r).card;
  if (!base || shooting.has(r.id)) return;
  const c = layers ? { ...base, layers } : base;
  shooting.add(r.id);
  btn.disabled = true;
  btn.textContent = t("po.photo.wait");
  try {
    const blob = await photo(c);
    const lay = layers ? `&layers=${encodeURIComponent(layers.join(","))}` : "";
    const res = await api(`/api/ops/posts/run.jpg?${q()}&id=${r.id}${lay}`, { method: "POST", headers: { "Content-Type": "image/jpeg" }, body: blob });
    if (!res.ok) throw new Error(await errorOf(res, t("ui.savefail")));
    const u = URL.createObjectURL(blob);
    pictures.set(String(r.id), u);
    img.src = u;
    dl.href = u;
    r.results = { ...(r.results ?? {}), photo: true, card: c } as unknown as Run["results"];
  } catch (e) {
    toast(t("po.photo.fail", { err: (e as Error).message }), "error");
  } finally {
    shooting.delete(r.id);
    btn.disabled = false;
    btn.textContent = t("po.photo.take");
  }
}

async function preview() {
  const c = cardOf() ?? { width_m: 900, title: "", note: "" };
  const p = new URLSearchParams({ region, width_m: String(c.width_m), title: c.title, note: c.note, look: c.look ?? "game" });
  if (c.lat != null && c.lon != null) p.set("lat", String(c.lat)), p.set("lon", String(c.lon));
  const tr = $<HTMLSelectElement>("post-trigger").value;
  if (tr === "chapter_end") {
    const k = $<HTMLSelectElement>("post-chapter").value;
    const ch = chapters.find((x) => x.key === k);
    if (ch && ch.ends < Date.now() / 1000) p.set("chapter", k);
  }
  const host = $("post-preview");
  host.replaceChildren(el("small", { textContent: t("po.preview.wait") }));
  // As in the game: the app draws it here (photo.ts); the old map comes from the server.
  const url =
    c.look === "game"
      ? await photo({ ...c, lat: c.lat ?? null, lon: c.lon ?? null, chapter: p.get("chapter") })
          .then((b) => URL.createObjectURL(b))
          .catch((e: Error) => (toast(t("po.photo.fail", { err: e.message }), "error"), null))
      : await picture(`/api/ops/card.jpg?${p}`);
  host.replaceChildren(url ? el("img", { src: url, className: "post-pic", alt: "" }) : el("small", { textContent: t("po.preview.fail") }));
}

const OUTCOME: Record<string, EKey> = { fire: "po.v.fire", water: "po.v.water", tie: "po.v.tie" };

function whenOf(p: Post): string {
  const off = p.offset_min ? ` ${p.offset_min > 0 ? "+" : "−"}${Math.abs(p.offset_min)} min` : "";
  switch (p.trigger) {
    case "at":
      return fmtDate(p.at!);
    case "manual":
      return t("po.tr.manual");
    default:
      return `${t(p.trigger === "chapter_start" ? "po.tr.start" : "po.tr.end")}: ${chapterName(p.chapter)}${off}`;
  }
}

function copyButton(text: () => string) {
  return el("button", {
    type: "button",
    textContent: t("po.copy"),
    onclick: async () => {
      await navigator.clipboard.writeText(text()).then(
        () => toast(t("po.copied")),
        () => toast(t("po.copyfail"), "error"),
      );
    },
  });
}

async function decide(r: Run, state: Run["state"], edited?: Texts) {
  const res = await api(`/api/ops/posts/run?${q()}&id=${r.id}`, { method: "POST", body: JSON.stringify({ state, texts: edited }) }).catch(() => null);
  if (!res?.ok) toast(await errorOf(res, t("ui.savefail")), "error");
  else toast(t(state === "sent" ? "po.marked" : state === "discarded" ? "po.discarded" : "po.reopened"));
  await loadPosts(region, primary, chapters);
}

async function send(r: Run, ch: string, edited: Texts | undefined, btn: HTMLButtonElement) {
  if (!(await ask(t("po.send.ask", { ch: t(LABEL[ch]) }), t("po.send")))) return;
  btn.disabled = true;
  const res = await api(`/api/ops/posts/send?${q()}&id=${r.id}&channel=${ch}`, { method: "POST", body: JSON.stringify({ texts: edited }) }).catch(() => null);
  if (res?.ok) toast(t("po.sent"));
  else toast(t("po.send.err", { err: await errorOf(res, t("ui.savefail")) }), "error");
  await loadPosts(region, primary, chapters);
}

/** A draft's map picture: download, and the app's picture with its layers. */
function cardFig(r: Run): HTMLElement {
  const fig = el("div", { className: "post-run-pic" });
  const known = pictures.get(String(r.id));
  const img = el("img", { className: "post-pic", alt: "" });
  const dl = el("a", { textContent: t("po.download"), className: "ed-link", download: `post-${r.id}.jpg` });
  const set = (u: string) => ((img.src = u), (dl.href = u));
  if (known) set(known);
  else
    void picture(`/api/ops/posts/run.jpg?${q()}&id=${r.id}`).then((u) => {
      if (u) pictures.set(String(r.id), u), set(u);
    });
  fig.append(img, dl);
  // As in the game: the app takes the picture (the server's is a stand-in), once by itself.
  if (r.state === "draft" && extra(r).card?.look === "game") {
    // This draft's layers (from its post; e.g. rain only today), then take it again.
    const have = extra(r).card?.layers ?? LAYERS_ON;
    const pick = el("div", { className: "post-layers" });
    const boxes = LAYERS.map((l) => {
      const box = document.createElement("input");
      box.type = "checkbox";
      box.checked = have.includes(l);
      const lab = el("label", { className: "check" });
      lab.append(box, ` ${t(`po.layer.${l}` as EKey)}`);
      pick.append(lab);
      return [l, box] as const;
    });
    const again = el("button", { type: "button", textContent: t("po.photo.take") });
    again.onclick = () => void shootRun(r, img, dl, again, boxes.filter(([, b]) => b.checked).map(([l]) => l));
    fig.append(pick, again);
    if (!extra(r).photo) void shootRun(r, img, dl, again);
  }
  return fig;
}

function drawRun(r: Run): HTMLElement {
  const p = posts.find((x) => x.id === r.post);
  const box = el("div", { className: `post-run ${r.state}` });
  const head = el("div", { className: "map-head" });
  const h = el("h3", { textContent: p?.title ?? `#${r.post}` });
  const meta = el("small", { textContent: fmtDate(r.created) });
  if (r.outcome) meta.textContent += ` · ${t(OUTCOME[r.outcome])}`;
  if (r.numbers) meta.textContent += ` · 🔥 ${r.numbers.fire} · 💧 ${r.numbers.water}`;
  if (p?.location && p.channels.includes("instagram")) meta.textContent += ` · 📍 ${p.location.name || p.location.id}`;
  if (r.state !== "draft") meta.textContent += ` · ${t(r.state === "sent" ? "po.st.sent" : "po.st.discarded")}${r.decided_by ? ` (${r.decided_by})` : ""}`;
  head.append(h, meta);
  box.append(head);
  if (r.note) box.append(el("p", { className: "notice", textContent: r.note }));
  const body = el("div", { className: "post-run-body" });
  const pics = el("div", { className: "post-run-pics" });
  const total = (r.image ? 1 : 0) + (r.extra ?? 0);
  const cardAt = r.image ? Math.min(r.card_at ?? 0, total - 1) : -1;
  for (let i = 0; i < total; i++) {
    if (i === cardAt) {
      pics.append(cardFig(r));
      continue;
    }
    const fig = el("div", { className: "post-run-pic" });
    const img = el("img", { className: "post-pic", alt: "" });
    const dl = el("a", { textContent: t("po.download"), className: "ed-link", download: `post-${r.id}-${i + 1}.jpg` });
    const key = `${r.id}:${i}`;
    const set = (u: string) => ((img.src = u), (dl.href = u));
    const known = pictures.get(key);
    if (known) set(known);
    else
      void picture(`/api/ops/posts/run.jpg?${q()}&id=${r.id}&n=${i}`).then((u) => {
        if (u) pictures.set(key, u), set(u);
      });
    fig.append(img, dl);
    pics.append(fig);
  }
  if (total) body.append(pics);
  const fields = el("div", { className: "post-run-texts" });
  const areas = new Map<string, HTMLTextAreaElement>();
  const edited = () => Object.fromEntries([...areas].map(([k, a]) => [k, a.value.trim()])) as Texts;
  const changed = () => [...areas].some(([k, a]) => a.value.trim() !== (r.texts[k as keyof Texts] ?? "").trim());
  for (const f of FIELDS) {
    if (r.texts[f] == null) continue;
    const a = document.createElement("textarea");
    Object.assign(a, { value: r.texts[f]!, maxLength: MAX[f], readOnly: r.state !== "draft" });
    areas.set(f, a);
    const lab = el("label");
    const row = el("span", { className: "post-run-label" });
    const tools = el("span");
    tools.append(copyButton(() => a.value));
    const done = r.results?.[f];
    if (done?.sending && !done.url) tools.append(el("small", { textContent: t("po.sending") }));
    else if (done?.url) tools.append(el("a", { className: "ed-link", href: done.url, target: "_blank", rel: "noopener", textContent: t("po.sent.view") }));
    else if (r.state === "draft" && ready.includes(f)) {
      const b = el("button", { type: "button", textContent: t("po.send") });
      b.onclick = () => void send(r, f, changed() ? edited() : undefined, b);
      tools.append(b);
    }
    row.append(el("b", { textContent: t(LABEL[f]) }), tools);
    lab.append(row, a);
    if (done?.error && !done.url) lab.append(el("small", { className: "error", textContent: t("po.send.err", { err: done.error }) }));
    if (done?.note && done.url) lab.append(el("small", { textContent: done.note }));
    fields.append(lab);
  }
  body.append(fields);
  box.append(body);
  const acts = el("div", { className: "ed-row" });
  if (r.state === "draft") {
    acts.append(
      el("button", { type: "button", className: "primary", textContent: t("po.mark"), onclick: () => void decide(r, "sent", changed() ? edited() : undefined) }),
      el("button", {
        type: "button",
        textContent: t("po.discard"),
        onclick: async () => {
          if (await ask(t("po.discard.ask"), t("po.discard"), true)) await decide(r, "discarded");
        },
      }),
    );
  } else {
    acts.append(el("button", { type: "button", textContent: t("po.reopen"), onclick: () => void decide(r, "draft") }));
  }
  box.append(acts);
  return box;
}

function drawRuns() {
  const host = $("post-runs");
  const open = runs.filter((r) => r.state === "draft");
  const done = runs.filter((r) => r.state !== "draft").slice(0, 10);
  host.replaceChildren();
  // The server checks the token daily; warn when it fails or ends soon.
  if (insta) {
    const user = insta.username ?? "?";
    const soon = insta.expires != null && insta.expires * 1000 - Date.now() < 14 * 86400e3;
    const msg = insta.error ? t("po.ig.error", { user, err: insta.error }) : soon ? t("po.ig.expires", { user, date: fmtDate(insta.expires!) }) : "";
    if (msg) host.append(el("p", { className: "notice", textContent: msg }));
  }
  if (!open.length) host.append(el("p", { className: "empty", textContent: t("po.runs.none") }));
  for (const r of open) host.append(drawRun(r));
  if (done.length) {
    const d = el("details", { className: "post-done" });
    d.append(el("summary", { textContent: t("po.runs.done", { n: done.length }) }), ...done.map(drawRun));
    host.append(d);
  }
}

/** The list's word for a post's pictures. */
function picsLabel(p: Post): string[] {
  const list = p.pictures ?? (p.card ? ["card"] : []);
  if (list.length > 1) return [t("po.pics.n", { n: list.length })];
  if (list.length === 1) return [list[0] === "card" ? t("po.f.card") : t("po.f.pics")];
  return [];
}

function drawList() {
  const host = $("posts-list");
  if (!posts.length) {
    host.className = "empty";
    host.textContent = t("po.none");
    return;
  }
  host.className = "";
  const tbl = el("table", { className: "ed-table" });
  const hr = tbl.createTHead().insertRow();
  for (const h of [t("po.th.post"), t("po.th.when"), t("ch.th.status"), ""]) hr.append(el("th", { textContent: h }));
  for (const p of posts) {
    const tr = tbl.insertRow();
    const name = tr.insertCell();
    name.append(el("b", { textContent: p.title }), el("br"), el("small", { textContent: [...p.channels.map((c) => t(LABEL[c] ?? "po.f.mastodon")), ...picsLabel(p)].join(" · ") }));
    tr.insertCell().textContent = whenOf(p);
    const mine = runs.filter((r) => r.post === p.id);
    const st = tr.insertCell();
    const last = mine[0];
    st.append(el("span", { className: `st ${last?.state === "draft" ? "live" : ""}`, textContent: last ? t(last.state === "draft" ? "po.st.draft" : last.state === "sent" ? "po.st.sent" : "po.st.discarded") : t("st.planned") }));
    const td = tr.insertCell().appendChild(el("div", { className: "post-acts" }));
    const b = (label: string, fn: () => void, title = "") => td.append(el("button", { type: "button", textContent: label, title, onclick: fn }));
    b(t("ui.edit"), async () => {
      if (await mayLeave("post-form")) open(p);
    });
    b(t("po.draftnow"), async () => {
      const res = await api(`/api/ops/posts/draft?${q()}&id=${p.id}`, { method: "POST" }).catch(() => null);
      if (!res?.ok) toast(await errorOf(res, t("ui.savefail")), "error");
      else toast(t("po.drafted"));
      await loadPosts(region, primary, chapters);
    }, t("po.draftnow.hint"));
    b(t("ui.delete"), async () => {
      if (!(await ask(t("po.delete.ask", { name: p.title }), t("ui.delete"), true))) return;
      const res = await api(`/api/ops/posts?${q()}&id=${p.id}`, { method: "DELETE" });
      if (!res.ok) toast(await errorOf(res, t("ui.deletefail")), "error");
      else toast(t("po.deleted"));
      await loadPosts(region, primary, chapters);
    });
  }
  host.replaceChildren(tbl);
}

export function initPosts(apiFn: Api) {
  api = apiFn;
  // The placeholders shown as such (t() would fill them).
  $("post-variants-hint").innerHTML = t("po.f.variants.hint", { fire: "{fire}", water: "{water}", chapter: "{chapter}" });
  $("post-new").onclick = async () => {
    if (await mayLeave("post-form")) open(null);
  };
  watchForm("post-form");
  $("post-cancel").onclick = async () => {
    if (!(await mayLeave("post-form"))) return;
    $("post-form").hidden = true;
  };
  $("post-save").onclick = () => void save();
  $("post-preview-btn").onclick = () => void preview();
  $("post-trigger").onchange = syncTrigger;
  input("post-card-on").onchange = () => {
    const on = input("post-card-on").checked;
    pics = pics.filter((x) => x !== "card");
    if (on) pics.unshift("card");
    drawPics();
    syncTrigger();
  };
  $("post-pic-add").onclick = () => input("post-pic-file").click();
  input("post-pic-file").onchange = () => {
    const f = input("post-pic-file");
    if (f.files?.length) void upload(f.files).finally(() => (f.value = ""));
  };
  input("post-ch-mastodon").addEventListener("change", drawPics);
  $("post-frame-auto").onclick = () => {
    setField("post-card-lat", "");
    setField("post-card-lon", "");
    void place();
  };
  for (const f of FORM) area(`post-${f}`).oninput = counts;
  for (const c of CHANNELS) {
    input(`post-own-${c}`).onchange = () => syncOwn(c);
    input(`post-ch-${c}`).addEventListener("change", counts);
  }
  input("post-tag-new").onkeydown = (e) => {
    if (e.key !== "Enter" && e.key !== "," && e.key !== " ") return;
    e.preventDefault();
    const tag = cleanTag(input("post-tag-new").value);
    if (!input("post-tag-new").value.trim()) return;
    if (!tag) return toast(t("po.tags.bad"), "error");
    if (!picked.some((p) => p.toLowerCase() === tag.toLowerCase())) picked.push(tag);
    input("post-tag-new").value = "";
    drawPicked();
  };
  const addTag = () => {
    const raw = input("post-hashtag-new").value;
    const tag = cleanTag(raw);
    if (!raw.trim()) return;
    if (!tag) return toast(t("po.tags.bad"), "error");
    input("post-hashtag-new").value = "";
    void tagCall("POST", tag, true);
  };
  $("post-hashtag-add").onclick = addTag;
  $("post-place-add").onclick = () => {
    const id = input("post-place-new-id").value.trim();
    if (!id) return;
    void placeCall("POST", { id, name: input("post-place-new-name").value.trim() }).then((ok) => {
      if (!ok) return;
      input("post-place-new-id").value = "";
      input("post-place-new-name").value = "";
    });
  };
  $("post-place-check").onclick = () => void checkPlace();
  $("post-card-look").onchange = syncLook;
  input("post-place-q").onkeydown = (e) => {
    if (e.key === "Enter") (e.preventDefault(), void checkPlace());
  };
  $("post-place-clear").onclick = () => {
    setPlace(null);
    input("post-place-id").dispatchEvent(new Event("input", { bubbles: true }));
  };
  input("post-hashtag-new").onkeydown = (e) => {
    if (e.key === "Enter") (e.preventDefault(), addTag());
  };
  const vs = $("post-variants");
  vs.replaceChildren(
    ...VARIANTS.map((v) => el("button", { type: "button", textContent: t(`po.v.${v}` as EKey), onclick: () => showVariant(v) })),
  );
  [...vs.children].forEach((b, i) => ((b as HTMLElement).dataset.v = VARIANTS[i]));
}

export async function loadPosts(slug: string, lang: string, chs: ChapterRef[]) {
  if (slug !== region) {
    for (const u of pictures.values()) URL.revokeObjectURL(u);
    pictures.clear();
  }
  region = slug;
  primary = lang;
  chapters = [...chs].sort((a, b) => b.starts - a.starts);
  const res = await api(`/api/ops/posts?${q()}`).catch(() => null);
  $("posts-card").hidden = !res?.ok;
  if (!res?.ok) return;
  ({ posts, runs, ready, instagram: insta, hashtags, places } = (await res.json()) as {
    posts: Post[];
    runs: Run[];
    ready: string[];
    instagram: typeof insta;
    hashtags: typeof hashtags;
    places: typeof places;
  });
  places ??= [];
  drawHashtags();
  drawPlaces();
  if (!$("post-form").hidden) drawSaved();
  drawRuns();
  drawList();
  const n = runs.filter((r) => r.state === "draft").length;
  const badge = document.querySelector('#tabs button[data-tab="beitraege"] .badge');
  if (badge) badge.textContent = n ? String(n) : "";
}
