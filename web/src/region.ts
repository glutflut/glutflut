// Which region the app plays: `?region=`, else where the player stands (the
// outlines from /api/regions, tested here, so the position leaves the phone
// only to play), else the last one played. Every request carries the choice.
import { KEYS, load, save } from "./store";

export interface RegionInfo {
  slug: string;
  name: string;
  city: string;
  language: string;
  bounds: [number, number, number, number];
  outline: GeoJSON.Polygon | GeoJSON.MultiPolygon;
  /** False for beta cities: not on the list, but picked by link or location. */
  listed?: boolean;
  /** "im Furttal" per language, where "in {city}" reads wrong. */
  city_in?: Record<string, string> | null;
  /** Set when another server runs it. */
  url?: string | null;
}

const stored = () => load(KEYS.region);

// A passport from the server the player came from (server: shards.rs), taken
// out of the address at once so it is neither shared nor kept in the history.
let passport: string | null = null;
{
  const m = location.hash.match(/[#&]passport=([0-9a-f.]+)/);
  if (m) {
    passport = m[1];
    history.replaceState(history.state, "", location.pathname + location.search);
  }
}

/** The passport for the first hello; `used` once the server welcomed us. */
export function arrivalPassport(): string | null {
  return passport;
}
export function passportUsed() {
  passport = null;
}

/** A passport for another server: this server signs who the player is. */
async function passportFor(): Promise<string | null> {
  // Without one, the cookie stands in.
  const token = load(KEYS.token);
  const res = await fetch("/api/passport", { method: "POST", body: token ?? "" }).catch(() => null);
  if (!res?.ok) return null;
  const v = await res.json().catch(() => null);
  return typeof v?.p === "string" ? v.p : null;
}

/** The chosen slug; empty = the server's default. */
export let region = new URLSearchParams(location.search).get("region") ?? stored() ?? "";

export function setRegion(slug: string) {
  region = slug;
  save(KEYS.region, slug);
}

/** `path` with `region=` added (nothing when the server's default applies). */
export function withRegion(path: string): string {
  if (!region) return path;
  return `${path}${path.includes("?") ? "&" : "?"}region=${encodeURIComponent(region)}`;
}

/** Packed outline (server: `regions::pack_outline`): polygons of rings, each
 * `[x0, y0, dx, dy, ...]` in 1e-4 degrees. */
type Packed = number[][][];

function unpack(ol: Packed): GeoJSON.MultiPolygon {
  return {
    type: "MultiPolygon",
    coordinates: ol.map((poly) =>
      poly.map((flat) => {
        const ring: GeoJSON.Position[] = [];
        let x = 0, y = 0;
        for (let i = 0; i + 1 < flat.length; i += 2) {
          x += flat[i];
          y += flat[i + 1];
          ring.push([x / 1e4, y / 1e4]);
        }
        return ring;
      }),
    ),
  };
}

export async function listRegions(): Promise<RegionInfo[]> {
  // Packed outlines: a third of the size; the ETag saves the rest.
  const res = await fetch("/api/regions?v=2");
  if (!res.ok) return [];
  const list: (Omit<RegionInfo, "outline"> & { ol?: Packed })[] = await res.json();
  return list.map(({ ol, ...r }) => ({ ...r, outline: unpack(ol ?? []) }));
}

function inRing(ring: GeoJSON.Position[], x: number, y: number): boolean {
  let inside = false;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const [xi, yi] = ring[i], [xj, yj] = ring[j];
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}

function inPolygon(rings: GeoJSON.Position[][], x: number, y: number): boolean {
  return inRing(rings[0], x, y) && !rings.slice(1).some((h) => inRing(h, x, y));
}

/** The region at [lon, lat], if any; listed ones first, unlisted (beta) ones too. */
export function regionAt(list: RegionInfo[], [x, y]: [number, number]): RegionInfo | undefined {
  const hit = list.filter((r) =>
    r.outline.type === "Polygon"
      ? inPolygon(r.outline.coordinates, x, y)
      : r.outline.coordinates.some((p) => inPolygon(p, x, y)),
  );
  return hit.find((r) => r.listed !== false) ?? hit[0];
}

/** Distance in km from [lon, lat] to the region's centre (its bounds). */
export function distanceKm(r: RegionInfo, [x, y]: [number, number]): number {
  const cx = (r.bounds[0] + r.bounds[2]) / 2, cy = (r.bounds[1] + r.bounds[3]) / 2;
  const rad = Math.PI / 180;
  const a =
    Math.sin(((cy - y) * rad) / 2) ** 2 +
    Math.cos(y * rad) * Math.cos(cy * rad) * Math.sin(((cx - x) * rad) / 2) ** 2;
  return 12_742 * Math.asin(Math.sqrt(a));
}

/** One position fix as [lon, lat], or null (denied, none, too slow). */
export function locate(timeoutMs: number): Promise<[number, number] | null> {
  if (!("geolocation" in navigator)) return Promise.resolve(null);
  return new Promise((done) => {
    const timer = setTimeout(() => done(null), timeoutMs + 1000);
    navigator.geolocation.getCurrentPosition(
      (p) => {
        clearTimeout(timer);
        done([p.coords.longitude, p.coords.latitude]);
      },
      () => {
        clearTimeout(timer);
        done(null);
      },
      { enableHighAccuracy: false, maximumAge: 300_000, timeout: timeoutMs },
    );
  });
}

/** Open the app for `slug`; on another server with a passport, so the
 *  player stays who they are (same side, name, badges). */
/** `look`: where the map starts there (an own permanent source; same server only). */
export async function openRegion(r: RegionInfo, watch = false, look?: [number, number]) {
  const u = new URL(r.url ? `${r.url}${location.pathname}` : location.href);
  u.search = location.search;
  u.searchParams.set("region", r.slug);
  // A test position (`?at=`) belongs to the old city: there it'd be far off the map.
  u.searchParams.delete("at");
  // From the city list or the globe: only watch there (watch.ts); walking in: play.
  if (watch) u.searchParams.set("watch", "");
  else u.searchParams.delete("watch");
  u.hash = look && !r.url ? `keep=${look[0].toFixed(6)},${look[1].toFixed(6)}` : "";
  if (r.url) {
    const p = await passportFor();
    if (p) u.hash = `passport=${p}`;
  }
  location.href = u.toString();
}

/** "I want to play here": the position rounded to ~1 km; the server keeps a
 *  coarser cell. Once per place and device. */
export async function wish([lon, lat]: [number, number]): Promise<boolean> {
  const key = `elements.wish.${lat.toFixed(2)},${lon.toFixed(2)}` as const;
  if (load(key)) return true;
  const res = await fetch("/api/wish", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ lat: +lat.toFixed(2), lon: +lon.toFixed(2) }),
  }).catch(() => null);
  if (!res?.ok) return false;
  save(key, "1");
  return true;
}
