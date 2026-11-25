// Walked rings (server `ring.rs`): a closed loop glows in its side's colour
// while it holds (claims of the other side inside are weaker).
import type * as maplibregl from "maplibre-gl";
import { GLOW, CORE } from "./geo";
import type { RingView } from "./protocol";

export function setupRings(map: maplibregl.Map) {
  map.addSource("rings", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
  const color = ["match", ["get", "side"], "fire", GLOW.fire, GLOW.water] as maplibregl.ExpressionSpecification;
  const core = ["match", ["get", "side"], "fire", CORE.fire, CORE.water] as maplibregl.ExpressionSpecification;
  map.addLayer({
    id: "rings-glow",
    type: "line",
    source: "rings",
    layout: { "line-join": "round", "line-cap": "round" },
    paint: { "line-color": color, "line-width": ["interpolate", ["linear"], ["zoom"], 13, 4, 18, 18], "line-blur": 6, "line-opacity": 0.55 },
  });
  map.addLayer({
    id: "rings",
    type: "line",
    source: "rings",
    layout: { "line-join": "round", "line-cap": "round" },
    paint: { "line-color": core, "line-width": ["interpolate", ["linear"], ["zoom"], 13, 1, 18, 3], "line-dasharray": [2, 1.5], "line-opacity": 0.9 },
  });
  let items: RingView[] = [];
  const draw = () => {
    const now = Date.now() / 1000;
    const features: GeoJSON.Feature[] = items
      .filter((r) => r.until > now && r.line.length > 2)
      .map((r) => ({ type: "Feature", properties: { side: r.side }, geometry: { type: "LineString", coordinates: [...r.line, r.line[0]] } }));
    (map.getSource("rings") as maplibregl.GeoJSONSource | undefined)?.setData({ type: "FeatureCollection", features });
  };
  // Rings run out on their own (the server says so too, within seconds).
  setInterval(() => items.length && draw(), 10_000);
  return {
    set(next: RingView[]) {
      items = next;
      draw();
    },
  };
}
