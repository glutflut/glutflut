// News flashes of the region (server: campaign.rs `notices`, editor): a
// story card once per device while one runs, and all running ones under
// "what's new" (news.ts).
import { lang, t } from "./i18n";
import type { ServerMsg } from "./net";
import { KEYS, loadJson, saveJson } from "./store";
import { pickLang, showStory } from "./story";

export type Notice = Extract<ServerMsg, { t: "notices" }>["items"][number];

let active: Notice[] = [];

function seen(): number[] {
  return loadJson<number[]>(KEYS.noticesSeen) ?? [];
}

/** The notices running now, newest first. */
export function activeNotices(): Notice[] {
  return active;
}

export function text(n: Notice): { title: string; text: string } {
  return pickLang(n.names, lang) ?? { title: "", text: "" };
}

export function onNotices(items: Notice[]) {
  active = items;
  const was = seen();
  const fresh = items.filter((n) => !was.includes(n.id));
  if (!fresh.length) return;
  saveJson(KEYS.noticesSeen, [...was, ...fresh.map((n) => n.id)].slice(-40));
  // Oldest first, so the newest ends on top.
  for (const n of [...fresh].reverse()) {
    const x = text(n);
    if (!x.title) continue;
    showStory({ cls: "notice", kicker: t("notice.kicker"), title: x.title, text: x.text, url: n.url, urlLabel: t("notice.more") });
  }
}
