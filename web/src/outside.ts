// Before the game: pick the region (see region.ts). Outside every region, or
// without a position, a page instead: what Elements is, the explainer video,
// the cities to play in, and "I want to play here".
import { inCity, lang, t } from "./i18n";
import { distanceKm, listRegions, locate, openRegion, region, regionAt, setRegion, wish, type RegionInfo } from "./region";
import { $ } from "./page";
import { watching } from "./watch";

const params = new URLSearchParams(location.search);

/** All active regions, once known (for the hint when walking into another). */
let regions: RegionInfo[] = [];

/** The region list as loaded by `chooseRegion`. */
export const knownRegions = () => regions;

/** True: play `region`. False: the page is shown instead. */
export async function chooseRegion(): Promise<boolean> {
  regions = await listRegions().catch(() => []);
  // An older server without the list: its one region.
  if (!regions.length) return true;
  const asked = params.get("region");
  if (asked) {
    const r = regions.find((r) => r.slug === asked);
    if (!r) return show(null), false;
    if (r.url) return openRegion(r), false;
    setRegion(r.slug);
    return true;
  }
  // Test servers: ?at=lat,lon stands in for the position.
  const at = params.get("at")?.split(",").map(Number);
  const pos: [number, number] | null = at?.length === 2 ? [at[1], at[0]] : await locate(8000);
  if (pos) {
    const here = regionAt(regions, pos);
    if (!here) return show(pos), false;
    if (here.url) return openRegion(here), false;
    setRegion(here.slug);
    return true;
  }
  // No position: the last city played, if any.
  if (region && regions.some((r) => r.slug === region && !r.url)) return true;
  show(null);
  return false;
}

function show(pos: [number, number] | null) {
  $("intro").hidden = true;
  $("outside").hidden = false;
  document.body.classList.add("outside");
  // Beta cities (not listed) only by link or location.
  const listed = regions.filter((r) => r.listed !== false);
  const nearest = pos ? [...listed].sort((a, b) => distanceKm(a, pos) - distanceKm(b, pos)) : listed;
  $("out-title").textContent = pos ? t("out.title") : t("out.title.nopos");
  $("out-lead").textContent = pos
    ? t("out.lead", { n: listed.length, city: nearest[0].city, km: Math.round(distanceKm(nearest[0], pos)) })
    : t("out.lead.nopos");
  $("out-locate").hidden = !!pos;
  $("out-locate").onclick = () => location.reload();

  const list = $("out-cities");
  list.replaceChildren(
    ...nearest.map((r) => {
      const li = document.createElement("li");
      const b = document.createElement("button");
      b.textContent = r.city;
      if (pos) {
        const km = document.createElement("span");
        km.className = "km";
        km.textContent = `${Math.round(distanceKm(r, pos))} km`;
        b.append(km);
      }
      b.onclick = () => openRegion(r);
      li.append(b);
      return li;
    }),
  );

  const wishBox = $("out-wish-box");
  wishBox.hidden = !pos;
  const btn = $("out-wish") as HTMLButtonElement;
  btn.onclick = async () => {
    if (!pos) return;
    btn.disabled = true;
    const ok = await wish(pos);
    btn.textContent = ok ? t("out.wish.done") : t("out.wish.error");
    btn.disabled = ok;
  };

  // The explainer in the reader's language (English if there is none).
  const video = $("out-video") as HTMLVideoElement;
  const src = async (l: string) => {
    const url = `/media/elements-${l}.mp4`;
    const res = await fetch(url, { method: "HEAD" }).catch(() => null);
    return res?.ok && res.headers.get("content-type")?.startsWith("video/") ? url : null;
  };
  void (async () => {
    const url = (await src(lang)) ?? (await src("en"));
    if (url) {
      video.src = url;
      video.hidden = false;
    }
  })();
}

/** Opens the city list (set up by `setupCities`). */
export let openCities = () => {};

/** While playing: the city list to watch another region (bottom line, profile). */
export function setupCities() {
  const here = regions.find((r) => r.slug === region);
  const pick = $("city-pick");
  pick.textContent = here?.city ?? "";
  const open = () => {
    const centre: [number, number] | null = here
      ? [(here.bounds[0] + here.bounds[2]) / 2, (here.bounds[1] + here.bounds[3]) / 2]
      : null;
    const all = regions.filter((r) => r.listed !== false || r.slug === region);
    if (centre) all.sort((a, b) => distanceKm(a, centre) - distanceKm(b, centre));
    else all.sort((a, b) => a.city.localeCompare(b.city, lang));
    // Search by name (accents and case don't matter): the list narrows as one types.
    const fold = (s: string) => s.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();
    const search = $("cities-search") as HTMLInputElement;
    const draw = () => {
      const q = fold(search.value.trim());
      const list = q ? all.filter((r) => fold(`${r.city} ${r.name}`).includes(q)) : all;
      $("cities-none").hidden = list.length > 0;
      $("cities-list").replaceChildren(
      ...list.map((r) => {
        const li = document.createElement("li");
        const b = document.createElement("button");
        b.textContent = r.city;
        if (r.slug === region) {
          b.classList.add("on");
          b.setAttribute("aria-current", "true");
        } else if (centre) {
          const km = document.createElement("span");
          km.className = "km";
          km.textContent = `${Math.round(distanceKm(r, centre))} km`;
          b.append(km);
        }
        b.onclick = () => (r.slug === region ? ($("cities").hidden = true) : openRegion(r, true));
        li.append(b);
        return li;
      }),
      );
    };
    search.value = "";
    search.oninput = draw;
    // Enter picks the first match.
    search.onkeydown = (e) => {
      if (e.key === "Enter") $("cities-list").querySelector("button")?.click();
    };
    draw();
    $("cities").hidden = false;
    if (matchMedia("(hover: hover)").matches) search.focus();
  };
  openCities = open;
  pick.onclick = open;
  $("cities-open").onclick = (e) => {
    e.preventDefault();
    open();
  };
  $("cities-close").onclick = () => ($("cities").hidden = true);
  // Playing: watch this city instead (no claims, the globe when zoomed out).
  const watchHere = $("cities-watch");
  const eye = $("eye");
  watchHere.hidden = eye.hidden = watching();
  watchHere.onclick = eye.onclick = () => {
    const u = new URL(location.href);
    u.searchParams.set("watch", "");
    u.searchParams.delete("at");
    location.href = u.toString();
  };
}

let hinted = "";

/** While playing: standing in another region -> offer to switch there. */
export function checkRegion(pos: [number, number]) {
  const here = regionAt(regions, pos);
  if (!here || here.slug === region || here.slug === hinted) return;
  hinted = here.slug;
  $("switch-text").textContent = t("out.switch", { city: here.city, in_city: inCity(here.city, here.city_in) });
  $("switch-go").onclick = () => openRegion(here);
  $("switch-close").onclick = () => ($("switch").hidden = true);
  $("switch").hidden = false;
}
