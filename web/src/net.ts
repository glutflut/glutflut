// WebSocket to the server's web gateway, with reconnect. Phones drop sockets
// silently while asleep (the socket still looks open), so the connection is
// probed with ping/pong after silence, on return to the page and on request.
import { arrivalInvite, inviteUsed } from "./invite";
import { arrivalPassport, passportUsed, withRegion } from "./region";
import { KEYS, load, save } from "./store";

import type { ClientMsg, ServerMsg } from "./protocol";
import { watching } from "./watch";

// The protocol types are generated from the server (crates/server/src/protocol.rs).
export type { AbilityName, Age, ChallengeKind, Faction, FindItem, Mine, SourceKind } from "./protocol";
export type { ClientMsg, ServerMsg };

/** Campaign chapter and race (server: campaign.rs). */
export type ChapterMsg = Extract<ServerMsg, { t: "chapter" }>;

function loadToken(): string | null {
  return load(KEYS.token);
}

/** Mirror the token in a first-party cookie (Safari clears script storage
 *  after 7 days without a visit; server cookies stay). Empty = delete it. */
function syncCookie(token: string) {
  fetch("/api/session", { method: "POST", body: token }).catch(() => {});
}

export function clearToken() {
  save(KEYS.token, null);
  syncCookie("");
}

export function saveToken(token: string) {
  save(KEYS.token, token);
  syncCookie(token);
}

/** The proof of work for a new identity, in a worker (pow-worker.ts). */
function solvePow(c: string, bits: number): Promise<string> {
  return new Promise((resolve) => {
    const w = new Worker(new URL("./pow-worker.ts", import.meta.url), { type: "module" });
    w.onmessage = (e: MessageEvent<string>) => {
      w.terminate();
      resolve(e.data);
    };
    w.postMessage({ c, bits });
  });
}

/** Silence after which the connection is probed. */
const QUIET_MS = 20_000;
/** A probe without any answer in this time means the socket is dead. */
const DEADLINE_MS = 5_000;
/** Wait after `busy` (too many new players from this network). */
const BUSY_MS = 30_000;

export class Net {
  private ws: WebSocket | null = null;
  private retry = 500;
  private lastRx = Date.now();
  private probe = 0;
  /** No new socket before this time (after `busy`). */
  private busyUntil = 0;
  private waiting = 0;

  constructor(
    private onMsg: (m: ServerMsg) => void,
    private onStatus: (connected: boolean) => void,
  ) {
    this.connect();
    setInterval(() => {
      if (!document.hidden && Date.now() - this.lastRx > QUIET_MS) this.check();
    }, 5_000);
    document.addEventListener("visibilitychange", () => !document.hidden && this.check());
    addEventListener("online", () => this.check());
  }

  /** False when not connected (a reconnect is then under way). */
  send(m: ClientMsg): boolean {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(m));
      return true;
    }
    if (this.ws?.readyState !== WebSocket.CONNECTING) this.reconnect();
    return false;
  }

  /** Ping; no message at all within the deadline -> reconnect. */
  check() {
    if (this.probe) return;
    const sent = Date.now();
    if (!this.send({ t: "ping" })) return;
    this.probe = window.setTimeout(() => {
      this.probe = 0;
      if (this.lastRx < sent) this.reconnect();
    }, DEADLINE_MS);
  }

  private reconnect() {
    const old = this.ws;
    if (old) {
      old.onclose = old.onmessage = old.onopen = null;
      old.close();
    }
    this.onStatus(false);
    this.retry = 500;
    this.connect();
  }

  private hello(pow?: { c: string; n: string }) {
    const passport = arrivalPassport();
    const invite = arrivalInvite();
    if (watching()) return void this.send({ t: "hello", token: null, tiles: true, watch: true });
    this.send({
      t: "hello",
      token: loadToken(),
      tiles: true,
      ...(passport ? { passport } : {}),
      ...(pow ? { pow } : {}),
      ...(invite ? { invite } : {}),
    });
  }

  private connect() {
    const wait = this.busyUntil - Date.now();
    if (wait > 0) {
      this.waiting ||= window.setTimeout(() => {
        this.waiting = 0;
        this.connect();
      }, wait);
      return;
    }
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${proto}://${location.host}${withRegion("/ws")}`);
    this.ws = ws;
    ws.onopen = () => {
      this.retry = 500;
      this.onStatus(true);
      this.hello();
    };
    ws.onmessage = (e) => {
      this.lastRx = Date.now();
      const m = JSON.parse(e.data) as ServerMsg;
      if (m.t === "pong") return;
      if (m.t === "busy") this.busyUntil = Date.now() + BUSY_MS;
      if (m.t === "pow") {
        // A new identity: solve the server's challenge, then say hello again.
        void solvePow(m.c, m.bits).then((n) => this.ws === ws && this.hello({ c: m.c, n }));
        return;
      }
      if (m.t === "welcome" && !m.watch) {
        saveToken(m.token);
        passportUsed();
        inviteUsed();
      }
      this.onMsg(m);
    };
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.onStatus(false);
      setTimeout(() => this.connect(), this.retry);
      this.retry = Math.min(this.retry * 2, 10_000);
    };
  }
}
