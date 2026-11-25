// The city as an instrument (stage 1): every junction has its note, from its
// height (one scale step per metre, wrapping), so a walk plays a melody and
// the same way always the same one. Claiming the last corner of a block your
// side holds all round plays the block's corners as a chord.
import type { Faction } from "./net";
import { owner } from "./geo";
import { NOTE_STEPS } from "./sound";
import type { TileLayer } from "./tiles";

/** Metres of height per scale step. */
const STEP_M = 1;
/** At most this many notes in a chord. */
const CHORD = 4;

export function stepOf(elev: number): number {
  return ((Math.floor(elev / STEP_M) % NOTE_STEPS) + NOTE_STEPS) % NOTE_STEPS;
}

/** The notes for a claim of `node` by `f`: one, or a block's chord. */
export function claimNotes(tiles: TileLayer, state: Map<number, [number, number]>, node: number, f: Faction): number[] {
  const elev = tiles.nodeElev.get(node);
  if (elev === undefined) return [];
  const mine = f === "fire" ? 1 : 2;
  const held = (n: number) => {
    if (n === node) return true;
    const [a, b] = state.get(n) ?? [0, 0];
    return owner(a, b) === mine;
  };
  for (const b of tiles.blocksOf.get(node) ?? []) {
    const corners = tiles.blockNodes.get(b) ?? [];
    if (corners.length < 3 || !corners.every(held)) continue;
    const steps = [...new Set(corners.map((n) => stepOf(tiles.nodeElev.get(n) ?? elev)))].sort((x, y) => x - y);
    if (steps.length < 2) steps.push(steps[0] + 4); // a fifth over one note
    // Spread over the block: lowest, highest and between.
    const pick = steps.length <= CHORD ? steps : [0, 1, 2, 3].map((i) => steps[Math.round((i * (steps.length - 1)) / (CHORD - 1))]);
    return pick;
  }
  return [stepOf(elev)];
}
