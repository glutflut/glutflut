// Carrying you can feel (vault wilde-ideen.md, "Krug schwappt"): a load's
// fill drops while it's carried. Water spills when the phone tilts away from
// how it was held at pick-up, or when the carrier runs; fire burns down when
// the carrier walks into the real wind (shield it: take the way with the wind).
// Kind to children: spilling alone never takes a load below `FLOOR`, small
// tilts and walking cost nothing. Without sensors only the wind counts (fire)
// or nothing (water). The game's carrying uses `fill` at delivery.
import type { Faction } from "./net";

/** Spilling and wind never take a load below this. */
export const FLOOR = 0.3;
/** Tilt from the pick-up hold that is still free (degrees), and where spilling is full. */
const TILT_FREE = 20;
const TILT_FULL = 65;
/** Fill lost per second at full tilt; by shaking at `SHAKE_FULL`. */
const TILT_RATE = 0.12;
const SHAKE_RATE = 0.08;
/** Movement (m/s², gravity removed, smoothed): walking stays below FREE. */
const SHAKE_FREE = 5;
const SHAKE_FULL = 12;
/** Fire: fill lost per second walking straight into wind of `WIND_FULL` m/s. */
const WIND_RATE = 0.0015;
const WIND_FULL = 8;
/** Walking slower than this has no heading (m/s). */
const WALKING = 0.6;

export type Wind = { fromDeg: number; speed: number };

export type FeelState = {
  fill: number;
  /** Degrees from the pick-up hold; m/s² of movement; -1..1 into the wind. */
  tilt: number;
  shake: number;
  headwind: number;
  /** The water surface's angle on screen (degrees), damped like a liquid. */
  surface: number;
  sensors: boolean;
};

/** Ask for motion sensors (iOS needs a tap first); false if refused or absent. */
export async function askMotion(): Promise<boolean> {
  const D = (window as unknown as { DeviceMotionEvent?: { requestPermission?: () => Promise<string> } }).DeviceMotionEvent;
  if (!D) return false;
  if (typeof D.requestPermission === "function") {
    try {
      return (await D.requestPermission()) === "granted";
    } catch {
      return false;
    }
  }
  return true;
}

export class Feel {
  fill = 1;
  private g: [number, number, number] | null = null;
  private g0: [number, number, number] | null = null;
  private g0n = 0;
  private shakeAvg = 0;
  private surface = 0;
  private surfaceV = 0;
  private wind: Wind | null = null;
  private last: { lat: number; lon: number; t: number } | null = null;
  private heading: number | null = null;
  private speed = 0;
  private sensors = false;
  private readonly onMotion = (e: DeviceMotionEvent) => this.motion(e);

  /** `onSpill(lost)`: fill lost in one tick, for a sound or a jolt. */
  constructor(
    readonly kind: Faction,
    private onSpill: (lost: number) => void = () => {},
  ) {}

  start() {
    window.addEventListener("devicemotion", this.onMotion);
  }

  stop() {
    window.removeEventListener("devicemotion", this.onMotion);
  }

  setWind(w: Wind | null) {
    this.wind = w;
  }

  /** A GPS fix: the walking heading comes from the last one. */
  move(lat: number, lon: number, t = Date.now()) {
    const p = this.last;
    this.last = { lat, lon, t };
    if (!p || t - p.t < 1500) {
      if (p) this.last = p;
      return;
    }
    const k = Math.cos((lat * Math.PI) / 180);
    const dx = (lon - p.lon) * 111_320 * k;
    const dy = (lat - p.lat) * 110_540;
    this.speed = Math.hypot(dx, dy) / ((t - p.t) / 1000);
    this.heading = this.speed >= WALKING ? ((Math.atan2(dx, dy) * 180) / Math.PI + 360) % 360 : null;
  }

  /** Walking heading and speed directly (test page, simulation). */
  walk(headingDeg: number | null, speed: number) {
    this.heading = headingDeg;
    this.speed = speed;
  }

  private motion(e: DeviceMotionEvent) {
    const a = e.accelerationIncludingGravity;
    if (a?.x == null || a.y == null || a.z == null) return;
    this.sensors = true;
    const g: [number, number, number] = [a.x, a.y, a.z];
    // Gravity: slow average; movement: what's left.
    this.g = this.g ? [0, 1, 2].map((i) => this.g![i] * 0.9 + g[i] * 0.1) as [number, number, number] : g;
    const lin = e.acceleration?.x != null ? Math.hypot(e.acceleration.x!, e.acceleration.y ?? 0, e.acceleration.z ?? 0) : Math.hypot(g[0] - this.g[0], g[1] - this.g[1], g[2] - this.g[2]);
    this.shakeAvg = this.shakeAvg * 0.95 + lin * 0.05;
    // The hold at pick-up: the first second.
    if (this.g0n < 20) {
      this.g0 = this.g0 ? ([0, 1, 2].map((i) => (this.g0![i] * this.g0n + g[i]) / (this.g0n + 1)) as [number, number, number]) : g;
      this.g0n++;
    }
  }

  private tilt(): number {
    if (!this.g || !this.g0) return 0;
    const [a, b] = [this.g, this.g0];
    const dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    const n = Math.hypot(...a) * Math.hypot(...b);
    return n ? (Math.acos(Math.max(-1, Math.min(1, dot / n))) * 180) / Math.PI : 0;
  }

  private headwind(): number {
    if (!this.wind || this.heading === null || this.speed < WALKING) return 0;
    return Math.cos(((this.heading - this.wind.fromDeg) * Math.PI) / 180);
  }

  /** Advance by `dt` seconds. */
  tick(dt: number): FeelState {
    const tilt = this.tilt();
    const shake = this.shakeAvg;
    const head = this.headwind();
    let loss = 0;
    if (this.kind === "water") {
      const t = Math.max(0, Math.min(1, (tilt - TILT_FREE) / (TILT_FULL - TILT_FREE)));
      const s = Math.max(0, Math.min(1, (shake - SHAKE_FREE) / (SHAKE_FULL - SHAKE_FREE)));
      loss = (TILT_RATE * t * t + SHAKE_RATE * s) * dt;
    } else if (head > 0 && this.wind) {
      loss = WIND_RATE * head * Math.min(1, this.wind.speed / WIND_FULL) * dt;
    }
    const before = this.fill;
    this.fill = Math.max(FLOOR, this.fill - loss);
    if (before - this.fill > 0) this.onSpill(before - this.fill);
    // The surface leans against the side tilt, springy and damped.
    const side = this.g ? (Math.atan2(this.g[0], Math.hypot(this.g[1], this.g[2])) * 180) / Math.PI : 0;
    const target = Math.max(-35, Math.min(35, side)) + Math.sin(Date.now() / 120) * Math.min(8, shake);
    this.surfaceV += (target - this.surface) * 18 * dt - this.surfaceV * 5 * dt;
    this.surface += this.surfaceV * dt;
    return { fill: this.fill, tilt, shake, headwind: head, surface: this.surface, sensors: this.sensors };
  }
}

/** A jug (water) or a lantern (fire) as SVG, drawn from a `FeelState`. */
export function feelView(kind: Faction): { el: SVGSVGElement; draw(s: FeelState): void } {
  const ns = "http://www.w3.org/2000/svg";
  const el = document.createElementNS(ns, "svg");
  el.setAttribute("viewBox", "0 0 120 140");
  if (kind === "water") {
    el.innerHTML = `
      <defs><clipPath id="jug-in"><path d="M30 30 Q24 60 28 110 Q30 126 60 126 Q90 126 92 110 Q96 60 90 30 Z"/></clipPath></defs>
      <g clip-path="url(#jug-in)"><rect id="water" x="-40" y="0" width="200" height="200" fill="#2f9bff" opacity="0.85"/></g>
      <path d="M30 30 Q24 60 28 110 Q30 126 60 126 Q90 126 92 110 Q96 60 90 30 Z M92 45 Q112 50 106 75 Q100 90 92 88" fill="none" stroke="#c9e8ff" stroke-width="4"/>
      <path d="M26 30 L94 30" stroke="#c9e8ff" stroke-width="5" stroke-linecap="round"/>`;
    const water = el.querySelector<SVGRectElement>("#water")!;
    return {
      el,
      draw(s) {
        const y = 126 - (126 - 34) * s.fill;
        water.setAttribute("y", String(y));
        water.setAttribute("transform", `rotate(${-s.surface} 60 ${y})`);
      },
    };
  }
  el.innerHTML = `
    <defs><radialGradient id="ember-g"><stop offset="0" stop-color="#ffe0b3"/><stop offset="0.45" stop-color="#ff7a1a"/><stop offset="1" stop-color="#ff7a1a" stop-opacity="0"/></radialGradient></defs>
    <circle id="glow" cx="60" cy="85" r="40" fill="url(#ember-g)"/>
    <path d="M38 40 L82 40 L88 120 L32 120 Z" fill="none" stroke="#ffe0b3" stroke-width="4" stroke-linejoin="round"/>
    <path d="M48 40 Q60 14 72 40" fill="none" stroke="#ffe0b3" stroke-width="4"/>
    <circle id="coal" cx="60" cy="100" r="10" fill="#ff7a1a"/>`;
  const glow = el.querySelector<SVGCircleElement>("#glow")!;
  const coal = el.querySelector<SVGCircleElement>("#coal")!;
  return {
    el,
    draw(s) {
      const flicker = s.headwind > 0 ? Math.random() * 0.25 * s.headwind : 0;
      glow.setAttribute("r", String(18 + 30 * s.fill * (1 - flicker)));
      glow.setAttribute("opacity", String(0.4 + 0.6 * s.fill));
      coal.setAttribute("r", String(5 + 7 * s.fill));
    },
  };
}
