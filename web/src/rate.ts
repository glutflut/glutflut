// "Wie ist es hier?" (web.rs `Rate`): the button opens the chips; after
// claims the app also asks by itself now and then, a card that goes away on
// its own. What is asked where lives only in memory (this visit).
import { t } from "./i18n";
import type { Net, ServerMsg } from "./net";
import { CLAIM_RADIUS_M, distM } from "./geo";
import { $, plus, toast } from "./page";

const TOO_FAST = t("too.fast");
/** Ask after the second claim of a visit, then after every third. */
const ASK_EVERY = 3;
/** Not again near a place asked about (or rated) within the hour. */
const ASK_GAP_M = 150;
const ASK_GAP_MS = 60 * 60_000;
/** At most this many cards per visit (ratings pay points up to 5 a day). */
const ASK_MAX = 6;
/** The card comes after the claim's own show, and goes away on its own. */
const ASK_DELAY_MS = 1500;
const ASK_SHOW_MS = 15_000;

export function setupRate(d: { net: Net; nodePos: Map<number, [number, number]>; me: () => [number, number] | null }) {
  const btn = $("rate") as HTMLButtonElement;
  const sheet = $("sheet");
  const sub = $("sheet-sub");
  const close = $("sheet-close") as HTMLButtonElement;
  let claims = ASK_EVERY - 1;
  let asks = 0;
  const places: { ll: [number, number]; at: number }[] = [];
  let timer = 0;

  function show(asked: boolean) {
    clearTimeout(timer);
    sheet.classList.toggle("asked", asked);
    sub.hidden = !asked;
    sheet.hidden = false;
    if (asked) timer = window.setTimeout(hide, ASK_SHOW_MS);
  }
  function hide() {
    clearTimeout(timer);
    sheet.hidden = true;
    sheet.classList.remove("asked");
  }
  const near = (me: [number, number]) => places.some((p) => Date.now() - p.at < ASK_GAP_MS && distM(me, p.ll) < ASK_GAP_M);

  btn.onclick = () => (sheet.hidden ? show(false) : hide());
  close.onclick = hide;
  for (const b of sheet.querySelectorAll<HTMLButtonElement>("button[data-cat]")) {
    b.onclick = () => {
      hide();
      const me = d.me();
      if (!me) return;
      let best: [number, number] | null = null;
      for (const [id, c] of d.nodePos) {
        const dist = distM(me, c);
        if (!best || dist < best[1]) best = [id, dist];
      }
      if (!best || best[1] > CLAIM_RADIUS_M) return toast(t("rate.noplace"));
      places.push({ ll: me, at: Date.now() });
      d.net.send({ t: "rate", node: best[0], cat: b.dataset.cat! });
    };
  }

  return {
    /** A claim of this player took here: maybe ask how it is. */
    onClaimed() {
      claims++;
      const me = d.me();
      if (claims < ASK_EVERY || asks >= ASK_MAX || !me || !sheet.hidden || btn.disabled || near(me)) return;
      claims = 0;
      asks++;
      places.push({ ll: me, at: Date.now() });
      window.setTimeout(() => sheet.hidden && show(true), ASK_DELAY_MS);
    },
    onRate(m: Extract<ServerMsg, { t: "rate" }>) {
      const why = m.reason === "cooldown" ? t("rate.cooldown") : m.reason === "moving" ? TOO_FAST : t("rate.nothere");
      toast(m.ok ? t("rate.thanks", { plus: plus(m.points) }) : why);
    },
  };
}
