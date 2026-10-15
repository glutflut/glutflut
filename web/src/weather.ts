// Today's weather as a rule (server: weather.rs): a symbol in the header on
// days the weather helps a side; a tap explains how. The same for the tide.
import { lang, t, type Key } from "./i18n";
import type { ServerMsg } from "./net";
import { $ } from "./page";

export type WeatherMsg = Extract<ServerMsg, { t: "weather" }>;
export type TideMsg = Extract<ServerMsg, { t: "tide" }>;

const ICON = { rain: "☔", heat: "☀", frost: "❄" };

export function setupWeather(notice: (cls: string, kicker: string, name: string, line: string) => void) {
  const btn = $("wx") as HTMLButtonElement;
  let last: WeatherMsg | null = null;
  btn.onclick = () => {
    if (!last?.effect || !last.side) return;
    const level = t(`wx.level.${last.level < 0.4 ? 1 : last.level < 0.75 ? 2 : 3}` as Key);
    const day = t("wx.day", { min: last.tmin, max: last.tmax, mm: last.precip_mm });
    notice(last.side, `${t("wx.now")} · ${level}`, t(`wx.effect.${last.effect}.title` as Key), `${t(`wx.effect.${last.effect}.text` as Key)} ${day}`);
  };
  // The tide at the waterfront (server: tide.rs), in regions with a gauge.
  const tideBtn = $("tide") as HTMLButtonElement;
  let tide: TideMsg | null = null;
  // The next forecast high or low water, whichever comes first (NOAA regions).
  const next = (m: TideMsg) => {
    const now = Date.now() / 1000;
    let best: [Key, number] | null = null;
    for (const [k, at] of [["tide.next_high", m.next_high], ["tide.next_low", m.next_low]] as [Key, number | null][]) {
      if (at != null && at > now && (!best || at < best[1])) best = [k, at];
    }
    if (!best) return "";
    const time = new Date(best[1] * 1000).toLocaleTimeString(lang, { hour: "2-digit", minute: "2-digit" });
    // Estimated from the last tides (no forecast at the gauge): "ca."
    if (m.hilo_estimated) return ` · ${t(best[0], { time: t("tide.about", { time }) })}`;
    return ` · ${t(best[0], { time })}`;
  };
  const stage = (m: TideMsg) => (!m.on ? "off" : m.phase > 0.33 ? "high" : m.phase < -0.33 ? "low" : "mid");
  tideBtn.onclick = () => {
    if (!tide) return;
    const s = stage(tide);
    const side = s === "high" ? "water" : s === "low" ? "fire" : "";
    const kicker = tide.on
      ? `${t("tide.now")} · ${t(tide.rising ? "tide.rising" : "tide.falling")} · ${t("tide.level", { m: tide.level_m.toFixed(1) })}${next(tide)}`
      : t("tide.now");
    notice(side, kicker, t(`tide.${s}.title` as Key), `${t(`tide.${s}.text` as Key)} ${t("tide.safety")}`);
  };
  return {
    onWeather(m: WeatherMsg) {
      last = m;
      btn.hidden = !m.effect;
      btn.textContent = m.effect ? ICON[m.effect] : "";
      btn.className = m.side ?? "";
    },
    onTide(m: TideMsg) {
      tide = m;
      tideBtn.hidden = false;
      const s = stage(m);
      tideBtn.className = s === "high" ? "water" : s === "low" ? "fire" : s === "off" ? "off" : "";
    },
  };
}
