// Invites (server: score.rs `invite_hello`): a link with the player's code in
// the address hash (never sent to the server as part of the URL). A browser
// that already plays ignores it; a new player sends it once with the hello.
// It counts for the inviter only after the friend really played (enough
// claims, back on a later day).
import qrcode from "qrcode-generator";
import { t } from "./i18n";
import { $, toast } from "./page";
import { region } from "./region";
import { KEYS, load, save } from "./store";

const PENDING = "elements.invite";

{
  const m = location.hash.match(/[#&]i=([0-9a-f]{12})/);
  if (m) {
    history.replaceState(history.state, "", location.pathname + location.search);
    // Kept for this tab until the welcome (an update may reload the app first).
    if (!load(KEYS.token)) {
      try {
        sessionStorage.setItem(PENDING, m[1]);
      } catch {}
    }
  }
}

/** The code to send with the hello: only from a browser that doesn't play yet. */
export function arrivalInvite(): string | null {
  if (load(KEYS.token)) return null;
  try {
    return sessionStorage.getItem(PENDING);
  } catch {
    return null;
  }
}

export function inviteUsed() {
  try {
    sessionStorage.removeItem(PENDING);
  } catch {}
}

/** Claims in a day before the nav button pulses once (a nudge, no popup). */
const NUDGE_AFTER = 3;
const today = () => Math.floor(Date.now() / 86_400_000);

export function setupInvite() {
  const card = $("invite");
  const navBtn = $("nav-invite");
  let url = "";
  let claims = 0;
  const text = () => t("invite.text");
  function open() {
    const q = qrcode(0, "M");
    q.addData(url);
    q.make();
    $("invite-qr").innerHTML = q.createSvgTag({ cellSize: 5, margin: 2, scalable: true });
    navBtn.classList.remove("nudge");
    card.hidden = false;
  }
  navBtn.onclick = () => (card.hidden ? open() : (card.hidden = true));
  $("invite-me-open").onclick = open;
  $("invite-close").onclick = () => (card.hidden = true);
  $("invite-share").onclick = () => {
    if (navigator.share) navigator.share({ title: "Glut & Flut", text: text(), url }).catch(() => {});
    else navigator.clipboard?.writeText(`${text()} ${url}`).then(() => toast(t("invite.copied")), () => {});
  };
  $("invite-copy").onclick = () => {
    navigator.clipboard?.writeText(url).then(() => toast(t("invite.copied")), () => {});
  };
  return {
    welcome(code: string, points: number) {
      navBtn.hidden = $("invite-me").hidden = !code;
      if (!code) return;
      const q = region ? `?region=${encodeURIComponent(region)}` : "";
      url = `${location.origin}/${q}#i=${code}`;
      $("invite-note").textContent = t("invite.note", { n: points });
    },
    /** A claim took: on the third of the day, once a day, the button pulses. */
    claimed() {
      if (navBtn.hidden || ++claims !== NUDGE_AFTER) return;
      if (load(KEYS.inviteNudge) === String(today())) return;
      save(KEYS.inviteNudge, String(today()));
      navBtn.classList.remove("nudge");
      void navBtn.offsetWidth;
      navBtn.classList.add("nudge");
    },
    paid(points: number) {
      if (points > 0) toast(t("invite.paid", { n: points }), 5000);
    },
  };
}
