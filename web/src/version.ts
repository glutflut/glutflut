// Build version (commit + date), set by vite.config.ts. Fills every `.version`;
// with `source` in /api/version (ELEMENTS_SOURCE_URL) also a source code link,
// and shows the `.source-icon` links next to the social ones.
declare const __VERSION__: string;
export const VERSION = __VERSION__;

const SOURCE: Record<string, string> = {
  de: "Quellcode",
  en: "Source code",
  fr: "Code source",
  es: "Código fuente",
  it: "Codice sorgente",
  pl: "Kod źródłowy",
  pt: "Código-fonte",
};

// Code brackets for a source host other than GitHub.
const CODE = "M8.7 16.3 4.4 12l4.3-4.3-1.4-1.4L1.6 12l5.7 5.7zm6.6 0 4.3-4.3-4.3-4.3 1.4-1.4 5.7 5.7-5.7 5.7z";

const spans = [...document.querySelectorAll<HTMLElement>(".version")];
for (const el of spans) el.textContent = `Version ${VERSION}`;
const icons = [...document.querySelectorAll<HTMLAnchorElement>("a.source-icon")];

if (spans.length || icons.length)
  void fetch("/api/version")
    .then((r) => r.json())
    .then((v: { source?: string }) => {
      if (!v.source?.startsWith("https://")) return;
      const github = new URL(v.source).hostname === "github.com";
      for (const a of icons) {
        a.href = v.source;
        if (!github) {
          const lang = a.closest<HTMLElement>("[data-lang]")?.dataset.lang ?? document.documentElement.lang;
          a.title = a.ariaLabel = SOURCE[lang] ?? SOURCE.en;
          a.querySelector("path")?.setAttribute("d", CODE);
        }
        a.hidden = false;
      }
      for (const el of spans) {
        const lang = el.closest<HTMLElement>("[data-lang]")?.dataset.lang ?? document.documentElement.lang;
        const a = document.createElement("a");
        a.href = v.source;
        a.target = "_blank";
        a.rel = "noopener";
        a.textContent = SOURCE[lang] ?? SOURCE.en;
        el.after(" · ", a);
      }
    })
    .catch(() => {});
