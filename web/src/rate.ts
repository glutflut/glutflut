// "Wie ist es hier?" (web.rs `Rate`): three pairs (voll/leer, laut/ruhig,
// unangenehm/einladend), what stands out, and a question of the city
// (editor, `questions`) on top where one runs. One tap per row; the card
// sends all of it as one rating when done, closed or timed out. The button
// opens it; after claims the app also asks by itself now and then (in the
// area of a city question at the next claim). What is asked where lives only
// in memory (this visit).
import { lang, t } from "./i18n";
import type { Net, ServerMsg } from "./net";
import type { Question } from "./protocol";
import { pickLang } from "./story";
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
/** Wait this many seconds at most for an open story card to be closed. */
const ASK_WAIT_TRIES = 30;

export function setupRate(d: { net: Net; nodePos: Map<number, [number, number]>; me: () => [number, number] | null }) {
  const btn = $("rate") as HTMLButtonElement;
  const row = btn.parentElement!;
  const sheet = $("sheet");
  const sub = $("sheet-sub");
  const close = $("sheet-close") as HTMLButtonElement;
  const done = $("sheet-done") as HTMLButtonElement;
  const qBox = $("sheet-q");
  const qTitle = $("sheet-q-title");
  const qChips = $("sheet-q-chips");
  let questions: Question[] = [];
  /** City questions answered this visit. */
  const answered = new Set<number>();
  let claims = ASK_EVERY - 2;
  let asks = 0;
  const places: { ll: [number, number]; at: number }[] = [];
  let timer = 0;
  /** This card's choices: per pair, per tag, the city's answer. */
  const picked = new Map<string, string>();
  let shownQ: Question | null = null;

  /** The city's question for where `me` stands, if any not answered yet. */
  function cityQuestion(me: [number, number] | null): Question | null {
    if (!me) return null;
    return questions.find((q) => !answered.has(q.id) && (!q.at || distM(me, q.at) <= q.radius_m)) ?? null;
  }
  function drawQuestion(q: Question | null) {
    shownQ = q;
    qBox.hidden = !q;
    qChips.replaceChildren();
    if (!q) return;
    qTitle.textContent = pickLang(q.names, lang) ?? "";
    q.answers.forEach((a, i) => {
      const b = document.createElement("button");
      b.textContent = pickLang(a, lang) ?? "";
      b.dataset.cat = `q${q.id}.${i}`;
      b.onclick = () => choose("city", b);
      qChips.append(b);
    });
  }
  /** One choice per group (a pair, the city's question); a tag is a group of its own. */
  function choose(group: string, b: HTMLButtonElement) {
    const cat = b.dataset.cat!;
    if (picked.get(group) === cat) picked.delete(group);
    else picked.set(group, cat);
    const chosen = new Set(picked.values());
    for (const x of sheet.querySelectorAll<HTMLButtonElement>("button[data-cat]")) x.classList.toggle("on", chosen.has(x.dataset.cat!));
    done.hidden = !picked.size;
    // Still answering: the card stays a while longer.
    if (sheet.classList.contains("asked")) {
      clearTimeout(timer);
      timer = window.setTimeout(finish, ASK_SHOW_MS);
    }
  }
  function show(asked: boolean) {
    clearTimeout(timer);
    picked.clear();
    for (const x of sheet.querySelectorAll<HTMLButtonElement>("button[data-cat]")) x.classList.remove("on");
    done.hidden = true;
    const q = cityQuestion(d.me());
    drawQuestion(q);
    sheet.classList.toggle("asked", asked);
    sub.hidden = !asked || !!q;
    // Just above the buttons (the footer grows with its hints).
    sheet.style.bottom = `${Math.max(0, innerHeight - row.getBoundingClientRect().top) + 8}px`;
    sheet.hidden = false;
    if (asked) timer = window.setTimeout(finish, ASK_SHOW_MS);
  }
  /** Close the card; what was chosen goes out as one rating. */
  function finish() {
    clearTimeout(timer);
    sheet.hidden = true;
    sheet.classList.remove("asked");
    const cats = [...picked.values()];
    picked.clear();
    if (!cats.length) return;
    const me = d.me();
    if (!me) return;
    let best: [number, number] | null = null;
    for (const [id, c] of d.nodePos) {
      const dist = distM(me, c);
      if (!best || dist < best[1]) best = [id, dist];
    }
    if (!best || best[1] > CLAIM_RADIUS_M) return toast(t("rate.noplace"));
    const q = shownQ;
    if (q && cats.some((c) => c.startsWith(`q${q.id}.`))) answered.add(q.id);
    places.push({ ll: me, at: Date.now() });
    const [cat, ...more] = cats;
    d.net.send({ t: "rate", node: best[0], cat, ...(more.length ? { more } : {}) });
  }
  const near = (me: [number, number]) => places.some((p) => Date.now() - p.at < ASK_GAP_MS && distM(me, p.ll) < ASK_GAP_M);

  btn.onclick = () => (sheet.hidden ? show(false) : finish());
  close.onclick = finish;
  done.onclick = finish;
  sheet.querySelectorAll<HTMLElement>(".pair").forEach((p, i) => {
    for (const b of p.querySelectorAll<HTMLButtonElement>("button[data-cat]")) b.onclick = () => choose(`pair${i}`, b);
  });
  for (const b of sheet.querySelectorAll<HTMLButtonElement>("button[data-tag]")) b.onclick = () => choose(`tag:${b.dataset.cat}`, b);

  return {
    /** A claim of this player took here: maybe ask how it is. */
    onClaimed() {
      claims++;
      const me = d.me();
      // In the area of a city question: ask at once (once per visit).
      const due = claims >= ASK_EVERY || !!cityQuestion(me);
      if (!due || asks >= ASK_MAX || !me || !sheet.hidden || btn.disabled || near(me)) return;
      claims = 0;
      asks++;
      places.push({ ll: me, at: Date.now() });
      // Not over a story card (side, chapter, notice): after it is closed.
      let tries = 0;
      const ask = () => {
        if (!sheet.hidden) return;
        if (!$("story").hidden) return void (++tries < ASK_WAIT_TRIES && window.setTimeout(ask, 1000));
        show(true);
      };
      window.setTimeout(ask, ASK_DELAY_MS);
    },
    onQuestions(items: Question[]) {
      questions = items;
    },
    onRate(m: Extract<ServerMsg, { t: "rate" }>) {
      const why = m.reason === "cooldown" ? t("rate.cooldown") : m.reason === "moving" ? TOO_FAST : t("rate.nothere");
      toast(m.ok ? t("rate.thanks", { plus: plus(m.points) }) : why);
    },
  };
}
