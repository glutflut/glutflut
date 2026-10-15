// Optional profile (age band, lives in Konstanz): kept in this browser, sent
// after each connect; the server only counts players per day by it. The
// chips render before the game connects, so the sender is registered later.
import { t } from "./i18n";
import { confirmAdult, isAdult } from "./consent";
import type { Age } from "./net";
import { KEYS, loadJson, saveJson } from "./store";

export type Profile = { age: Age | null; local: boolean | null };
let profile: Profile = { age: null, local: null, ...loadJson<Partial<Profile>>(KEYS.profile) };
let send = (_: Profile) => {};
/** Who gets the profile when it changes (and on `sendProfile`). */
export function onProfile(f: (p: Profile) => void) {
  send = f;
}
// Only from 16 (consent, Art. 8 GDPR).
export const sendProfile = () => send(isAdult() ? profile : { age: null, local: null });

function save() {
  saveJson(KEYS.profile, profile);
}

/** No longer 16+: the answers go. */
export function clearProfile() {
  profile = { age: null, local: null };
  save();
  sendProfile();
  renderProfiles();
}

export function renderProfiles() {
  const rows: [string, [string, Partial<Profile>][]][] = [
    [t("profile.age"), [[t("profile.age.u30"), { age: "u30" }], [t("profile.age.30_59"), { age: "30_59" }], [t("profile.age.60p"), { age: "60p" }]]],
    [t("profile.local"), [[t("profile.yes"), { local: true }], [t("profile.no"), { local: false }]]],
  ];
  for (const host of document.querySelectorAll<HTMLElement>(".profile")) {
    host.replaceChildren(
      Object.assign(document.createElement("div"), {
        className: "profile-note",
        textContent: t("profile.note"),
      }),
    );
    for (const [title, opts] of rows) {
      const row = Object.assign(document.createElement("div"), { className: "chips" });
      row.append(Object.assign(document.createElement("span"), { className: "profile-q", textContent: title }));
      for (const [label, val] of opts) {
        const [k, v] = Object.entries(val)[0] as [keyof Profile, never];
        const b = Object.assign(document.createElement("button"), { textContent: label });
        b.classList.toggle("on", profile[k] === v);
        b.setAttribute("aria-pressed", String(profile[k] === v));
        b.onclick = async () => {
          if (!(await confirmAdult())) return;
          profile = { ...profile, [k]: profile[k] === v ? null : v }; // tap again: no answer
          save();
          sendProfile();
          renderProfiles();
        };
        row.append(b);
      }
      host.append(row);
    }
  }
}
