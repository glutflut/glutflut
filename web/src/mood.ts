// The mood map (`/api/mood`): how places felt, from "Wie ist es hier?", per
// H3 cell for one past day; only cells where at least k different people
// answered. A button in the drawer shows it, a tap on a cell says the shares.
import * as maplibregl from "maplibre-gl";
import { cellToBoundary } from "h3-js";
import { t, type Key } from "./i18n";
import { $, toast } from "./page";

type Mood = { day: string | null; k: number; cells: [string, Record<string, number>][] };

/** Colour and sign per answer; the strongest share colours the cell. */
const LOOK: Record<string, [string, string]> = {
  einladend: ["#69db7c", "✨"],
  ruhig: ["#3bc9db", "🍃"],
  leer: ["#a5d8ff", "○"],
  voll: ["#ffa94d", "👥"],
  laut: ["#ff6b6b", "🔊"],
  unangenehm: ["#e64980", "✖"],
  dunkel: ["#7048e8", "🌑"],
  kalt: ["#74c0fc", "❄"],
};

export function setupMood(map: maplibregl.Map, slug: () => string) {
  const btn = $("nav-mood");
  let on = false;
  let mood: Mood | null = null;

  const top = (s: Record<string, number>) => Object.entries(s).sort((a, b) => b[1] - a[1])[0]?.[0] ?? "";
  const label = (c: string) => t(`cat.${c}` as Key);

  function draw(m: Mood) {
    const features: GeoJSON.Feature[] = m.cells.map(([h3, shares]) => {
      const c = top(shares);
      return {
        type: "Feature",
        properties: { h3, color: LOOK[c]?.[0] ?? "#adb5bd", sign: LOOK[c]?.[1] ?? "" },
        geometry: { type: "Polygon", coordinates: [cellToBoundary(h3, true)] },
      };
    });
    const data: GeoJSON.FeatureCollection = { type: "FeatureCollection", features };
    const src = map.getSource("mood") as maplibregl.GeoJSONSource | undefined;
    if (src) return src.setData(data);
    map.addSource("mood", { type: "geojson", data });
    map.addLayer({ id: "mood", type: "fill", source: "mood", paint: { "fill-color": ["get", "color"], "fill-opacity": 0.35 } });
    map.addLayer({ id: "mood-line", type: "line", source: "mood", paint: { "line-color": ["get", "color"], "line-width": 1.5 } });
    map.addLayer({
      id: "mood-sign",
      type: "symbol",
      source: "mood",
      layout: { "text-field": ["get", "sign"], "text-size": 18, "text-allow-overlap": true },
    });
    map.on("click", "mood", (e) => {
      const h3 = e.features?.[0]?.properties?.h3 as string | undefined;
      const shares = mood?.cells.find((c) => c[0] === h3)?.[1];
      if (!shares) return;
      const text = Object.entries(shares)
        .sort((a, b) => b[1] - a[1])
        .map(([c, p]) => `${label(c)} ${p} %`)
        .join(" · ");
      toast(text, 4000);
    });
  }

  function show(visible: boolean) {
    for (const id of ["mood", "mood-line", "mood-sign"]) {
      if (map.getLayer(id)) map.setLayoutProperty(id, "visibility", visible ? "visible" : "none");
    }
    btn.classList.toggle("on", visible);
  }

  btn.onclick = async () => {
    on = !on;
    if (!on) return show(false);
    try {
      mood = await fetch(`/api/mood?region=${encodeURIComponent(slug())}`).then((r) => (r.ok ? r.json() : Promise.reject(r.status)));
    } catch {
      mood = null;
    }
    if (!on) return;
    if (!mood?.day || !mood.cells.length) {
      on = false;
      show(false);
      return toast(t("mood.none", { k: mood?.k ?? 5 }), 5000);
    }
    draw(mood);
    show(true);
    const day = new Date(`${mood.day}T12:00:00Z`).toLocaleDateString(document.documentElement.lang || undefined, { weekday: "long", day: "numeric", month: "long" });
    toast(t("mood.day", { day }), 4000);
  };
}
