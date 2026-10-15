import { defineConfig } from "vite";
import basicSsl from "@vitejs/plugin-basic-ssl";
import { execSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import type { Plugin } from "vite";

// Dev: the Rust server's web gateway runs on :5080.
const gateway = process.env.ELEMENTS_GATEWAY ?? "http://127.0.0.1:5080";

// Shown in the app. The image build passes ELEMENTS_VERSION; locally: git.
function version(): string {
  if (process.env.ELEMENTS_VERSION) return process.env.ELEMENTS_VERSION;
  try {
    const rev = execSync("git describe --always --dirty=+", { encoding: "utf8" }).trim();
    return `${rev} ${new Date().toISOString().slice(0, 10)} (lokal)`;
  } catch {
    return "dev";
  }
}

// Credits only some instances carry (e.g. the elevation data of their regions):
// `<!-- credits:<name>:<lang> -->` in a page becomes <dir>/<name>.<lang>.html,
// dir from ELEMENTS_CREDITS_DIR or credits.local/ (copy yours there before an image build).
// Without a file the comment stays.
function credits(): Plugin {
  const dir = process.env.ELEMENTS_CREDITS_DIR ?? "credits.local";
  return {
    name: "credits",
    transformIndexHtml(html) {
      return html.replace(/<!-- credits:([a-z-]+):([a-z]+) -->/g, (m, name, lang) => {
        const f = `${dir}/${name}.${lang}.html`;
        return existsSync(f) ? readFileSync(f, "utf8").trim() : m;
      });
    },
  };
}

export default defineConfig({
  define: { __VERSION__: JSON.stringify(version()) },
  // HTTPS=1: self-signed cert, so phones on the LAN get GPS (secure context).
  plugins: [credits(), ...(process.env.HTTPS ? [basicSsl()] : [])],
  // Pre-bundling breaks MapLibre's worker URL.
  optimizeDeps: { exclude: ["maplibre-gl"] },
  worker: { format: "es" },
  build: {
    rollupOptions: {
      input: {
        main: "index.html",
        dashboard: "dashboard.html",
        editor: "editor.html",
        info: "info.html",
        impressum: "impressum.html",
        datenschutz: "datenschutz.html",
        sounds: "sounds.html",
      },
    },
  },
  server: {
    host: true,
    proxy: {
      "/api": gateway,
      "/ws": { target: gateway.replace(/^http/, "ws"), ws: true },
    },
  },
});
