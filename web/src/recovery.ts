// Recovery code (server: score.rs `recovery`): shown only after a tap, with a
// clear warning; saved by the player (mail to self, share, copy, QR), never
// by us in plain text.
import qrcode from "qrcode-generator";
import { t } from "./i18n";
import type { Net } from "./net";
import { $, toast } from "./page";
import { ask } from "./ask";

export function setupRecovery(net: Net) {
  const box = $("keep-code");
  let code = "";
  const ios = /iP(hone|ad|od)/.test(navigator.userAgent) && !(navigator as Navigator & { standalone?: boolean }).standalone;
  ($("keep").querySelector(".keep-ios") as HTMLElement).hidden = !ios;
  $("keep-show").onclick = () => net.send({ t: "code" });
  const text = () => t("keep.mail.body", { code }).replace(/\\n/g, "\n");
  $("keep-share").onclick = () => {
    if (navigator.share) navigator.share({ title: t("keep.mail.subject"), text: text() }).catch(() => {});
    else navigator.clipboard?.writeText(text());
  };
  $("keep-copy").onclick = () => {
    navigator.clipboard?.writeText(code).then(() => toast(t("keep.copied")), () => {});
  };
  $("keep-qr-btn").onclick = () => {
    const qr = $("keep-qr");
    if (qr.hidden) {
      const q = qrcode(0, "M");
      q.addData(code);
      q.make();
      qr.innerHTML = q.createSvgTag({ cellSize: 5, margin: 2, scalable: true });
    }
    qr.hidden = !qr.hidden;
  };
  $("keep-redeem").onclick = async () => {
    const v = ($("keep-input") as HTMLInputElement).value.trim();
    if (v && (await ask(t("keep.redeem.confirm")))) net.send({ t: "recover", code: v });
  };
  return {
    show(c: string) {
      code = c;
      $("keep-value").textContent = c;
      ($("keep-mail") as HTMLAnchorElement).href =
        `mailto:?subject=${encodeURIComponent(t("keep.mail.subject"))}&body=${encodeURIComponent(text())}`;
      $("keep-qr").hidden = true;
      box.hidden = false;
    },
  };
}
