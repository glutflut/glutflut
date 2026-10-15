// Sounds, all synthesized with Web Audio (no files, nothing to license, every
// claim a little different): fire crackles, water bubbles, putting a spark
// out hisses like steam. Plus a quiet ambient layer from the ground under the
// player: embers on fire ground, trickling on water ground, both at the front.
// On by default; the speaker button mutes everything, the settings switch
// the ambient layer. Browsers start audio only after a first tap.
import type { Faction } from "./net";
import { KEYS, load, save, type StoreKey } from "./store";

const KEY_ON = KEYS.sound;
const KEY_AMBIENT = KEYS.soundAmbient;
/** Master volume of events and of the ambient layer at full strength. */
const EVENT_GAIN = 0.55;
const AMBIENT_GAIN = 0.35;

function stored(key: StoreKey): boolean {
  return load(key) !== "0";
}

function store(key: StoreKey, on: boolean) {
  save(key, on ? "1" : "0");
}

const rnd = (a: number, b: number) => a + Math.random() * (b - a);

export class Sound {
  on = stored(KEY_ON);
  ambientOn = stored(KEY_AMBIENT);
  private ctx: AudioContext | null = null;
  private out!: GainNode;
  private noise!: AudioBuffer;
  private amb: { fire: GainNode; water: GainNode; level: [number, number]; timer: number } | null = null;

  constructor() {
    // Audio may only start from a user gesture: the first tap anywhere.
    const unlock = () => {
      this.start();
      if (this.ctx?.state === "running") {
        window.removeEventListener("pointerdown", unlock);
        window.removeEventListener("keydown", unlock);
      }
    };
    window.addEventListener("pointerdown", unlock);
    window.addEventListener("keydown", unlock);
    document.addEventListener("visibilitychange", () => {
      if (!this.ctx) return;
      if (document.hidden) void this.ctx.suspend();
      else if (this.on) void this.ctx.resume();
    });
  }

  private start() {
    if (!this.on) return;
    if (!this.ctx) {
      const Ctx = window.AudioContext ?? (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
      if (!Ctx) return;
      this.ctx = new Ctx();
      const comp = this.ctx.createDynamicsCompressor();
      comp.connect(this.ctx.destination);
      this.out = this.ctx.createGain();
      this.out.gain.value = 1;
      this.out.connect(comp);
      const len = this.ctx.sampleRate * 2;
      this.noise = this.ctx.createBuffer(1, len, this.ctx.sampleRate);
      const d = this.noise.getChannelData(0);
      for (let i = 0; i < len; i++) d[i] = Math.random() * 2 - 1;
      this.startAmbient();
    }
    if (this.ctx.state !== "running") void this.ctx.resume();
  }

  setOn(on: boolean) {
    this.on = on;
    store(KEY_ON, on);
    if (on) this.start();
    else void this.ctx?.suspend();
  }

  setAmbient(on: boolean) {
    this.ambientOn = on;
    store(KEY_AMBIENT, on);
    this.ground(...(this.amb?.level ?? [0, 0]));
  }

  private ready(): AudioContext | null {
    return this.on && this.ctx?.state === "running" ? this.ctx : null;
  }

  // Building blocks -----------------------------------------------------------

  /** A burst of filtered noise. */
  private burst(t: number, dur: number, gain: number, type: BiquadFilterType, freq: number, q = 1, to?: AudioNode) {
    const c = this.ctx!;
    const src = c.createBufferSource();
    src.buffer = this.noise;
    const f = c.createBiquadFilter();
    f.type = type;
    f.frequency.value = freq;
    f.Q.value = q;
    const g = c.createGain();
    g.gain.setValueAtTime(0, t);
    g.gain.linearRampToValueAtTime(gain, t + Math.min(0.004, dur / 4));
    g.gain.exponentialRampToValueAtTime(0.0001, t + dur);
    src.connect(f).connect(g).connect(to ?? this.out);
    src.start(t, Math.random() * 1.5, dur + 0.05);
    return { f, g };
  }

  /** A tone with an envelope; `to` glides the pitch. */
  private tone(t: number, freq: number, dur: number, gain: number, type: OscillatorType = "sine", to?: number, out?: AudioNode) {
    const c = this.ctx!;
    const o = c.createOscillator();
    o.type = type;
    o.frequency.setValueAtTime(freq, t);
    if (to) o.frequency.exponentialRampToValueAtTime(to, t + dur);
    const g = c.createGain();
    g.gain.setValueAtTime(0, t);
    g.gain.linearRampToValueAtTime(gain, t + Math.min(0.01, dur / 5));
    g.gain.exponentialRampToValueAtTime(0.0001, t + dur);
    o.connect(g).connect(out ?? this.out);
    o.start(t);
    o.stop(t + dur + 0.05);
  }

  /** Crackling: `n` sharp clicks over `dur` seconds. */
  private crackle(t: number, n: number, dur: number, gain: number, out?: AudioNode) {
    for (let i = 0; i < n; i++) {
      const at = t + Math.pow(Math.random(), 1.4) * dur;
      this.burst(at, rnd(0.006, 0.03), gain * rnd(0.3, 1), "bandpass", rnd(1500, 5000), rnd(0.7, 2), out);
    }
  }

  /** One bubble: a short rising (or falling) tone. */
  private bubble(t: number, f0: number, gain: number, down = false, out?: AudioNode) {
    const dur = rnd(0.05, 0.1);
    this.tone(t, f0, dur, gain, "sine", down ? f0 * 0.5 : f0 * rnd(2, 3), out);
  }

  // Events ----------------------------------------------------------------------

  /** A claim of `f` took. */
  claim(f: Faction) {
    const c = this.ready();
    if (!c) return;
    const t = c.currentTime + 0.01;
    if (f === "fire") {
      this.burst(t, 0.35, 0.5 * EVENT_GAIN, "lowpass", 500);
      this.crackle(t, 14, 0.6, 0.5 * EVENT_GAIN);
    } else {
      const n = 2 + Math.floor(Math.random() * 2);
      for (let i = 0; i < n; i++) this.bubble(t + i * rnd(0.06, 0.12), rnd(280, 650), 0.45 * EVENT_GAIN);
    }
  }

  /** A claim builds up for `secs` (on enemy ground): denser and denser. */
  building(f: Faction, secs: number) {
    const c = this.ready();
    if (!c) return;
    const t = c.currentTime + 0.05;
    const dur = Math.min(secs, 8);
    const n = Math.round(dur * 6);
    for (let i = 0; i < n; i++) {
      const at = t + dur * Math.sqrt(i / n);
      if (f === "fire") this.burst(at, rnd(0.006, 0.02), 0.25 * EVENT_GAIN, "bandpass", rnd(1800, 4500), 1);
      else this.bubble(at, rnd(500, 900), 0.18 * EVENT_GAIN);
    }
  }

  /** Putting out a spark: steam. */
  steam() {
    const c = this.ready();
    if (!c) return;
    const t = c.currentTime + 0.01;
    const { f } = this.burst(t, 1.3, 0.45 * EVENT_GAIN, "bandpass", 6000, 0.6);
    f.frequency.exponentialRampToValueAtTime(2500, t + 1.2);
    this.crackle(t, 6, 0.3, 0.3 * EVENT_GAIN);
  }

  /** Glut / Eis placed. */
  keep(f: Faction) {
    const c = this.ready();
    if (!c) return;
    const t = c.currentTime + 0.01;
    if (f === "fire") {
      this.tone(t, 90, 0.7, 0.5 * EVENT_GAIN, "sine", 55);
      this.burst(t, 0.6, 0.4 * EVENT_GAIN, "lowpass", 700);
      this.crackle(t + 0.1, 18, 1.0, 0.4 * EVENT_GAIN);
    } else {
      for (const [hz, g] of [[1318, 0.25], [1975, 0.18], [2637, 0.12]] as const) {
        this.tone(t, hz * rnd(0.995, 1.005), 1.6, g * EVENT_GAIN, "sine");
      }
    }
  }

  /** Own permanent source topped up. */
  topup(f: Faction) {
    const c = this.ready();
    if (!c) return;
    const t = c.currentTime + 0.01;
    if (f === "fire") {
      const { f: flt } = this.burst(t, 0.5, 0.35 * EVENT_GAIN, "bandpass", 500, 0.8);
      flt.frequency.exponentialRampToValueAtTime(1800, t + 0.4);
      this.crackle(t + 0.15, 10, 0.5, 0.4 * EVENT_GAIN);
    } else {
      this.tone(t, 2400, 0.3, 0.2 * EVENT_GAIN);
      this.tone(t + 0.07, 3600, 0.25, 0.15 * EVENT_GAIN);
    }
  }

  /** An own source was taken over (or a permanent one fell). */
  lost(f: Faction) {
    const c = this.ready();
    if (!c) return;
    const t = c.currentTime + 0.01;
    if (f === "fire") {
      const { f: flt } = this.burst(t, 0.9, 0.3 * EVENT_GAIN, "highpass", 3000);
      flt.frequency.exponentialRampToValueAtTime(800, t + 0.8);
      for (let i = 0; i < 5; i++) this.burst(t + i * 0.12, 0.02, (0.3 - i * 0.05) * EVENT_GAIN, "bandpass", 2500, 1);
    } else {
      for (let i = 0; i < 4; i++) this.bubble(t + i * 0.13, 600 - i * 90, 0.3 * EVENT_GAIN, true);
    }
  }

  /** A spark of element `f` appeared nearby (in your ground): a distant warning. */
  spark(f: Faction) {
    const c = this.ready();
    if (!c) return;
    const t = c.currentTime + 0.01;
    if (f === "fire") {
      const { g } = this.burst(t, 1.4, 0.0001, "bandpass", 1500, 0.7);
      g.gain.cancelScheduledValues(t);
      g.gain.setValueAtTime(0.0001, t);
      g.gain.linearRampToValueAtTime(0.25 * EVENT_GAIN, t + 0.6);
      g.gain.exponentialRampToValueAtTime(0.0001, t + 1.4);
      this.crackle(t + 0.4, 6, 0.8, 0.15 * EVENT_GAIN);
    } else {
      for (let i = 0; i < 8; i++) this.bubble(t + i * rnd(0.07, 0.14), rnd(900, 1500), 0.12 * EVENT_GAIN);
    }
  }

  /** A find uncovered. */
  chime() {
    const c = this.ready();
    if (!c) return;
    const t = c.currentTime + 0.01;
    this.tone(t, 1046, 1.2, 0.2 * EVENT_GAIN, "sine");
    this.tone(t + 0.09, 1568, 1.1, 0.16 * EVENT_GAIN, "sine");
  }

  /** A tunnel opened. */
  rumble() {
    const c = this.ready();
    if (!c) return;
    const t = c.currentTime + 0.01;
    this.burst(t, 1.6, 0.6 * EVENT_GAIN, "lowpass", 140);
    this.tone(t, 48, 1.5, 0.45 * EVENT_GAIN, "sine", 38);
  }

  /** Level up, badge. */
  motif() {
    const c = this.ready();
    if (!c) return;
    const t = c.currentTime + 0.01;
    [523, 659, 784].forEach((hz, i) => this.tone(t + i * 0.12, hz, 0.35, 0.22 * EVENT_GAIN, "triangle"));
  }

  /** The next claim is possible again. */
  tick() {
    const c = this.ready();
    if (!c) return;
    this.tone(c.currentTime + 0.01, 1800, 0.03, 0.06 * EVENT_GAIN);
  }

  // Ambient ---------------------------------------------------------------------

  private startAmbient() {
    const c = this.ctx!;
    const bed = (type: BiquadFilterType, freq: number) => {
      const src = c.createBufferSource();
      src.buffer = this.noise;
      src.loop = true;
      const f = c.createBiquadFilter();
      f.type = type;
      f.frequency.value = freq;
      f.Q.value = 0.7;
      const g = c.createGain();
      g.gain.value = 0;
      src.connect(f).connect(g).connect(this.out);
      src.start();
      return g;
    };
    const fire = bed("lowpass", 700);
    const water = bed("bandpass", 450);
    // Crackles and trickles on top of the beds, as often as the ground is strong.
    const timer = window.setInterval(() => {
      const a = this.amb;
      const c = this.ready();
      if (!a || !c || !this.ambientOn) return;
      const [fl, wl] = a.level;
      const t = c.currentTime + 0.02;
      if (Math.random() < fl * 0.45) this.burst(t, rnd(0.005, 0.02), rnd(0.03, 0.09) * fl * AMBIENT_GAIN * 2, "bandpass", rnd(1500, 4500), 1);
      if (Math.random() < wl * 0.3) this.bubble(t, rnd(700, 1400), rnd(0.02, 0.05) * wl * AMBIENT_GAIN * 2);
    }, 110);
    this.amb = { fire, water, level: this.amb?.level ?? [0, 0], timer };
    this.ground(...this.amb.level);
  }

  /** Strength of fire and water under the player (0..1). */
  ground(fire: number, water: number) {
    const lv: [number, number] = [Math.min(1, fire), Math.min(1, water)];
    if (!this.amb) {
      this.amb = { fire: null!, water: null!, level: lv, timer: 0 };
      return;
    }
    this.amb.level = lv;
    if (!this.ctx || !this.amb.fire) return;
    const on = this.ambientOn ? 1 : 0;
    const t = this.ctx.currentTime;
    this.amb.fire.gain.setTargetAtTime(lv[0] * 0.05 * AMBIENT_GAIN * on, t, 0.8);
    this.amb.water.gain.setTargetAtTime(lv[1] * 0.06 * AMBIENT_GAIN * on, t, 0.8);
  }
}
