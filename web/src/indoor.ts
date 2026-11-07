// Indoors (server: indoor.rs, `[indoor]`): no GPS; the player scans a
// junction's code (sheet on the wall, or a screen) and stands there for a
// while. The camera scanner uses BarcodeDetector where it works, else jsQR
// (loaded only then). A phone camera app opens the game link itself
// (`?q=`): that code is sent once the server says the region is indoor.
import { t } from "./i18n";
import { codeFromScan } from "./indoor-code";
import type { Net } from "./net";
import { $, toast } from "./page";

// Kept for this tab across the reload that joining makes.
const PENDING = "elements.scan";
const pendingGet = () => {
  try {
    return sessionStorage.getItem(PENDING);
  } catch {
    return null;
  }
};
const pendingSet = (code: string | null) => {
  try {
    if (code) sessionStorage.setItem(PENDING, code);
    else sessionStorage.removeItem(PENDING);
  } catch {}
};

/** Take `?q=` from the address (call once at start; a shared link doesn't pass it on). */
export function takeScanCode() {
  const u = new URL(location.href);
  const q = u.searchParams.get("q");
  const code = q && codeFromScan(q);
  if (code) pendingSet(code);
  if (u.searchParams.has("q")) {
    u.searchParams.delete("q");
    history.replaceState(history.state, "", u);
  }
}

/** A code came with the link: whoever scans one is there and wants to play. */
export const scanPending = () => pendingGet() !== null;

type Detector = { detect(src: CanvasImageSource): Promise<{ rawValue: string }[]> };

async function detector(): Promise<(v: HTMLVideoElement, c: HTMLCanvasElement) => Promise<string | null>> {
  const BD = (window as unknown as { BarcodeDetector?: { new (o: object): Detector; getSupportedFormats(): Promise<string[]> } }).BarcodeDetector;
  // Chromium on Android without Play services lists no formats.
  if (BD && (await BD.getSupportedFormats().catch((): string[] => [])).includes("qr_code")) {
    const d = new BD({ formats: ["qr_code"] });
    return async (v) => (await d.detect(v).catch(() => []))[0]?.rawValue ?? null;
  }
  const jsQR = (await import("jsqr")).default;
  return async (v, c) => {
    const w = Math.min(640, v.videoWidth);
    const h = Math.round((v.videoHeight * w) / Math.max(1, v.videoWidth));
    if (!w || !h) return null;
    c.width = w;
    c.height = h;
    const g = c.getContext("2d", { willReadFrequently: true })!;
    g.drawImage(v, 0, 0, w, h);
    return jsQR(g.getImageData(0, 0, w, h).data, w, h, { inversionAttempts: "attemptBoth" })?.data ?? null;
  };
}

export function setupIndoor(d: {
  net: Net;
  /** Placed by the server at a junction, or nowhere (`null`). */
  place: (p: [number, number] | null) => void;
}) {
  let on = false;
  let until = 0;
  let timer = 0;
  let stream: MediaStream | null = null;
  let scanning = false;
  const box = $("scanner");
  const video = box.querySelector("video") as HTMLVideoElement;
  const canvas = document.createElement("canvas");

  const send = (code: string) => d.net.send({ t: "scan", code });

  function close() {
    scanning = false;
    stream?.getTracks().forEach((tr) => tr.stop());
    stream = null;
    video.srcObject = null;
    box.hidden = true;
  }

  async function open() {
    if (scanning) return;
    if (!navigator.mediaDevices?.getUserMedia) return void toast(t("indoor.nocamera"), 6000);
    box.hidden = false;
    scanning = true;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: "environment" }, audio: false });
    } catch {
      close();
      return void toast(t("indoor.nocamera"), 6000);
    }
    if (!scanning) return close();
    video.srcObject = stream;
    await video.play().catch(() => {});
    const read = await detector();
    while (scanning) {
      const text = await read(video, canvas);
      const code = text && codeFromScan(text);
      if (code) {
        close();
        navigator.vibrate?.(40);
        send(code);
        return;
      }
      if (text) $("scanner-hint").textContent = t("indoor.foreign");
      await new Promise((r) => setTimeout(r, 200));
    }
  }

  function tick() {
    const left = Math.round((until - Date.now()) / 1000);
    if (left > 0) {
      $("gps").textContent = t("indoor.here", { t: `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}` });
      return;
    }
    clearInterval(timer);
    timer = 0;
    d.place(null);
    $("gps").textContent = t("indoor.scan");
  }

  $("scan").onclick = () => void open();
  $("scanner-close").onclick = close;

  return {
    /** From the welcome: indoor or not (a reconnect keeps the server's hold). */
    welcome(indoor: boolean) {
      on = indoor;
      document.body.classList.toggle("indoor", on);
      $("scan").hidden = !on;
      if (!on) return;
      if (!timer) $("gps").textContent = t("indoor.scan");
      $("gps").onclick = () => void open();
      const code = pendingGet();
      pendingSet(null);
      if (code) send(code);
    },
    answer(m: { ok: boolean; reason?: string; lat?: number; lon?: number; hold_s: number }) {
      if (!m.ok || m.lat === undefined || m.lon === undefined) {
        const why = m.reason === "old" ? "indoor.old" : m.reason === "moving" ? "indoor.moving" : m.reason === "screen" ? "indoor.screen" : "indoor.unknown";
        return void toast(t(why), 5000);
      }
      until = Date.now() + m.hold_s * 1000;
      d.place([m.lon, m.lat]);
      tick();
      timer ||= window.setInterval(tick, 1000);
    },
    active: () => on,
  };
}
