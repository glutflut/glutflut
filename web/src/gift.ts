// Gifts (server: keep.rs keys, far.rs): the owner of a permanent source hands
// out a key to it as a link with `#gift=<region>.<key>` in the address hash
// (never sent to a server as part of the URL). The receiving app keeps up to
// three; "Verbinden …" offers them, and claims at an own source go through.
import qrcode from "qrcode-generator";
import { t } from "./i18n";
import { $, toast } from "./page";
import { KEYS, loadJson, saveJson } from "./store";

/** `origin`: given by someone (default), found (Funkenflug, Flaschenpost),
 *  or from the city of that region (editor, `#gift=<region>.<key>.city`). */
export type Gift = { region: string; key: string; at: number; origin?: "gift" | "ember" | "bottle" | "city" };

/** Keys kept at most (the oldest goes). */
const MAX = 3;
/** The server's `gift_days`; older ones are dropped here too (city keys: up to 90 days). */
const KEEP_MS = 14 * 86_400_000;
const CITY_MS = 90 * 86_400_000;

export function gifts(): Gift[] {
  const v = loadJson<Gift[]>(KEYS.gifts);
  const now = Date.now();
  return (Array.isArray(v) ? v : []).filter(
    (g) => typeof g?.region === "string" && /^[0-9a-f]{32}$/.test(g?.key ?? "") && now - g.at < (g.origin === "city" ? CITY_MS : KEEP_MS),
  );
}

/** Keep a key (given or found); the oldest goes beyond three. */
export function addGift(g: Gift) {
  saveJson(KEYS.gifts, [...gifts().filter((x) => x.key !== g.key), g].slice(-MAX));
}

export function dropGift(key: string) {
  const list = gifts().filter((g) => g.key !== key);
  saveJson(KEYS.gifts, list.length ? list : null);
}

/** A key just received, for the toast once the app runs. */
let arrived: Gift | null = null;
{
  const m = location.hash.match(/[#&]gift=([a-z0-9-]+)\.([0-9a-f]{32})(\.city)?/);
  if (m) {
    history.replaceState(history.state, "", location.pathname + location.search);
    const g: Gift = { region: m[1], key: m[2], at: Date.now(), ...(m[3] ? { origin: "city" as const } : {}) };
    addGift(g);
    arrived = g;
  }
}

/** The key that came with this visit (once), with its region's slug. */
export function arrivedGift(): Gift | null {
  const g = arrived;
  arrived = null;
  return g;
}

/** Show a key for someone else: QR, copy, share. */
export function showGift(region: string, key: string) {
  const url = `${location.origin}/#gift=${region}.${key}`;
  const q = qrcode(0, "M");
  q.addData(url);
  q.make();
  $("gift-qr").innerHTML = q.createSvgTag({ cellSize: 5, margin: 2, scalable: true });
  const card = $("gift");
  $("gift-close").onclick = () => (card.hidden = true);
  $("gift-copy").onclick = () => {
    navigator.clipboard?.writeText(url).then(() => toast(t("invite.copied")), () => {});
  };
  $("gift-share").onclick = () => {
    const text = t("gift.text");
    if (navigator.share) navigator.share({ title: "Glut & Flut", text, url }).catch(() => {});
    else navigator.clipboard?.writeText(`${text} ${url}`).then(() => toast(t("invite.copied")), () => {});
  };
  card.hidden = false;
}
