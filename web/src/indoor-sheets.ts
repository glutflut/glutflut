// Ops page (indoor.html): the QR sheets of one indoor region, also opened from
// another shard's admin page (login there; the token of this site if stored).
import { authHeaders } from "./opsauth";
import { renderIndoor, type IndoorReply } from "./indoor-render";

const TOKEN_KEY = "elements.ops"; // shared with the admin page, dashboard, editor
const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const stored = () => {
  try {
    return localStorage.getItem(TOKEN_KEY) ?? "";
  } catch {
    return "";
  }
};

$<HTMLInputElement>("region").value = new URLSearchParams(location.search).get("region") ?? "";
$<HTMLInputElement>("token").value = stored();
$("print").onclick = () => print();

async function load() {
  $("msg").textContent = "";
  const region = $<HTMLInputElement>("region").value.trim();
  if (!region) return;
  const res = await fetch(`/api/ops/indoor?region=${encodeURIComponent(region)}`, { headers: authHeaders($<HTMLInputElement>("token").value.trim()) }).catch(() => null);
  if (!res?.ok) {
    $("token-field").hidden = false;
    return void ($("msg").textContent = `Fehler ${res?.status ?? "Netz"}: ${(await res?.text().catch(() => "")) ?? ""}`);
  }
  const out = renderIndoor((await res.json()) as IndoorReply, $("sheets"), $("screen-list"));
  $("screens").hidden = !out.screens;
  $("msg").textContent = out.text;
}

$("form").onsubmit = (e) => {
  e.preventDefault();
  void load();
};
// Opened with ?region= (admin page): straight away, with the login or stored token.
if ($<HTMLInputElement>("region").value) {
  $("token-field").hidden = true;
  void load();
}
