// Approved images of the region (server: editor_media.rs, `GET /api/media`):
// for the find card, the challenge card (Entdecken: the zone) and chapter
// cards, with the credit line under them. Loaded once, when first needed.
import { t } from "./i18n";
import { region, withRegion } from "./region";

export interface Media {
  kind: "find" | "zone" | "challenge";
  target: string;
  url: string;
  w: number;
  h: number;
  author: string;
  licence: string | null;
  licence_url: string | null;
  source: string | null;
  alt: string | null;
  /** Zones: the name (the challenge hint names it). */
  name: string | null;
}

let loaded: { region: string; list: Promise<Media[]> } | null = null;

function all(): Promise<Media[]> {
  if (loaded?.region !== region) {
    const list = fetch(withRegion("/api/media"))
      .then((r) => (r.ok ? (r.json() as Promise<Media[]>) : []))
      .catch(() => []);
    loaded = { region, list };
  }
  return loaded.list;
}

/** The image of a find (id), zone (id or name) or chapter (key), if any. */
export async function mediaFor(kind: Media["kind"], target: string): Promise<Media | undefined> {
  return (await all()).find((m) => m.kind === kind && (m.target === target || (kind === "zone" && m.name === target)));
}

/** Start loading early (no wait on the first card). */
export function preloadMedia() {
  void all();
}

const link = (href: string, text: string) =>
  Object.assign(document.createElement("a"), { href, textContent: text, target: "_blank", rel: "noopener noreferrer" });

/** The image, lazy, with "Photo: author, licence" (links to source and licence). */
export function figure(m: Media): HTMLElement {
  const f = document.createElement("figure");
  f.className = "media";
  const img = Object.assign(document.createElement("img"), { src: m.url, alt: m.alt ?? "", loading: "lazy", decoding: "async", width: m.w, height: m.h });
  const cap = document.createElement("figcaption");
  const [before, after] = t("media.credit", { author: "\u0000" }).split("\u0000");
  cap.append(before ?? "", m.source ? link(m.source, m.author) : m.author, after ?? "");
  if (m.licence) cap.append(", ", m.licence_url ? link(m.licence_url, m.licence) : m.licence);
  f.append(img, cap);
  return f;
}
