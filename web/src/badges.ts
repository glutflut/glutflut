// Badge names (server: score.rs grants them by key): claims-N, level-N,
// cities-N, coll:<city>:<collection>.
import { chapterTitle } from "./campaign";
import { t, type Key } from "./i18n";

export function badgeLabel(key: string): string {
  const [kind, a, b] = key.split(/[-:]/);
  if (key.startsWith("chapter:")) return t("badge.chapter", { title: chapterTitle(key.slice(8)) });
  if (key.startsWith("chapter-win:")) return t("badge.chapter.win", { title: chapterTitle(key.slice(12)) });
  if (key.startsWith("coll:")) {
    const city = a.charAt(0).toUpperCase() + a.slice(1);
    return t("badge.coll", { coll: t(`finds.c.${b}` as Key), city });
  }
  const n = Number(a);
  if (kind === "claims") return n === 1 ? t("badge.claims.1") : t("badge.claims", { n });
  if (kind === "level") return t("badge.level", { n });
  if (kind === "cities") return t("badge.cities", { n });
  if (kind === "streak") return t("badge.streak", { n });
  if (kind === "presence") return n === 1 ? t("badge.presence.1") : t("badge.presence", { n });
  return key;
}
