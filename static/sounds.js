"use strict";
// Sons do Concord: efeitos da interface e Soundpad.
//
// Nenhum arquivo de áudio é baixado para os efeitos nem para os sons padrão do
// Soundpad: tudo é sintetizado aqui com Web Audio. Preferências ficam só neste
// navegador (localStorage), como as de voz.

// ---------- motor de áudio ----------
let actx = null;
let sfxGain = null;
let boardGain = null;

const sndPrefs = Object.assign(
  { volume: 0.6, board: 0.8, off: [] },
  load("concord.sounds", {}),
);
const saveSndPrefs = () => store("concord.sounds", sndPrefs);

function audio() {
  if (!actx) {
    const Ctx = window.AudioContext || window.webkitAudioContext;
    if (!Ctx) return null;
    actx = new Ctx();
    sfxGain = actx.createGain();
    boardGain = actx.createGain();
    sfxGain.connect(actx.destination);
    boardGain.connect(actx.destination);
    applySoundVolumes();
    applySoundOutput();
  }
  if (actx.state === "suspended") actx.resume().catch(() => {});
  return actx;
}

function applySoundVolumes() {
  if (!actx) return;
  sfxGain.gain.value = sndPrefs.volume;
  boardGain.gain.value = sndPrefs.board;
}

/** Mesma saída de áudio escolhida em Voz e vídeo (quando o navegador permite). */
function applySoundOutput() {
  if (!actx?.setSinkId || typeof vset === "undefined") return;
  actx.setSinkId(vset.output === "default" ? "" : vset.output).catch(() => {});
}

// O navegador só libera áudio depois de um gesto da pessoa.
for (const ev of ["pointerdown", "keydown"]) {
  document.addEventListener(ev, () => { if (actx?.state === "suspended") actx.resume().catch(() => {}); }, { passive: true });
}

// Gerador pseudoaleatório com semente: todos ouvem exatamente o mesmo som.
function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** Envelope: sobe em `a`, sustenta até `hold` e some em `rel`. */
function envelope(param, t, g, a, hold, rel) {
  param.setValueAtTime(0.0001, t);
  param.linearRampToValueAtTime(g, t + a);
  param.setValueAtTime(g, t + Math.max(a, hold));
  param.exponentialRampToValueAtTime(0.0001, t + Math.max(a, hold) + rel);
}

/** Nota simples. `f2`: desliza até essa frequência. `lp`: filtro passa-baixa. */
function tone(ctx, out, { f, f2, t = 0, d = 0.15, g = 0.25, type = "sine", a = 0.006, hold = 0, lp, vib }) {
  const o = ctx.createOscillator();
  o.type = type;
  o.frequency.setValueAtTime(f, t);
  if (f2) o.frequency.exponentialRampToValueAtTime(f2, t + Math.max(d * 0.8, 0.02));
  const gn = ctx.createGain();
  envelope(gn.gain, t, g, a, hold, d);
  let node = o;
  if (lp) {
    const fl = ctx.createBiquadFilter();
    fl.type = "lowpass";
    fl.frequency.value = lp;
    node.connect(fl);
    node = fl;
  }
  if (vib) {
    const l = ctx.createOscillator();
    const lg = ctx.createGain();
    l.frequency.value = vib.rate;
    lg.gain.value = vib.depth;
    l.connect(lg).connect(o.frequency);
    l.start(t);
    l.stop(t + hold + d + 0.05);
  }
  node.connect(gn).connect(out);
  o.start(t);
  o.stop(t + hold + d + 0.05);
}

function noise(ctx, out, { t = 0, d = 0.2, g = 0.3, type = "highpass", f = 4000, q = 0.8, a = 0.002, seed = 7 }) {
  const len = Math.max(1, Math.ceil(ctx.sampleRate * (d + 0.05)));
  const buf = ctx.createBuffer(1, len, ctx.sampleRate);
  const ch = buf.getChannelData(0);
  const r = rng(seed);
  for (let i = 0; i < len; i++) ch[i] = r() * 2 - 1;
  const src = ctx.createBufferSource();
  src.buffer = buf;
  const fl = ctx.createBiquadFilter();
  fl.type = type;
  fl.frequency.value = f;
  fl.Q.value = q;
  const gn = ctx.createGain();
  envelope(gn.gain, t, g, a, 0, d);
  src.connect(fl).connect(gn).connect(out);
  src.start(t);
}

// ---------- efeitos da interface ----------
const SFX_EVENTS = [
  ["message_sent", "Enviar mensagem"],
  ["notify", "Menção ou resposta para você"],
  ["voice_join", "Entrar em um canal de voz"],
  ["voice_leave", "Sair de um canal de voz"],
  ["user_join", "Alguém entra na sua chamada"],
  ["user_leave", "Alguém sai da sua chamada"],
  ["mute", "Silenciar ou ativar o microfone"],
  ["deafen", "Desativar ou ativar o áudio"],
  ["stream", "Começar ou parar de transmitir"],
  ["disconnect", "Conexão perdida (tentando reconectar)"],
  ["reconnect", "Conexão restabelecida"],
];

const SFX = {
  message_sent: (c, o, t) => tone(c, o, { f: 620, f2: 940, t, d: 0.07, g: 0.12, type: "triangle" }),
  notify: (c, o, t) => {
    tone(c, o, { f: 988, t, d: 0.16, g: 0.2, type: "triangle" });
    tone(c, o, { f: 1480, t: t + 0.11, d: 0.3, g: 0.17, type: "triangle" });
  },
  voice_join: (c, o, t) => [523, 659, 784].forEach((f, i) => tone(c, o, { f, t: t + i * 0.075, d: 0.16, g: 0.16 })),
  voice_leave: (c, o, t) => [784, 659, 494].forEach((f, i) => tone(c, o, { f, t: t + i * 0.075, d: 0.16, g: 0.16 })),
  user_join: (c, o, t) => { tone(c, o, { f: 587, t, d: 0.12, g: 0.13 }); tone(c, o, { f: 880, t: t + 0.08, d: 0.18, g: 0.13 }); },
  user_leave: (c, o, t) => { tone(c, o, { f: 880, t, d: 0.12, g: 0.13 }); tone(c, o, { f: 587, t: t + 0.08, d: 0.18, g: 0.13 }); },
  mute: (c, o, t, on) => tone(c, o, { f: on ? 480 : 360, f2: on ? 340 : 500, t, d: 0.09, g: 0.15, type: "triangle" }),
  deafen: (c, o, t, on) => {
    tone(c, o, { f: on ? 440 : 294, t, d: 0.09, g: 0.14, type: "triangle" });
    tone(c, o, { f: on ? 294 : 440, t: t + 0.08, d: 0.12, g: 0.14, type: "triangle" });
  },
  stream: (c, o, t, on) => (on ? [440, 660, 880] : [880, 660, 440]).forEach((f, i) => tone(c, o, { f, t: t + i * 0.05, d: 0.08, g: 0.12, type: "square", lp: 2200 })),
  disconnect: (c, o, t) => {
    for (const dt of [0, 0.22]) {
      tone(c, o, { f: 220, t: t + dt, d: 0.16, g: 0.12, type: "sawtooth", lp: 900 });
      tone(c, o, { f: 233, t: t + dt, d: 0.16, g: 0.12, type: "sawtooth", lp: 900 });
    }
  },
  reconnect: (c, o, t) => { tone(c, o, { f: 659, t, d: 0.1, g: 0.14 }); tone(c, o, { f: 1318, t: t + 0.07, d: 0.22, g: 0.14 }); },
};

/** Toca um efeito da interface, se estiver ligado. `on`: variação (ex.: silenciar x ativar). */
function playSfx(name, on = true) {
  if (sndPrefs.off.includes(name) || sndPrefs.volume <= 0) return;
  const ctx = audio();
  if (!ctx || ctx.state !== "running") return;
  try { SFX[name]?.(ctx, sfxGain, ctx.currentTime + 0.01, on); } catch {}
}

// ---------- sons padrão do Soundpad (sintetizados, iguais para todos) ----------
const BUILTINS = [
  { key: "badumtss", name: "Ba dum tss", emoji: "🥁", play(c, o, t) {
    tone(c, o, { f: 190, f2: 90, t, d: 0.18, g: 0.5 });
    tone(c, o, { f: 150, f2: 70, t: t + 0.17, d: 0.2, g: 0.5 });
    noise(c, o, { t: t + 0.42, d: 1.1, g: 0.22, f: 7000, seed: 3 });
    tone(c, o, { f: 60, f2: 40, t: t + 0.42, d: 0.25, g: 0.45 });
  } },
  { key: "buzina", name: "Buzina", emoji: "📯", play(c, o, t) {
    for (const [s, h] of [[0, 0.16], [0.26, 0.16], [0.52, 0.75]]) {
      for (const f of [370, 374, 555]) tone(c, o, { f, t: t + s, d: 0.08, hold: h, g: f > 500 ? 0.06 : 0.11, type: "sawtooth", lp: 2600, a: 0.015 });
    }
  } },
  { key: "trombone", name: "Trombone triste", emoji: "🎺", play(c, o, t) {
    const notes = [[392, 0], [370, 0.48], [349, 0.96]];
    for (const [f, s] of notes) tone(c, o, { f, t: t + s, d: 0.12, hold: 0.32, g: 0.16, type: "sawtooth", lp: 1300, a: 0.03 });
    tone(c, o, { f: 330, f2: 300, t: t + 1.44, d: 0.35, hold: 0.9, g: 0.16, type: "sawtooth", lp: 1300, a: 0.03, vib: { rate: 5.5, depth: 7 } });
  } },
  { key: "tada", name: "Tadã!", emoji: "🎉", play(c, o, t) {
    tone(c, o, { f: 523, t, d: 0.08, hold: 0.06, g: 0.14, type: "sawtooth", lp: 3000 });
    for (const f of [523, 659, 784, 1046]) tone(c, o, { f, t: t + 0.16, d: 0.9, hold: 0.25, g: 0.09, type: "sawtooth", lp: 3200, vib: { rate: 6, depth: 4 } });
  } },
  { key: "moeda", name: "Plim!", emoji: "🪙", play(c, o, t) {
    for (const [f, g] of [[2093, 0.18], [3140, 0.08], [4410, 0.05], [5280, 0.03]]) tone(c, o, { f, t, d: 0.9, g, a: 0.002 });
  } },
  { key: "sino", name: "Sino", emoji: "🔔", play(c, o, t) {
    for (const [m, g, d] of [[1, 0.22, 2.2], [2, 0.1, 1.6], [2.76, 0.08, 1.2], [5.4, 0.04, 0.7], [0.5, 0.08, 2.4]]) tone(c, o, { f: 523 * m, t, d, g, a: 0.003 });
  } },
  { key: "grilos", name: "Grilos", emoji: "🦗", play(c, o, t) {
    for (let k = 0; k < 6; k++) {
      const s = t + k * 0.42;
      for (let j = 0; j < 4; j++) tone(c, o, { f: 4400, t: s + j * 0.045, d: 0.03, g: 0.05, a: 0.004 });
    }
  } },
  { key: "palmas", name: "Palmas", emoji: "👏", play(c, o, t) {
    const r = rng(42);
    for (let k = 0; k < 70; k++) {
      const s = t + r() * 2.2 * (k < 60 ? 1 : 1.1);
      noise(c, o, { t: s, d: 0.05 + r() * 0.04, g: 0.08 + r() * 0.08, type: "bandpass", f: 1200 + r() * 1400, q: 1.2, seed: k + 1 });
    }
  } },
  { key: "laser", name: "Laser", emoji: "⚡", play(c, o, t) {
    for (const s of [0, 0.28]) tone(c, o, { f: 2000, f2: 180, t: t + s, d: 0.26, g: 0.12, type: "square", lp: 4000 });
  } },
  { key: "boing", name: "Boing", emoji: "🌀", play(c, o, t) {
    tone(c, o, { f: 140, f2: 420, t, d: 0.7, g: 0.3, vib: { rate: 14, depth: 60 } });
  } },
];
const builtinByKey = new Map(BUILTINS.map((b) => [b.key, b]));

// ---------- Soundpad: estado ----------
const sp = {
  lists: new Map(), // serverId -> { sounds, can_manage, max }
  waiters: new Map(), // serverId -> [resolve]
  buffers: new Map(), // id do som -> Promise<AudioBuffer>
  favorites: load("concord.sp.favorites", []),
  recent: load("concord.sp.recent", []),
  pad: null,
  uploadWaiters: [],
};

function fetchSounds(serverId) {
  return new Promise((resolve) => {
    const list = sp.waiters.get(serverId) ?? [];
    list.push(resolve);
    sp.waiters.set(serverId, list);
    send({ type: "fetch_sounds", server_id: serverId });
    setTimeout(() => resolve(sp.lists.get(serverId) ?? null), 8000);
  });
}

async function ensureSounds(serverId) {
  return sp.lists.get(serverId) ?? fetchSounds(serverId);
}

function onSounds(m) {
  sp.lists.set(m.server_id, { sounds: m.sounds, can_manage: m.can_manage, max: m.max });
  for (const r of sp.waiters.get(m.server_id) ?? []) r(sp.lists.get(m.server_id));
  sp.waiters.delete(m.server_id);
  if (sp.pad && sp.pad.serverId === m.server_id) renderPad();
}

function onSoundsChanged(m) {
  if (sp.lists.has(m.server_id) || sp.pad?.serverId === m.server_id) fetchSounds(m.server_id);
}

/** Som pela chave: "builtin:nome" ou id de um som do servidor. */
function soundInfo(serverId, key) {
  if (key.startsWith("builtin:")) {
    const b = builtinByKey.get(key.slice(8));
    return b ? { key, name: b.name, emoji: b.emoji, builtin: b } : null;
  }
  const s = sp.lists.get(serverId)?.sounds.find((x) => x.id === key);
  return s ? { key, name: s.name, emoji: s.emoji, sound: s } : null;
}

async function bufferFor(sound) {
  let p = sp.buffers.get(sound.id);
  if (!p) {
    p = fetch(sound.url)
      .then((r) => { if (!r.ok) throw new Error(String(r.status)); return r.arrayBuffer(); })
      .then((b) => audio().decodeAudioData(b));
    p.catch(() => sp.buffers.delete(sound.id));
    sp.buffers.set(sound.id, p);
  }
  return p;
}

/** Toca localmente (para todos na chamada isso acontece ao receber `sound_played`). */
async function playLocal(info) {
  const ctx = audio();
  if (!ctx || !info) return;
  if (info.builtin) {
    try { info.builtin.play(ctx, boardGain, ctx.currentTime + 0.02); } catch {}
    return;
  }
  try {
    const buf = await bufferFor(info.sound);
    const src = ctx.createBufferSource();
    src.buffer = buf;
    src.connect(boardGain);
    src.start();
  } catch {
    toast("Não foi possível tocar o som");
  }
}

async function onSoundPlayed(m) {
  const inCall = typeof voice !== "undefined" && voice.room && voice.channelId === m.channel_id;
  if (!inCall) return;
  markSoundPlayer(m);
  if (voice.deafened || myServerMute().deafened) return;
  let info = soundInfo(m.server_id, m.sound);
  if (!info && !m.sound.startsWith("builtin:")) {
    await fetchSounds(m.server_id);
    info = soundInfo(m.server_id, m.sound);
  }
  playLocal(info);
}

/** Mostra o emoji do som em cima de quem tocou (lista da chamada e blocos de vídeo). */
function markSoundPlayer(m) {
  const info = soundInfo(m.server_id, m.sound);
  for (const e of document.querySelectorAll(`[data-speaker="${CSS.escape(m.user_id)}"]`)) {
    e.classList.add("sound-playing");
    const bubble = el("span", "sp-bubble", info?.emoji || "🔊");
    bubble.title = info?.name ?? "Som";
    e.append(bubble);
    setTimeout(() => { bubble.remove(); e.classList.remove("sound-playing"); }, 1800);
  }
}

function playToCall(key) {
  if (!voice.room) return toast("Entre num canal de voz para usar o Soundpad");
  audio();
  send({ type: "play_sound", sound: key });
  sp.recent = [key, ...sp.recent.filter((k) => k !== key)].slice(0, 12);
  store("concord.sp.recent", sp.recent);
}

function toggleFavorite(key) {
  const i = sp.favorites.indexOf(key);
  if (i >= 0) sp.favorites.splice(i, 1);
  else sp.favorites.unshift(key);
  store("concord.sp.favorites", sp.favorites);
  renderPad();
}

// ---------- Soundpad: painel ----------
ICONS.clock = "M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0zM12 6v6l4 2";
ICONS.music = "M9 18V5l12-2v13M9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM21 16a3 3 0 1 1-6 0 3 3 0 0 1 6 0z";
ICONS.search = "M21 21l-4.3-4.3M19 11a8 8 0 1 1-16 0 8 8 0 0 1 16 0z";

function openSoundpad(anchor) {
  if (sp.pad) return closeSoundpad();
  if (typeof setDrawer === "function") setDrawer(null);
  const serverId = voice.serverId ?? state.serverId;
  if (!serverId) return;
  audio();
  const root = el("div", "soundpad");
  root.setAttribute("role", "dialog");
  root.setAttribute("aria-label", "Soundpad");
  sp.pad = { root, serverId, query: "", showVolume: false, anchor };
  document.body.append(root);
  renderPad(!window.matchMedia("(max-width: 760px)").matches); // no celular, sem teclado de cara
  ensureSounds(serverId);
  placePad();
  if (typeof renderVoiceControls === "function") renderVoiceControls();
}

function placePad() {
  const pad = sp.pad;
  if (!pad || window.matchMedia("(max-width: 760px)").matches) return;
  // Acima do painel de voz (sem cobrir o botão de desligar) ou acima da barra da chamada.
  const box = pad.anchor.closest("#voice-panel, .vv-controls") ?? pad.anchor;
  const r = box.getBoundingClientRect();
  const a = pad.anchor.getBoundingClientRect();
  const w = pad.root.offsetWidth;
  const left = box.id === "voice-panel" ? r.left : a.left + a.width / 2 - w / 2;
  pad.root.style.left = `${Math.max(8, Math.min(left, window.innerWidth - w - 8))}px`;
  pad.root.style.bottom = `${Math.max(8, window.innerHeight - r.top + 8)}px`;
}

function closeSoundpad() {
  if (!sp.pad) return;
  sp.pad.root.remove();
  sp.pad = null;
  if (typeof renderVoiceControls === "function") renderVoiceControls();
}

document.addEventListener("pointerdown", (e) => {
  if (!sp.pad) return;
  if (sp.pad.root.contains(e.target) || e.target.closest?.(".menu, #modal, .emoji-picker, #vp-soundpad, [data-act=\"sounds\"]")) return;
  closeSoundpad();
});
document.addEventListener("keydown", (e) => { if (e.key === "Escape" && sp.pad && !openMenu) closeSoundpad(); });
window.addEventListener("resize", placePad);

function renderPad(focusSearch) {
  const pad = sp.pad;
  if (!pad) return;
  const server = state.servers.find((s) => s.id === pad.serverId);
  const data = sp.lists.get(pad.serverId);
  const root = pad.root;
  const scrollTop = root.querySelector(".sp-body")?.scrollTop ?? 0;
  root.replaceChildren();

  // Busca e volume
  const head = el("div", "sp-head");
  const searchWrap = el("label", "sp-search");
  searchWrap.append(icon("search", "sp-search-icon"));
  const search = el("input");
  search.type = "search";
  search.placeholder = "Encontre o som perfeito";
  search.value = pad.query;
  search.setAttribute("aria-label", "Buscar som");
  search.oninput = () => { pad.query = search.value; renderPad(true); };
  searchWrap.append(search);
  const volBtn = button("", "icon-btn", () => { pad.showVolume = !pad.showVolume; renderPad(); });
  volBtn.append(icon("volume"));
  volBtn.title = "Volume do Soundpad (só para você)";
  volBtn.setAttribute("aria-label", "Volume do Soundpad");
  volBtn.classList.toggle("active", pad.showVolume);
  head.append(searchWrap, volBtn);
  root.append(head);
  if (pad.showVolume) {
    const row = el("div", "sp-volume");
    const range = el("input");
    range.type = "range";
    range.min = "0";
    range.max = "100";
    range.value = String(Math.round(sndPrefs.board * 100));
    const label = el("span", null, `${range.value}%`);
    range.oninput = () => {
      sndPrefs.board = Number(range.value) / 100;
      label.textContent = `${range.value}%`;
      applySoundVolumes();
      saveSndPrefs();
    };
    row.append(el("span", null, "Volume dos sons que você ouve"), range, label);
    root.append(row);
  }

  const q = pad.query.trim().toLowerCase();
  const match = (info) => !q || info.name.toLowerCase().includes(q) || (info.emoji ?? "") === q;
  const serverInfos = (data?.sounds ?? []).map((s) => soundInfo(pad.serverId, s.id)).filter(Boolean);
  const builtinInfos = BUILTINS.map((b) => soundInfo(pad.serverId, `builtin:${b.key}`));
  const resolve = (keys) => keys.map((k) => soundInfo(pad.serverId, k)).filter(Boolean);

  const sections = [
    { id: "fav", title: "Favoritos", icon: "star", items: resolve(sp.favorites) },
    { id: "recent", title: "Usados recentemente", icon: "clock", items: resolve(sp.recent).slice(0, 6) },
    { id: "server", title: server?.name ?? "Servidor", server, items: serverInfos, add: !!data?.can_manage, loading: !data },
    { id: "builtin", title: "Padrão do Concord", icon: "music", items: builtinInfos },
  ];

  const main = el("div", "sp-main");
  const nav = el("nav", "sp-nav");
  const body = el("div", "sp-body");
  for (const sec of sections) {
    const items = sec.items.filter(match);
    if (q && items.length === 0) continue;
    if (!q && sec.items.length === 0 && !sec.add && !sec.loading && sec.id !== "fav") continue;
    const navBtn = el("button", "sp-nav-btn");
    navBtn.type = "button";
    navBtn.title = sec.title;
    navBtn.setAttribute("aria-label", sec.title);
    if (sec.server) {
      navBtn.classList.add("server");
      navBtn.textContent = initials(sec.server.name);
    } else navBtn.append(icon(sec.icon));
    nav.append(navBtn);

    const block = el("section", "sp-section");
    const title = el("div", "sp-title");
    if (sec.icon) title.append(icon(sec.icon, "sp-title-icon"));
    title.append(el("span", null, sec.title));
    if (sec.id === "server" && data) title.append(el("span", "sp-count", `${data.sounds.length}/${data.max}`));
    block.append(title);
    navBtn.onclick = () => block.scrollIntoView({ behavior: "smooth", block: "start" });

    const grid = el("div", "sp-grid");
    for (const info of items) grid.append(soundTileEl(info, sec.id === "server" && data?.can_manage));
    if (sec.add && !q) {
      const add = button("", "sp-tile add", () => addSoundDialog(pad.serverId));
      add.append(icon("plus", "sp-add-icon"), el("span", "sp-name", "Adicionar som"));
      add.disabled = data.sounds.length >= data.max;
      grid.append(add);
    }
    if (sec.loading) grid.append(el("div", "sp-empty", "Carregando…"));
    else if (sec.id === "fav" && items.length === 0) grid.append(el("div", "sp-empty", "Clique com o botão direito (ou segure) num som para favoritar."));
    else if (sec.id === "server" && items.length === 0 && !sec.add) grid.append(el("div", "sp-empty", "Este servidor ainda não tem sons próprios."));
    block.append(grid);
    body.append(block);
  }
  if (!body.children.length) body.append(el("div", "sp-empty big", "Nenhum som encontrado"));
  main.append(nav, body);
  root.append(main);

  const foot = el("div", "sp-foot");
  foot.textContent = voice.room
    ? "Todos na chamada ouvem o som. Volume e favoritos ficam só neste navegador."
    : "Entre num canal de voz para tocar sons para todos.";
  root.append(foot);

  body.scrollTop = scrollTop;
  if (focusSearch) {
    search.focus();
    search.setSelectionRange(search.value.length, search.value.length);
  }
}

function soundTileEl(info, canDelete) {
  const b = el("button", "sp-tile");
  b.type = "button";
  if (info.emoji) b.append(el("span", "sp-emoji", info.emoji));
  b.append(el("span", "sp-name", info.name));
  b.title = `${info.name}${info.sound ? ` · ${(info.sound.duration_ms / 1000).toFixed(1)} s` : ""}`;
  if (sp.favorites.includes(info.key)) b.classList.add("fav");
  b.onclick = () => {
    b.classList.remove("pressed");
    void b.offsetWidth;
    b.classList.add("pressed");
    playToCall(info.key);
  };
  const menu = (e) => {
    e.preventDefault();
    const fav = sp.favorites.includes(info.key);
    showMenu(e, [
      { label: "Ouvir só para mim", icon: "volume", onClick: () => playLocal(info) },
      { label: fav ? "Remover dos favoritos" : "Favoritar", icon: "star", onClick: () => toggleFavorite(info.key) },
      canDelete && info.sound && "-",
      canDelete && info.sound && { label: "Apagar som", icon: "trash", danger: true, onClick: () => deleteSound(info) },
    ]);
  };
  b.oncontextmenu = menu;
  longPress(b, menu);
  return b;
}

async function deleteSound(info) {
  const ok = await dialog({ title: `Apagar "${info.name}"?`, desc: "O som some do Soundpad do servidor para todos.", submit: "Apagar", danger: true });
  if (ok) send({ type: "delete_sound", sound_id: info.key });
}

// ---------- adicionar som (moderação) ----------
const MAX_SOUND_SECONDS = 5.2;
const SOUND_RATE = 48000;

/** Converte qualquer áudio que o navegador entenda em WAV mono 16 bits, sem metadados. */
async function encodeSoundClip(buffer, start, duration) {
  const len = Math.max(1, Math.round(duration * SOUND_RATE));
  const off = new OfflineAudioContext(1, len, SOUND_RATE);
  const src = off.createBufferSource();
  src.buffer = buffer;
  const g = off.createGain();
  // Pequenas rampas evitam estalos no corte.
  g.gain.setValueAtTime(0, 0);
  g.gain.linearRampToValueAtTime(1, 0.008);
  g.gain.setValueAtTime(1, Math.max(0.01, duration - 0.03));
  g.gain.linearRampToValueAtTime(0, duration);
  src.connect(g).connect(off.destination);
  src.start(0, start, duration);
  const out = await off.startRendering();
  const pcm = out.getChannelData(0);
  const view = new DataView(new ArrayBuffer(44 + pcm.length * 2));
  const ascii = (o, s) => { for (let i = 0; i < s.length; i++) view.setUint8(o + i, s.charCodeAt(i)); };
  ascii(0, "RIFF");
  view.setUint32(4, 36 + pcm.length * 2, true);
  ascii(8, "WAVEfmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, SOUND_RATE, true);
  view.setUint32(28, SOUND_RATE * 2, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  ascii(36, "data");
  view.setUint32(40, pcm.length * 2, true);
  for (let i = 0; i < pcm.length; i++) {
    const s = Math.max(-1, Math.min(1, pcm[i]));
    view.setInt16(44 + i * 2, s < 0 ? s * 0x8000 : s * 0x7fff, true);
  }
  return new Blob([view.buffer], { type: "audio/wav" });
}

function onSoundUploadToken(m) {
  sp.uploadWaiters.shift()?.resolve(m);
}

function waitSoundToken() {
  return new Promise((resolve, reject) => {
    const w = { resolve };
    sp.uploadWaiters.push(w);
    setTimeout(() => {
      const i = sp.uploadWaiters.indexOf(w);
      if (i >= 0) { sp.uploadWaiters.splice(i, 1); reject(new Error("timeout")); }
    }, 10000);
  });
}

function addSoundDialog(serverId) {
  const card = $("#modal-card");
  card.replaceChildren();
  card.append(el("h2", null, "Adicionar som"));
  card.append(el("p", "muted center", "Até 5 segundos. O áudio é convertido aqui no seu navegador e chega ao servidor sem metadados (autor, software, datas)."));

  let decoded = null;
  let emoji = null;
  const file = el("input", "field");
  file.type = "file";
  file.accept = "audio/*,.mp3,.ogg,.wav,.m4a,.webm,.flac";
  const fileLabel = el("label", "field-label", "Arquivo de áudio");
  const name = el("input", "field");
  name.maxLength = 32;
  name.placeholder = "Nome do som";
  name.autocomplete = "off";
  const emojiBtn = button("Emoji (opcional)", "btn secondary sp-emoji-pick", (e) => {
    e.stopPropagation();
    openEmojiPicker(emojiBtn, (x) => { emoji = x; emojiBtn.textContent = `${x}  Trocar emoji`; });
  });
  const trim = el("div", "sp-trim hidden");
  const startRange = el("input");
  startRange.type = "range";
  startRange.min = "0";
  startRange.step = "0.1";
  startRange.value = "0";
  const trimLabel = el("span", "help");
  trim.append(el("label", "field-label", "Trecho"), startRange, trimLabel);
  const clipLen = () => (decoded ? Math.min(MAX_SOUND_SECONDS, decoded.duration - Number(startRange.value)) : 0);
  const updateTrim = () => {
    paintRange(startRange);
    const s = Number(startRange.value);
    trimLabel.textContent = `De ${s.toFixed(1)} s a ${(s + clipLen()).toFixed(1)} s (de ${decoded.duration.toFixed(1)} s)`;
  };
  startRange.oninput = updateTrim;
  const status = el("p", "help");
  const preview = button("Ouvir", "btn secondary", () => {
    if (!decoded) return;
    const ctx = audio();
    const src = ctx.createBufferSource();
    src.buffer = decoded;
    src.connect(boardGain);
    src.start(0, Number(startRange.value), clipLen());
  });
  preview.disabled = true;

  file.onchange = async () => {
    decoded = null;
    preview.disabled = true;
    const f = file.files[0];
    if (!f) return;
    if (f.size > 25 * 1024 * 1024) return (status.textContent = "Arquivo grande demais (até 25 MB antes de cortar).");
    status.textContent = "Lendo o áudio…";
    try {
      decoded = await audio().decodeAudioData(await f.arrayBuffer());
    } catch {
      status.textContent = "Não foi possível ler este arquivo. Tente MP3, OGG ou WAV.";
      return;
    }
    status.textContent = "";
    preview.disabled = false;
    if (!name.value) name.value = f.name.replace(/\.[^.]+$/, "").slice(0, 32);
    const extra = Math.max(0, decoded.duration - MAX_SOUND_SECONDS);
    trim.classList.toggle("hidden", extra <= 0);
    startRange.max = extra.toFixed(1);
    startRange.value = "0";
    updateTrim();
  };

  const row = el("div", "row");
  row.append(emojiBtn, preview);
  card.append(fileLabel, file, el("label", "field-label", "Nome"), name, row, trim, status);

  const actions = el("div", "actions");
  const cancel = button("Cancelar", null, () => finish());
  const ok = el("button", "primary", "Enviar");
  ok.type = "submit";
  actions.append(cancel, ok);
  card.append(actions);
  show("#modal");
  file.focus();

  function finish() {
    hide("#modal");
    card.onsubmit = null;
    $("#modal").onkeydown = null;
  }
  $("#modal").onkeydown = (e) => { if (e.key === "Escape") finish(); };
  card.onsubmit = async (e) => {
    e.preventDefault();
    if (!decoded) return (status.textContent = "Escolha um arquivo de áudio.");
    if (!name.value.trim()) return name.focus();
    ok.disabled = true;
    status.textContent = "Convertendo…";
    try {
      const wav = await encodeSoundClip(decoded, Number(startRange.value), clipLen());
      const tokenP = waitSoundToken();
      send({ type: "sound_upload_begin", server_id: serverId, size: wav.size, name: name.value.trim(), emoji });
      const g = await tokenP;
      status.textContent = "Enviando…";
      const r = await fetch(`/sons/${g.upload_id}?token=${encodeURIComponent(g.token)}`, { method: "POST", body: wav });
      if (!r.ok) {
        const j = await r.json().catch(() => ({}));
        throw new Error(j.error || `HTTP ${r.status}`);
      }
      finish();
      toast(`Som "${name.value.trim()}" adicionado`, true);
    } catch (err) {
      status.textContent = err.message === "timeout" ? "O servidor recusou o envio." : `Erro: ${err.message}`;
      ok.disabled = false;
    }
  };
}

// ---------- configurações: Sons ----------
function renderSoundSettings(c) {
  settingsHeader(c, "Sons", "Efeitos sonoros da interface e volume do Soundpad. Ficam só neste navegador.");
  const slider = (value, onInput) => {
    const wrap = el("div", "range-row");
    const r = el("input");
    r.type = "range";
    r.min = "0";
    r.max = "100";
    r.value = String(Math.round(value * 100));
    const label = el("span", null, `${r.value}%`);
    r.oninput = () => { label.textContent = `${r.value}%`; onInput(Number(r.value) / 100); };
    wrap.append(r, label);
    return wrap;
  };
  const s1 = settingsSection(c, "Volume dos efeitos");
  s1.append(slider(sndPrefs.volume, (v) => { sndPrefs.volume = v; applySoundVolumes(); saveSndPrefs(); }));
  const s2 = settingsSection(c, "Volume do Soundpad", "Volume dos sons que outras pessoas tocam na chamada.");
  s2.append(slider(sndPrefs.board, (v) => { sndPrefs.board = v; applySoundVolumes(); saveSndPrefs(); }));
  const s3 = settingsSection(c, "Efeitos sonoros", "Escolha quando o Concord toca um som. Use ▶ para ouvir.");
  const list = el("div", "sound-list");
  for (const [key, label] of SFX_EVENTS) {
    const row = el("div", "sound-row");
    const play = button("▶", "icon-btn", () => {
      const ctx = audio();
      if (ctx) SFX[key](ctx, sfxGain, ctx.currentTime + 0.02, true);
    });
    play.title = "Ouvir";
    play.setAttribute("aria-label", `Ouvir: ${label}`);
    row.append(play, el("span", "sound-label", label), switchToggle(!sndPrefs.off.includes(key), (on) => {
      sndPrefs.off = sndPrefs.off.filter((k) => k !== key);
      if (!on) sndPrefs.off.push(key);
      saveSndPrefs();
    }));
    list.append(row);
  }
  s3.append(list);
}

// ---------- ligações ----------
$("#vp-soundpad").onclick = (e) => { e.stopPropagation(); openSoundpad(e.currentTarget); };
