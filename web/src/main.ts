// The game page: loads the city, builds the map, wires the modules together
// and routes each server message (net.ts `ServerMsg`) to the one it concerns.
import * as maplibregl from "maplibre-gl";
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url";
import "./version";
import { applyStatic, langChosen, langSwitch, lang, loadCity, t, type Key } from "./i18n";

// Bundled worker; MapLibre's own lookup breaks once everything is one chunk.
maplibregl.setWorkerUrl(workerUrl);
import { Net, clearToken, saveToken, type Faction, type ServerMsg } from "./net";
import { TileLayer, loadMeta, type Meta } from "./tiles";
import { FlowLayer } from "./flow";
import { BurstLayer, fx, riseWait, spotBurst, type BurstKind } from "./burst";
import { chooseRegion, knownRegions, openCities, setupCities } from "./outside";
import { setupWeather } from "./weather";
import { setupNews } from "./news";
import { onNotices } from "./notices";
import { markStorySeen, showStory, storySeen } from "./story";
import { checkInstall } from "./install";
import { setupRadar } from "./radar";
import { setupFinds } from "./finds";
import { setupPerm, type PermMsg } from "./perm";
import { setupUpdate } from "./update";
import { setupNodeCard } from "./nodecard";
import { setupFeedback } from "./feedback";
import { Sound } from "./sound";
import { isCounted } from "./consent";
import { presenceAnswer, takePresenceCode } from "./presence";
import { setupCampaign } from "./campaign";
import { setupTunnel } from "./tunnel";
import { setupTop } from "./top";
import { setupPages } from "./pages";
import { autoGfx, calm, gfxChoice, setGfx, watchFrames, type GfxChoice } from "./gfx";
import { createMap } from "./layers";
import { setupWorld } from "./world";
import { setupOverview } from "./overview";
import { setupSources, type SourceItems } from "./sources";
import { setupProgress } from "./progress";
import { setupClaim } from "./claim";
import { setupHotspots } from "./hotspots";
import { setupPlayer } from "./player";
import { setupCheer } from "./cheer";
import { setupFloaters } from "./floaters";
import { setupGps } from "./gps";
import { setupArBridge } from "./arbridge";
import { setupVideo } from "./video";
import { setupBoard } from "./board";
import { setupInvite } from "./invite";
import { addGift } from "./gift";
import { setupNavMore } from "./navmore";
import { setupRecovery } from "./recovery";
import { onProfile, renderProfiles, sendProfile } from "./profile";
import { CLAIM_RADIUS_M, COLORS, distM, owner } from "./geo";
import { $, params, toast } from "./page";
import { KEYS, blocked, load, save } from "./store";
import { joinGate, kiosk, tryJoin, watching } from "./watch";
import { freshSources, setupLive, startKiosk } from "./live";
import { setupTimelapse } from "./timelapse";
import { lookFromHash, setupGlobe } from "./globe";
import { openRegion } from "./region";

// No token yet when the page loaded: the first `welcome` as a player makes
// this browser a new player (the side reveal card below).
const newcomer = !load(KEYS.token);

// Graphics level per browser (see gfx.ts): game view unless "map"; tilt,
// 3D buildings and moving elements only at "full".
const choice = gfxChoice();
const gfx = choice === "auto" ? autoGfx() : choice;
const gameView = gfx !== "map";
const animOn = gfx === "full" && !calm;

takePresenceCode();
// From the weather globe: dark until the map stands (style.css `arrive`).
if (/(?:^#|&)look=/.test(location.hash)) document.documentElement.classList.add("arrive");

type Handlers = { [K in ServerMsg["t"]]: (m: Extract<ServerMsg, { t: K }>) => void };

async function main() {
  const meta: Meta = await loadMeta(() => ($("me-label").textContent = t("region.waking")));
  const map = await createMap(meta, gameView, gfx === "full");

  let flow: FlowLayer | null = null;
  const world = setupWorld(map, () => flow, meta.area ?? null);
  const { state, edgeState } = world;
  let lastView: [number, number][] = [];
  const tiles = new TileLayer(
    map,
    meta,
    () => {},
    (gone) => {
      world.drop(gone);
      sources.forget(gone);
    },
    (view) => {
      lastView = view;
      net.send({ t: "view", tiles: view });
    },
  );
  tiles.onBlocks = (ids) => world.blocks(ids, tiles);
  tiles.blockProps = world.blockProps;
  const nodePos = tiles.nodePos;
  flow = animOn ? new FlowLayer(tiles) : null;
  if (flow) {
    map.addLayer(flow, "nodes");
    // Automatic level: a device that can't keep up gets the still view.
    if (choice === "auto")
      watchFrames(() => {
        map.removeLayer("flow");
        flow = null;
        map.easeTo({ pitch: 0 });
        toast(t("gfx.slow"));
      });
  }

  // The moment of a claim or a new permanent source (burst.ts); off for "reduce motion".
  const burstLayer = gameView && !calm ? new BurstLayer(tiles) : null;
  if (burstLayer) map.addLayer(burstLayer, "nodes");
  const burst = (node: number, side: Faction, kind: BurstKind): boolean => {
    const ll = nodePos.get(node);
    if (!burstLayer || !ll) return false;
    // Someone else's claim only where it can be seen.
    if (kind === "other" && !map.getBounds().contains(ll)) return false;
    if (!burstLayer.fire(node, side, kind)) return false;
    spotBurst(map, ll, side, kind);
    // A slight bow of the camera for a new permanent source.
    if (kind === "keep" && gfx === "full") {
      const pitch = map.getPitch();
      map.easeTo({ pitch: Math.min(55, pitch + 12), duration: 900 });
      setTimeout(() => map.easeTo({ pitch, duration: 1400 }), 2200);
    }
    return true;
  };
  // A claim far away (through a link): once the camera is there and the
  // streets around it are loaded (tries for 3 s).
  const burstThere = (node: number, side: Faction, kind: BurstKind, tries = 8) => {
    const ll = nodePos.get(node);
    if (ll && map.getBounds().contains(ll)) return void burst(node, side, kind);
    if (tries > 0) setTimeout(() => burstThere(node, side, kind, tries - 1), 400);
  };
  // While playing, other players' claims in view too; own ones had theirs (claim handler).
  const fresh = freshSources();
  const ownBurst = new Map<number, number>();
  const othersBurst = (items: Parameters<typeof fresh>[0]) => {
    if (live || !burstLayer) return;
    const now = Date.now();
    for (const f of fresh(items)) if (now - (ownBurst.get(f.node) ?? 0) > 15_000) burst(f.node, f.side, "other");
  };
  // Try the bursts on a test server (`sim` in welcome): ?fx shows buttons that
  // fire at the map centre; ?fx=4 in slow motion.
  const fxParam = new URLSearchParams(location.search).get("fx");
  let fxShown = false;
  const fxOn = (sim: boolean) => {
    if (fxParam === null || !sim || fxShown) return;
    fxShown = true;
    fx.slow = Math.max(1, Number(fxParam) || 1);
    fxDemo(map, nodePos, burst);
  };

  const overview = setupOverview(map, tiles, (on) => net.send({ t: "overview", on }));
  const sources = setupSources(map);
  const onMove = () => tiles.refresh().then(overview.show);
  map.on("moveend", () => {
    onMove();
    sources.draw();
  });
  onMove();

  let faction: Faction | null = null;

  // Side colours everywhere; on test servers also the switch in the settings.
  const sideBtn = $("side-switch") as HTMLButtonElement;
  function applyFaction(f: Faction) {
    faction = f;
    const color = COLORS[f];
    $("me-dot").style.background = color;
    $("me-label").textContent = t(f === "fire" ? "you.fire" : "you.water");
    (player.marker.getElement() as HTMLElement).style.background = color;
    (player.marker.getElement() as HTMLElement).style.setProperty("--me", color);
    $("claim").className = f;
    document.body.dataset.side = f;
    map.setPaintProperty("range", "fill-color", color);
    map.setPaintProperty("range-line", "line-color", color);
    const other: Faction = f === "fire" ? "water" : "fire";
    sideBtn.textContent = t("side.switch", { side: t(`side.${other}`) });
    sideBtn.onclick = () => net.send({ t: "side", faction: other });
  }

  const sound = new Sound();
  setupNavMore(map);
  const soundBtn = $("nav-sound");
  const drawSoundBtn = () => soundBtn.classList.toggle("off", !sound.on);
  drawSoundBtn();
  soundBtn.onclick = () => {
    sound.setOn(!sound.on);
    drawSoundBtn();
  };
  const ambientBox = $("ambient-on") as HTMLInputElement;
  ambientBox.checked = sound.ambientOn;
  ambientBox.onchange = () => sound.setAmbient(ambientBox.checked);
  // Ambient layer: fire and water of the junction under the player.
  setInterval(() => {
    const me = player.me();
    if (!me) return;
    let best: [number, number] | null = null;
    for (const [id, c] of nodePos) {
      const d = distM(me, c);
      if (d <= 60 && (!best || d < best[1])) best = [id, d];
    }
    const [f, w] = best ? (state.get(best[0]) ?? [0, 0]) : [0, 0];
    sound.ground(f, w);
  }, 1000);

  // Messages arrive only after main() has set everything up, so `on` (below)
  // is complete by then.
  const net = new Net(
    (m: ServerMsg) => (on[m.t] as (m: ServerMsg) => void)(m),
    (connected) => {
      if (!connected)
        $("me-label").textContent = FREE_WIFI.has(meta.region.slug)
          ? `${t("connection.lost")} ${t("connection.wifi")}`
          : t("connection.lost");
    },
  );
  onProfile((p) => net.send({ t: "profile", ...p }));

  const board = setupBoard(net);
  const perm = setupPerm(
    net,
    toast,
    distM,
    CLAIM_RADIUS_M,
    (node) => {
      const ll = sources.all().find((x) => x.node === node)?.ll ?? nodePos.get(node);
      if (!ll) return;
      player.setFollow(false);
      map.flyTo({ center: ll, zoom: Math.max(map.getZoom(), 17) });
    },
    () => progress.mine()?.level ?? 1,
    () => progress.mine()?.badges ?? [],
    map,
  );
  setupFeedback(toast);
  const news = setupNews((k) => t(k));
  const card = setupNodeCard(map, net, (node) => claim.send(node), () => faction, () => perm.nodes(), owner, () => claim.wait(), () => player.me(), perm.linkControl);
  // The junction nearest a tap, within 28 px (the dots are small).
  const pickNode = (p: maplibregl.Point): number | null => {
    let best: [number, number] | null = null;
    for (const [id, c] of nodePos) {
      const q = map.project(c);
      const d = Math.hypot(q.x - p.x, q.y - p.y);
      if (d <= 28 && (!best || d < best[1])) best = [id, d];
    }
    return best?.[0] ?? null;
  };
  const tapCard = (p: maplibregl.Point) => {
    const id = pickNode(p);
    if (id === null) return card.close();
    card.open(id, nodePos.get(id)!);
  };
  const update = setupUpdate(() => ["sheet", "tunnel", "find-card", "node-card", "story"].some((id) => !$(id).hidden));
  const finds = setupFinds(map, net, toast);
  void setupRadar(map, toast);
  setupTop((k) => t(k));
  setupPages();
  const campaign = setupCampaign(map, finds.notice);
  const weather = setupWeather(finds.notice);
  // The tunnel shows its far end, then always back to the player (as a claim
  // through a link does, `look` below).
  const tunnel = setupTunnel(
    map,
    () => player.me(),
    (node) => claim.send(node),
    (on) => {
      if (on) player.setFollow(false);
      else player.home();
    },
    toast,
  );
  const recovery = setupRecovery(net);
  const invite = setupInvite();
  const progress = setupProgress(net, sound, () => faction, () => player.drawRange());
  const claim = setupClaim({
    net,
    sound,
    nodePos,
    state,
    sources: sources.all,
    faction: () => faction,
    me: () => player.me(),
    claimRadius: progress.claimRadius,
    supply: progress.supply,
    reinforce: progress.reinforce,
    perm,
    links: perm.links,
    // A claim through a link: the camera goes there for a moment, then back.
    look: (ll) => {
      player.setFollow(false);
      map.flyTo({ center: ll, zoom: Math.max(map.getZoom(), 16.5), duration: 1500, essential: true });
      setTimeout(() => player.home(), 4000);
    },
    tunnel,
    card,
    claimed: () => invite.claimed(),
  });
  const lapse = setupTimelapse(map, () => meta.region.slug);
  // Far out the globe, for players too; another city opens for watching there.
  // From the globe: players back to where they stand, watchers to the city.
  setupGlobe(map, knownRegions, meta.region.slug, (r) => void openRegion(r, true), () => !document.body.classList.contains("watching") && player.home());
  // Back from the weather globe (`#look=`): start up there and fly in.
  const look = lookFromHash();
  if (look) {
    map.jumpTo(look);
    map.once("idle", () =>
      map.flyTo({ center: meta.hotspot ?? meta.region.centroid, zoom: 15, duration: 6000, essential: true }),
    );
  }
  const root = document.documentElement;
  if (root.classList.contains("arrive")) {
    let lifted = false;
    const lift = () => {
      if (lifted) return;
      lifted = true;
      root.classList.add("arrived");
      setTimeout(() => root.classList.remove("arrive", "arrived"), 1000);
    };
    map.once("idle", lift);
    setTimeout(lift, 2500);
  }
  const hotspots = setupHotspots(map, () => player.me(), (on) => player.setFollow(on));
  const player = setupPlayer({
    map,
    meta,
    net,
    tiles,
    claimRadius: progress.claimRadius,
    moved: () => {
      tunnel.moved();
      claim.update();
      card.refresh();
      hotspots.hint();
    },
  });
  const gps = setupGps(map, player.setMe, tapCard);
  // Recordings only (video.ts): staged claims keep their rings through the server's lists.
  const staged: SourceItems = [];
  setupVideo({
    map,
    me: () => player.me(),
    faction: () => faction,
    setMe: (ll) => player.setMe(ll),
    hotspots: hotspots.at,
    lastClaim: claim.lastClaim,
    state,
    nodePos,
    burst,
    stage: (node, side) => {
      const ll = nodePos.get(node);
      if (!ll) return;
      const item: SourceItems[number] = [node, side, 540, 540, false, ll[0], ll[1], 0, 0, "", false, null, false, false];
      staged.push(item);
      sources.apply([item], [], perm.nodes());
    },
  });
  setupArBridge({ me: () => player.me(), tiles, edgeState, sources: sources.all, flow: () => flow });
  const floaters = setupFloaters(map, () => player.me(), () => faction);
  const cheer = setupCheer(map, nodePos, (m) => net.send(m), !kiosk);

  const onKeep = (m: PermMsg) => {
    if (faction && m.t === "keep" && m.ok) m.topup ? sound.topup(faction) : sound.keep(faction);
    if (faction && m.t === "keep" && m.ok) burst(m.node, faction, m.topup ? "claim" : "keep");
    if (faction && m.t === "kept" && m.state === "fallen") sound.lost(faction);
    if (m.t === "kept" && m.points) floaters.earned(m.points, m.node ? nodePos.get(m.node) : null);
    perm.onMsg(m);
    claim.update();
    sources.draw();
  };
  // Map changes come in bursts: the claim button looks once per frame.
  let updating = 0;
  const laterUpdate = () => {
    updating ||= requestAnimationFrame(() => {
      updating = 0;
      claim.update();
    });
  };
  // Watching only (watch.ts): flashes of fresh claims, the kiosk tour.
  let watchTap = false;
  let stopRise: (() => void) | null = null;
  let live: ReturnType<typeof setupLive> | null = null;
  const on: Handlers = {
    welcome(m) {
      // Only watching (watch.ts): the map and the live game, no side, no position.
      if (m.watch) {
        // Without a position: into the centre once, close enough to see streets.
        if (!look && !document.body.classList.contains("watching"))
          map.jumpTo({ center: meta.hotspot ?? meta.region.centroid, zoom: Math.max(map.getZoom(), 15) });
        document.body.classList.add("watching");
        // The top bar picks the city: "Zwettl ▾", below "Du schaust zu".
        const city = knownRegions().find((r) => r.slug === meta.region.slug)?.city ?? meta.region.name;
        $("me-label").textContent = kiosk ? city : `${city} ▾`;
        $("me-sub").textContent = t("watch.label");
        $("me").onclick = openCities;
        // A tap on a junction opens its card, as when playing (read-only).
        if (!kiosk && !watchTap) {
          watchTap = true;
          map.on("click", (e) => tapCard(e.point));
          // Watching: the ranking from the map buttons (the name opens the cities).
          $("nav-rank").hidden = false;
          $("nav-rank").onclick = board.openRank;
        }
        if (!kiosk) void joinGate(meta.region.slug, knownRegions);
        if (!live) {
          live = setupLive(map, () => t("watch.label"), (node, side) => burst(node, side, "other"));
          if (kiosk) startKiosk(map, meta.hotspot ?? meta.region.centroid, live, hotspots.at, lapse, board);
        }
        if (!update.onServerVersion(m.version) && !kiosk) news.check();
        if (lastView.length) net.send({ t: "view", tiles: lastView });
        if (overview.on()) net.send({ t: "overview", on: true });
        return;
      }
      if (!isCounted()) net.send({ t: "stats", on: false });
      sendProfile();
      gps.start(params.has("sim") && m.sim);
      document.body.classList.toggle("test", m.sim); // test server: dev data, sim allowed
      fxOn(m.sim);
      applyFaction(m.faction);
      sideBtn.hidden = !m.sim;
      // Test servers with levels: jump to a level (abilities, challenges).
      $("level-test").hidden = !(m.sim && m.levels);
      ($("level-test-pick") as HTMLSelectElement).onchange = (e) =>
        net.send({ t: "level", level: Number((e.target as HTMLSelectElement).value) });
      // Test servers: `?side=fire|water` picks the side (bookmarks, scripts).
      const want = params.get("side");
      if (m.sim && (want === "fire" || want === "water") && want !== m.faction) net.send({ t: "side", faction: want });
      progress.welcome(m);
      invite.welcome(m.invite, m.invite_points);
      hotspots.setRadius(m.hotspot_m);
      perm.onWelcome(m, m.faction);
      // What's new: in the app of the new version, not just before it reloads.
      if (!update.onServerVersion(m.version)) {
        news.check();
        checkInstall();
        void campaign.catchUp();
      }
      const me = player.me();
      if (me) net.send({ t: "pos", lat: me[1], lon: me[0] });
      // New connection: the server knows no tiles yet.
      if (lastView.length) net.send({ t: "view", tiles: lastView });
      if (overview.on()) net.send({ t: "overview", on: true });
      claim.update();
      card.refresh();
      progress.show();
      perm.drawList();
      // First time as a player: the side this browser got and what to do
      // first. Only a bias picks the side (server `join_side`), so the card
      // says nothing about weaker or stronger.
      if (newcomer && !storySeen("side") && !(m.sim && want && want !== m.faction)) {
        markStorySeen("side");
        const fire = m.faction === "fire";
        showStory({ cls: m.faction, kicker: t("side.card.kicker"), title: t(fire ? "you.fire" : "you.water"), text: t(fire ? "side.card.fire" : "side.card.water") });
      }
    },
    side(m) {
      applyFaction(m.faction);
      claim.update();
      card.refresh();
      progress.show();
      toast(t("side.switched", { side: t(`side.${m.faction}`) }));
    },
    state(m) {
      world.apply(m.nodes, tiles);
      card.changed(m.nodes.map((n) => n[0]));
      laterUpdate();
    },
    areas: (m) => world.areas(m.player, m.mine, tiles),
    overview: (m) => overview.rows(m.tiles, m.full),
    totals(m) {
      $("n-fire").textContent = `${m.fire}`;
      $("n-front").textContent = `${m.front}`;
      $("n-water").textContent = `${m.water}`;
    },
    cheers: (m) => cheer.onMsg(m),
    hotspots: (m) => hotspots.show(m.items),
    finds: (m) => finds.onFinds(m),
    find(m) {
      finds.onFind(m);
      sound.chime();
    },
    near: (m) => finds.onNear(m.c, m.m),
    badge(m) {
      finds.onBadge(m.badge);
      sound.motif();
    },
    rise(m) {
      // `[rise]`: stay at a sink (water) or a peak (fire), and a source rises.
      stopRise?.();
      stopRise = null;
      if (!faction) return;
      if (m.state === "wait") {
        toast(t(`rise.wait.${faction}`, { s: m.secs }));
        const ring = riseWait(map, [m.lon, m.lat], faction, m.secs);
        // A device standing still may send no fix: tell the server once the time is up.
        const due = setTimeout(() => {
          const me = player.me();
          if (me) net.send({ t: "pos", lat: me[1], lon: me[0] });
        }, (m.secs + 1) * 1000);
        stopRise = () => {
          ring();
          clearTimeout(due);
        };
      } else if (m.state === "done") {
        toast(m.took > 1 ? t(`rise.fill.${faction}`, { n: m.took }) : t(`rise.done.${faction}`));
        sound.keep(faction);
        burst(m.node, faction, "rise");
      } else toast(t(m.state === "strong" ? "rise.strong" : "rise.left"));
    },
    spark(m) {
      // A spark of the other element in your ground.
      toast(t(faction === "fire" ? "spark.water" : "spark.fire", { m: Math.round(m.m / 10) * 10 }));
      sound.spark(faction === "fire" ? "water" : "fire");
    },
    tunnel(m) {
      tunnel.onTunnel(m);
      sound.rumble();
    },
    chapter: (m) => campaign.onChapter(m),
    notices: (m) => onNotices(m.items),
    weather: (m) => weather.onWeather(m),
    tide: (m) => weather.onTide(m),
    info: (m) => card.onInfo(m),
    keep: onKeep,
    kept: onKeep,
    released: onKeep,
    motto: onKeep,
    link: onKeep,
    gift: onKeep,
    found(m) {
      // Funkenflug / Flaschenpost: a key to a source of the own side elsewhere.
      addGift({ region: m.region, key: m.key, at: Date.now(), origin: m.origin });
      toast(t(m.origin === "ember" ? "found.ember" : "found.bottle", { region: m.name }), 10_000);
      if (faction) sound.keep(faction);
    },
    peek: claim.onPeek,
    sources(m) {
      live?.rows(m.items);
      othersBurst(m.items);
      const now = Date.now();
      // An own source gone long before its time: taken over.
      if (faction) {
        const still = new Set(m.items.filter((i) => i[1] === faction).map((i) => i[0]));
        if (sources.all().some((s) => s.mine && s.faction === faction && s.kind !== "keep" && s.until - now > 5000 && !still.has(s.node))) sound.lost(faction);
      }
      sources.set(m.items, perm.nodes());
      if (staged.length) sources.apply(staged, [], perm.nodes());
      claim.update();
      card.refresh();
    },
    sourcesdiff(m) {
      live?.rows(m.set);
      othersBurst(m.set);
      const now = Date.now();
      // An own source gone or taken long before its time.
      if (faction) {
        const taken = new Set([...m.gone, ...m.set.filter((i) => i[1] !== faction).map((i) => i[0])]);
        if (sources.all().some((s) => s.mine && s.faction === faction && s.kind !== "keep" && s.until - now > 5000 && taken.has(s.node))) sound.lost(faction);
      }
      sources.apply(m.set, m.gone, perm.nodes());
      claim.update();
      card.changed([...m.set.map((i) => i[0]), ...m.gone]);
    },
    score(m) {
      progress.setMine(m);
      card.refresh(); // the level sets the claim's strength
      floaters.score(m.today);
      board.mine(m);
    },
    presence(m) {
      const r = presenceAnswer(m.ok, m.reason);
      if (r === "ok") toast(t("presence.ok"), 5000);
      else if (r === "retry" && m.reason === "far") toast(t("presence.far"), 4000);
    },
    busy: () => toast(t("net.busy"), 8000),
    forgotten() {
      clearToken();
      location.reload();
    },
    ability: (m) => {
      progress.onAbility(m);
      claim.update();
      card.refresh();
    },
    challenge: (m) => progress.onChallenge(m),
    challengedone: (m) => progress.onChallengeDone(m),
    challengepaused: () => progress.onChallengePaused(),
    ringbroken(m) {
      toast(t("ring.broken", { n: m.points }), 5000);
      floaters.earned(m.points, null, "big");
    },
    invited(m) {
      invite.paid(m.points);
      if (m.points > 0) floaters.earned(m.points, null, "big");
    },
    encircled(m) {
      toast(t(`siege.points.${m.side}` as Key, { n: m.points }), 5000);
      floaters.earned(m.points, null, "big");
    },
    code: (m) => recovery.show(m.code),
    recovered(m) {
      saveToken(m.token);
      location.reload();
    },
    recover: (m) => toast(t(m.reason === "wait" ? "keep.redeem.wait" : "keep.redeem.unknown")),
    rate(m) {
      claim.onRate(m);
      if (m.ok) floaters.earned(m.points);
    },
    claim(m) {
      claim.onClaim(m);
      if (m.ok) floaters.earned(m.points, nodePos.get(m.node));
      if (m.ok && faction) {
        const first = !load(KEYS.firstBurst);
        if (first) save(KEYS.firstBurst, "1");
        ownBurst.set(m.node, Date.now());
        burstThere(m.node, faction, first ? "first" : "claim");
      }
    },
    // Answered in net.ts.
    pong() {},
    pow() {},
  };
}

// Ask the browser not to clear our storage (mainly Android/Chrome).
navigator.storage?.persist?.().catch(() => {});

renderProfiles();
applyStatic();
langSwitch();
document.body.classList.toggle("game", gameView);
const gfxSel = $("gfx") as HTMLSelectElement;
gfxSel.value = choice;
// Show what "automatic" picked on this device (again after each text refresh).
const gfxAutoLabel = () => (gfxSel.options[0].text = `${t("gfx.auto")} (${t(`gfx.${autoGfx()}` as Key)})`);
gfxAutoLabel();
gfxSel.onchange = () => {
  setGfx(gfxSel.value as GfxChoice);
  location.reload();
};
const cityKnown = () => {
  // City known, and maybe the region's language instead of German.
  applyStatic();
  gfxAutoLabel();
  langSwitch();
  renderProfiles();
  document.title = t("title");
};
// Links to the rules and legal pages keep an explicitly chosen language.
if (langChosen) for (const a of document.querySelectorAll<HTMLAnchorElement>('a[href$=".html"]')) a.href = `${a.getAttribute("href")}?lang=${lang}`;

// First visit: short intro card with a link to the rules.
const introSeen = () => {
  $("intro").hidden = true;
  save(KEYS.intro, "1");
};
$("intro-ok").onclick = () => {
  introSeen();
  // Watching: the same way as "Mitspielen" (only with a position here).
  if (watching()) void tryJoin();
};
$("intro-watch").onclick = introSeen;
$("intro-watch").hidden = !watching();
$("intro").querySelector("a")!.addEventListener("click", introSeen);

/** Cities whose centre has free public Wi-Fi (the hint when the connection drops). */
const FREE_WIFI = new Set(["region-konstanz"]);

// Where to play first (or the page for places without Elements), then the game.
chooseRegion()
  .then((play) => {
    if (!play) return;
    setupCities();
    loadCity(cityKnown);
    // Storage blocked: skip the intro (it would come every visit).
    if (!blocked && !kiosk && !load(KEYS.intro)) $("intro").hidden = false;
    return main();
  })
  .catch((e) => {
    $("me-label").textContent = t("load.error");
    console.error(e);
  });

/** Buttons for trying the bursts without playing (`?fx`): nearest junction to the map centre. */
function fxDemo(map: maplibregl.Map, nodePos: Map<number, [number, number]>, burst: (node: number, side: Faction, kind: BurstKind) => void) {
  const bar = document.createElement("div");
  bar.className = "fx-demo";
  const fire = (side: Faction, kind: BurstKind) => {
    const c = map.getCenter();
    const k = Math.cos((c.lat * Math.PI) / 180);
    let best = 0;
    let bd = Infinity;
    for (const [id, [lon, lat]] of nodePos) {
      const d = ((lon - c.lng) * k) ** 2 + (lat - c.lat) ** 2;
      if (d < bd) [best, bd] = [id, d];
    }
    if (best) burst(best, side, kind);
  };
  const add = (label: string, side: Faction, kind: BurstKind) => {
    const b = document.createElement("button");
    b.textContent = label;
    b.onclick = () => fire(side, kind);
    bar.append(b);
  };
  add("🔥", "fire", "claim");
  add("💧", "water", "claim");
  add("🔥 erste", "fire", "first");
  add("🔥 Glut", "fire", "keep");
  add("❄ Eis", "water", "keep");
  document.body.append(bar);
}
