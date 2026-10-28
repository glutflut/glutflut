// Invites (server: score.rs `invite_hello`): a link with the player's code in
// the address hash (never sent to the server as part of the URL). A browser
// that already plays ignores it; a new player sends it once with the hello.
// It counts for the inviter only after the friend really played (enough
// claims, back on a later day).
import qrcode from "qrcode-generator";
import { t } from "./i18n";
import { $, toast } from "./page";
import { region } from "./region";
import { KEYS, load } from "./store";

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

export function setupInvite() {
  const box = $("invite");
  let url = "";
  const text = () => t("invite.text");
  $("invite-share").onclick = () => {
    if (navigator.share) navigator.share({ title: "Glut & Flut", text: text(), url }).catch(() => {});
    else navigator.clipboard?.writeText(`${text()} ${url}`).then(() => toast(t("invite.copied")), () => {});
  };
  $("invite-copy").onclick = () => {
    navigator.clipboard?.writeText(url).then(() => toast(t("invite.copied")), () => {});
  };
  $("invite-qr-btn").onclick = () => {
    const qr = $("invite-qr");
    if (qr.hidden) {
      const q = qrcode(0, "M");
      q.addData(url);
      q.make();
      qr.innerHTML = q.createSvgTag({ cellSize: 5, margin: 2, scalable: true });
    }
    qr.hidden = !qr.hidden;
  };
  return {
    welcome(code: string, points: number) {
      box.hidden = !code;
      if (!code) return;
      const q = region ? `?region=${encodeURIComponent(region)}` : "";
      url = `${location.origin}/${q}#i=${code}`;
      $("invite-note").textContent = t("invite.note", { n: points });
      $("invite-qr").hidden = true;
    },
    paid(points: number) {
      if (points > 0) toast(t("invite.paid", { n: points }), 5000);
    },
  };
}
