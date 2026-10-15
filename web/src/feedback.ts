// Feedback (server: gateway.rs `/api/feedback`): a mood (three faces) and an
// optional text, stored with city, day, language and version, without the
// player or a position. Opened from the settings.
import { lang, t } from "./i18n";
import { withRegion } from "./region";
import { VERSION } from "./version";
import { $ } from "./page";

export function setupFeedback(toast: (s: string, ms?: number) => void) {
  const box = $("feedback");
  const text = $("feedback-text") as HTMLTextAreaElement;
  const send = $("feedback-send") as HTMLButtonElement;
  let mood = 0;
  const faces = [...box.querySelectorAll<HTMLButtonElement>("button[data-mood]")];
  const pick = (m: number) => {
    mood = m;
    for (const f of faces) f.classList.toggle("on", Number(f.dataset.mood) === m);
    send.disabled = !mood;
  };
  for (const f of faces) f.onclick = () => pick(Number(f.dataset.mood));
  const close = () => (box.hidden = true);
  $("feedback-close").onclick = close;
  $("feedback-open").onclick = () => {
    // From the settings: they close, the form takes their place.
    ($("board-close") as HTMLButtonElement).click();
    pick(0);
    text.value = "";
    box.hidden = false;
  };
  send.onclick = async () => {
    if (!mood) return;
    send.disabled = true;
    const res = await fetch(withRegion("/api/feedback"), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ mood, text: text.value.slice(0, 1000), lang, version: VERSION }),
    }).catch(() => null);
    send.disabled = false;
    if (res?.ok) {
      close();
      toast(t("feedback.thanks"), 4000);
    } else toast(t(res?.status === 429 ? "feedback.wait" : "feedback.failed"));
  };
}
