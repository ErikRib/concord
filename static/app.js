"use strict";

// ---------- utilidades ----------
const $ = (sel) => document.querySelector(sel);

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

function button(label, cls, onClick) {
  const b = el("button", cls, label);
  b.type = "button";
  b.onclick = onClick;
  return b;
}

function load(key, fallback) {
  try {
    const v = localStorage.getItem(key);
    return v ? JSON.parse(v) : fallback;
  } catch {
    return fallback;
  }
}

function store(key, value) {
  try { localStorage.setItem(key, JSON.stringify(value)); } catch {}
}

function clearLocalData() {
  try {
    for (const k of Object.keys(localStorage)) {
      if (k.startsWith("concord.")) localStorage.removeItem(k);
    }
  } catch {}
}

// Segredo de 256 bits que nunca sai deste navegador. O servidor só vê o hash dele.
function newSecret() {
  const b = crypto.getRandomValues(new Uint8Array(32));
  return [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
}

const COLORS = ["#5865f2", "#3ba55c", "#faa61a", "#eb459e", "#ed4245", "#f47b67", "#00b0f4", "#9b59b6", "#1abc9c", "#e67e22"];

const REPORT_CATEGORIES = [
  ["spam", "Spam ou propaganda"],
  ["assedio", "Assédio ou ameaça"],
  ["odio", "Discurso de ódio"],
  ["dados_pessoais", "Exposição de dados pessoais de alguém"],
  ["ilegal", "Conteúdo ilegal"],
  ["outro", "Outro"],
];
const categoryLabel = (c) => REPORT_CATEGORIES.find(([k]) => k === c)?.[1] ?? c;

function initials(name) {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return [...parts[0]].slice(0, 2).join("");
  return [...parts[0]][0] + [...parts[1]][0];
}

function avatar(user, big) {
  const a = el("div", big ? "avatar big" : "avatar", [...user.name][0] || "?");
  a.style.background = user.color;
  return a;
}

function formatTime(ts) {
  const d = new Date(ts);
  const time = d.toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" });
  if (d.toDateString() === new Date().toDateString()) return `Hoje às ${time}`;
  return `${d.toLocaleDateString("pt-BR")} ${time}`;
}

function shortTime(ts) {
  return new Date(ts).toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" });
}

// Transforma URLs em links e <@id> em menções, sem usar innerHTML (evita XSS).
function renderContent(text) {
  const frag = document.createDocumentFragment();
  const re = /(https?:\/\/[^\s<]+)|<@([0-9a-f-]{36})>/g;
  let last = 0;
  for (const m of text.matchAll(re)) {
    frag.append(text.slice(last, m.index));
    if (m[1]) {
      const a = el("a", null, m[1]);
      a.href = m[1];
      a.target = "_blank";
      a.rel = "noopener noreferrer";
      frag.append(a);
    } else {
      const u = typeof userById === "function" ? userById(m[2]) : null;
      const pill = el("span", m[2] === state.me?.id ? "mention me" : "mention", `@${u?.name ?? "desconhecido"}`);
      if (u) pill.onclick = (e) => userMenu(e, u);
      frag.append(pill);
    }
    last = m.index + m[0].length;
  }
  frag.append(text.slice(last));
  return frag;
}

let toastTimer = null;
function toast(msg, ok) {
  const t = $("#toast");
  t.textContent = msg;
  t.style.background = ok ? "var(--green)" : "var(--danger)";
  t.classList.remove("hidden");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.add("hidden"), ok ? 4000 : 3500);
}

function show(sel) { $(sel).classList.remove("hidden"); }
function hide(sel) { $(sel).classList.add("hidden"); }

/**
 * Toque longo (celular): chama `handler` com um evento cujo currentTarget é o elemento.
 * No Android o navegador já gera "contextmenu"; isto cobre o iOS.
 */
function longPress(target, handler, ms = 480) {
  let timer = null;
  let start = null;
  const cancel = () => { clearTimeout(timer); timer = null; };
  target.addEventListener("touchstart", (e) => {
    if (e.touches.length !== 1) return cancel();
    start = { x: e.touches[0].clientX, y: e.touches[0].clientY };
    const t = e.target;
    timer = setTimeout(() => {
      timer = null;
      target.dataset.longPressed = "1";
      setTimeout(() => delete target.dataset.longPressed, 600);
      handler({ target: t, currentTarget: target, preventDefault() {}, stopPropagation() {} });
    }, ms);
  }, { passive: true });
  target.addEventListener("touchmove", (e) => {
    if (start && Math.hypot(e.touches[0].clientX - start.x, e.touches[0].clientY - start.y) > 10) cancel();
  }, { passive: true });
  target.addEventListener("touchend", cancel);
  target.addEventListener("touchcancel", cancel);
  // Evita o clique (e o menu nativo do Android) logo depois do toque longo.
  target.addEventListener("click", (e) => { if (target.dataset.longPressed) { e.stopPropagation(); e.preventDefault(); } }, true);
  target.addEventListener("contextmenu", (e) => { if (target.dataset.longPressed) e.preventDefault(); });
}

async function copyText(text, input) {
  try {
    await navigator.clipboard.writeText(text);
    toast("Copiado!", true);
  } catch {
    // Sem HTTPS a área de transferência pode não estar disponível.
    input?.select();
    toast("Selecione e copie com Ctrl+C");
  }
}

// ---------- diálogo genérico ----------
// fields: [{ name, type: text|textarea|checkbox|select|copy, label, value, placeholder, maxLength, required, options }]
function dialog({ title, desc = "", fields = [], submit = "OK", danger = false, cancel = "Cancelar" }) {
  return new Promise((resolve) => {
    const card = $("#modal-card");
    card.replaceChildren();
    card.append(el("h2", null, title));
    if (desc) card.append(el("p", "muted center", desc));

    const inputs = {};
    for (const f of fields) {
      if (f.type === "checkbox") {
        const label = el("label", "check");
        const cb = el("input");
        cb.type = "checkbox";
        cb.checked = !!f.value;
        label.append(cb, " " + f.label);
        card.append(label);
        inputs[f.name] = cb;
        continue;
      }
      if (f.label) card.append(el("label", "field-label", f.label));
      if (f.type === "select") {
        const sel = el("select", "field");
        for (const [v, l] of f.options) {
          const o = el("option", null, l);
          o.value = v;
          sel.append(o);
        }
        if (f.value) sel.value = f.value;
        card.append(sel);
        inputs[f.name] = sel;
      } else if (f.type === "textarea") {
        const ta = el("textarea", "field");
        ta.maxLength = f.maxLength ?? 500;
        ta.placeholder = f.placeholder ?? "";
        ta.value = f.value ?? "";
        card.append(ta);
        inputs[f.name] = ta;
      } else if (f.type === "key") {
        // Captura a próxima tecla pressionada (ex.: atalho de apertar para falar).
        const inp = el("input", "field");
        inp.type = "text";
        inp.readOnly = true;
        inp.value = f.value ?? "";
        inp.placeholder = "Clique e aperte uma tecla";
        inp.onkeydown = (e) => {
          if (e.key === "Escape" || e.key === "Enter") return;
          e.preventDefault();
          inp.value = e.code;
        };
        card.append(inp);
        inputs[f.name] = inp;
      } else if (f.type === "copy") {
        const row = el("div", "copy-row");
        const inp = el("input", "field");
        inp.type = "text";
        inp.readOnly = true;
        inp.value = f.value;
        row.append(inp, button("Copiar", null, () => copyText(f.value, inp)));
        card.append(row);
      } else {
        const inp = el("input", "field");
        inp.type = "text";
        inp.maxLength = f.maxLength ?? 50;
        inp.placeholder = f.placeholder ?? "";
        inp.value = f.value ?? "";
        inp.autocomplete = "off";
        card.append(inp);
        inputs[f.name] = inp;
      }
    }

    const actions = el("div", "actions");
    const extra = [];
    if (cancel) extra.push(button(cancel, null, () => finish(null)));
    const ok = el("button", danger ? "primary danger" : "primary", submit);
    ok.type = "submit";
    actions.append(...extra, ok);
    card.append(actions);

    show("#modal");
    (card.querySelector("input.field:not([readonly]), textarea, select") || ok).focus();

    function finish(result) {
      hide("#modal");
      card.onsubmit = null;
      $("#modal").onkeydown = null;
      resolve(result);
    }
    card.onsubmit = (e) => {
      e.preventDefault();
      const values = {};
      for (const [k, i] of Object.entries(inputs)) values[k] = i.type === "checkbox" ? i.checked : i.value.trim();
      for (const f of fields) {
        if (f.required && !values[f.name]) {
          inputs[f.name].focus();
          return;
        }
      }
      finish(values);
    };
    $("#modal").onkeydown = (e) => { if (e.key === "Escape") finish(null); };
  });
}

async function askText({ title, desc, placeholder, value, maxLength = 50, submit }) {
  const r = await dialog({ title, desc, submit, fields: [{ name: "v", placeholder, value, maxLength, required: true }] });
  return r?.v || null;
}

async function confirmTyping(title, desc, word) {
  const r = await dialog({ title, desc: `${desc} Digite ${word} para confirmar.`, danger: true, submit: "Confirmar", fields: [{ name: "v", placeholder: word, maxLength: 60 }] });
  return r && r.v.toUpperCase() === word.toUpperCase();
}

// ---------- ícones (traço 2px, 24x24; ver docs/DESIGN.md) ----------
const ICONS = {
  invite: "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2M13 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0M19 8v6M22 11h-6",
  settings: "M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z",
  flag: "M4 15s1-1 4-1 5 2 8 2 4-1 4-1V3s-1 1-4 1-5-2-8-2-4 1-4 1zM4 22v-7",
  ban: "M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0zM4.9 4.9l14.2 14.2",
  leave: "M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4M16 17l5-5-5-5M21 12H9",
  trash: "M3 6h18M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6M10 11v6M14 11v6M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2",
  plus: "M12 5v14M5 12h14",
  link: "M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.7 1.7M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7l1.7-1.7",
  user: "M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2M16 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0",
  shield: "M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z",
  star: "M12 2l3 7h7l-5.5 4.5 2 7.5-6.5-4-6.5 4 2-7.5L2 9h7z",
  volume: "M11 5 6 9H2v6h4l5 4V5zM15.5 8.5a5 5 0 0 1 0 7M19 5a10 10 0 0 1 0 14",
  move: "M5 12h14M12 5l7 7-7 7",
  hangup: "M10.7 13.3a16 16 0 0 0 3 2.3l1.3-1.3a2 2 0 0 1 2.1-.4c.8.3 1.7.5 2.6.6a2 2 0 0 1 1.7 2v3a2 2 0 0 1-2.2 2A19.8 19.8 0 0 1 2.5 4.2 2 2 0 0 1 4.5 2h3a2 2 0 0 1 2 1.7c.1.9.3 1.8.6 2.6a2 2 0 0 1-.4 2.1L8.4 9.7M23 1 1 23",
  edit: "M12 20h9M16.5 3.5a2.1 2.1 0 1 1 3 3L7 19l-4 1 1-4z",
  copy: "M20 9h-9a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h9a2 2 0 0 0 2-2v-9a2 2 0 0 0-2-2zM5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1",
  smile: "M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0zM8 14s1.5 2 4 2 4-2 4-2M9 9h.01M15 9h.01",
  reply: "M9 17l-5-5 5-5M20 18v-2a4 4 0 0 0-4-4H4",
  more: "M12 13a1 1 0 1 0 0-2 1 1 0 0 0 0 2zM19 13a1 1 0 1 0 0-2 1 1 0 0 0 0 2zM5 13a1 1 0 1 0 0-2 1 1 0 0 0 0 2z",
  image: "M21 19V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2zM8.5 10a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3zM21 15l-5-5L5 21",
  file: "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6M8 13h8M8 17h5",
  film: "M4 3h16a1 1 0 0 1 1 1v16a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1zM7 3v18M17 3v18M3 8h4M3 16h4M17 8h4M17 16h4",
  archive: "M21 8v13H3V8M1 3h22v5H1zM10 12h4",
  alert: "M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0zM12 9v4M12 17h.01",
  download: "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3",
  volumeOff: "M11 5 6 9H2v6h4l5 4V5zM23 9l-6 6M17 9l6 6",
};

function icon(name, cls = "mi-icon") {
  const NS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "2");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("class", cls);
  const p = document.createElementNS(NS, "path");
  p.setAttribute("d", ICONS[name] ?? "");
  svg.append(p);
  return svg;
}

/**
 * Barra de volume (0 a `max`%), com botão de mudo e valor ao lado. Aplica enquanto arrasta.
 * Cliques dentro dela não fecham menus nem acionam o bloco de vídeo em volta.
 */
function volumeSlider(value, onInput, { max = 200, compact = false } = {}) {
  const wrap = el("div", compact ? "vol-slider compact" : "vol-slider");
  const mute = el("button", "vol-mute");
  mute.type = "button";
  const range = el("input");
  range.type = "range";
  range.min = "0";
  range.max = String(max);
  range.step = "1";
  range.value = String(Math.round(value * 100));
  range.setAttribute("aria-label", "Volume");
  const label = el("span", "vol-val");
  let lastAudible = value > 0 ? value : 1;
  const paint = () => {
    const v = Number(range.value);
    label.textContent = `${v}%`;
    range.style.setProperty("--fill", `${(v / max) * 100}%`);
    mute.replaceChildren(icon(v === 0 ? "volumeOff" : "volume", "vol-icon"));
    mute.title = v === 0 ? "Ativar som" : "Silenciar";
    mute.setAttribute("aria-label", mute.title);
  };
  range.oninput = () => {
    const v = Number(range.value) / 100;
    if (v > 0) lastAudible = v;
    paint();
    onInput(v);
  };
  mute.onclick = () => {
    range.value = String(Number(range.value) === 0 ? Math.round(lastAudible * 100) : 0);
    range.oninput();
  };
  for (const ev of ["click", "pointerdown", "mousedown", "dblclick", "touchstart"]) {
    wrap.addEventListener(ev, (e) => e.stopPropagation(), { passive: true });
  }
  wrap.append(mute, range, label);
  paint();
  return wrap;
}

/** Pinta a parte preenchida de qualquer controle deslizante (o CSS usa --fill). */
function paintRange(r) {
  const min = Number(r.min) || 0;
  const max = Number(r.max) || 100;
  r.style.setProperty("--fill", `${((Number(r.value) - min) / (max - min || 1)) * 100}%`);
}
document.addEventListener("input", (e) => { if (e.target.type === "range") paintRange(e.target); });
new MutationObserver((list) => {
  for (const m of list) {
    for (const n of m.addedNodes) {
      if (n.nodeType !== 1) continue;
      if (n.matches?.("input[type=range]")) paintRange(n);
      n.querySelectorAll?.("input[type=range]").forEach(paintRange);
    }
  }
}).observe(document.documentElement, { childList: true, subtree: true });

// ---------- menu de contexto ----------
// Item: { label, icon?, danger?, checked? (caixinha à direita), onClick }. "-" = divisória.
// Item com barra: { slider: { label, value (0 a 2), onInput } }.
let openMenu = null;
function closeMenu() {
  openMenu?.remove();
  openMenu = null;
}
document.addEventListener("click", closeMenu);
document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeMenu(); });

function showMenu(event, items) {
  event.stopPropagation();
  closeMenu();
  // Remove divisórias repetidas, no começo ou no fim.
  const list = items.filter(Boolean).filter((it, i, arr) => it !== "-" || (i > 0 && i < arr.length - 1 && arr[i - 1] !== "-"));
  if (list.length === 0) return;
  const menu = el("div", "menu");
  menu.setAttribute("role", "menu");
  for (const it of list) {
    if (it === "-") {
      menu.append(el("div", "menu-sep"));
      continue;
    }
    if (it.slider) {
      const row = el("div", "menu-slider");
      row.append(el("div", "menu-slider-label", it.slider.label), volumeSlider(it.slider.value, it.slider.onInput));
      row.onclick = (ev) => ev.stopPropagation();
      menu.append(row);
      continue;
    }
    const b = el("button", it.danger ? "menu-item danger" : "menu-item");
    b.type = "button";
    b.setAttribute("role", it.checked === undefined ? "menuitem" : "menuitemcheckbox");
    if (it.icon) b.append(icon(it.icon));
    b.append(el("span", "mi-label", it.label));
    if (it.checked !== undefined) {
      b.setAttribute("aria-checked", String(!!it.checked));
      b.append(el("span", it.checked ? "menu-check on" : "menu-check", it.checked ? "✓" : ""));
    }
    b.onclick = () => { closeMenu(); it.onClick(); };
    menu.append(b);
  }
  document.body.append(menu);
  const r = event.currentTarget.getBoundingClientRect();
  const left = Math.min(r.left, window.innerWidth - menu.offsetWidth - 8);
  let top = r.bottom + 4;
  if (top + menu.offsetHeight > window.innerHeight - 8) top = Math.max(8, r.top - menu.offsetHeight - 4);
  menu.style.left = `${Math.max(8, left)}px`;
  menu.style.top = `${top}px`;
  openMenu = menu;
}

// ---------- estado ----------
// Sem conta: tudo fica neste navegador. `consent` = versão da política aceita.
function loadIdentity() {
  const id = load("concord.identity", null);
  if (id && /^[0-9a-f]{64}$/.test(id.secret || "")) return id;
  return {
    secret: newSecret(),
    name: id?.name || "",
    color: COLORS[Math.floor(Math.random() * COLORS.length)],
    consent: 0,
  };
}

let identity = loadIdentity();
const state = {
  me: null, // perfil público devolvido pelo servidor (inclui o id pseudônimo)
  isAdmin: false,
  servers: [],
  serverId: load("concord.server", null),
  channelId: load("concord.channel", null),
  messages: new Map(), // channelId -> [Message]
  hasMore: new Map(), // channelId -> ainda há mensagens antigas no servidor?
  loadingOlder: new Set(), // canais com página antiga a caminho
  online: [],
  typing: new Map(), // channelId -> Map(userId -> { name, until })
  unread: new Set(),
  reports: new Map(), // serverId -> [ReportView]
  voiceStates: new Map(), // serverId -> [VoiceStateView]
  voiceEnabled: false,
  focus: null, // { server_id, channel_id } pedido pelo servidor
  collapsed: new Set(load("concord.collapsed", [])), // categorias recolhidas: "servidor:tipo"
  mentionCounts: new Map(), // channelId -> menções a mim ainda não vistas
};
let ws = null;
let reconnect = true;

// Link de convite: /convite/CODIGO
let pendingInvite = null;
{
  const m = location.pathname.match(/^\/convite\/([A-Za-z0-9]+)\/?$/);
  if (m) {
    pendingInvite = m[1];
    history.replaceState(null, "", "/");
  }
}

function send(obj) {
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(obj));
}

function hello() {
  send({ type: "hello", secret: identity.secret, name: identity.name || "Anônimo", color: identity.color, consent_version: identity.consent || 0 });
}

function currentServer() {
  return state.servers.find((s) => s.id === state.serverId);
}

function currentChannel() {
  return currentServer()?.channels.find((c) => c.id === state.channelId);
}

function roleIn(server, userId) {
  if (!server) return null;
  if (server.owner_id === userId) return "owner";
  if (server.moderators.includes(userId)) return "mod";
  return null;
}

// ---------- conexão ----------
function connect() {
  reconnect = true;
  const proto = location.protocol === "https:" ? "wss" : "ws";
  ws = new WebSocket(`${proto}://${location.host}/ws`);
  ws.onopen = hello;
  ws.onmessage = (e) => handle(JSON.parse(e.data));
  ws.onclose = () => {
    if (!reconnect) return;
    // Som só na primeira queda (não a cada tentativa).
    if (state.me && !state.offline) playSfx("disconnect");
    state.offline = true;
    setStatus("Sem conexão com o servidor — tentando reconectar…");
    setTimeout(connect, 1500);
  };
}

function setStatus(text) {
  const s = $("#status");
  s.textContent = text || "";
  s.classList.toggle("hidden", !text);
}

function upsertServer(server) {
  const i = state.servers.findIndex((s) => s.id === server.id);
  if (i >= 0) state.servers[i] = server;
  else state.servers.push(server);
}

function handle(m) {
  switch (m.type) {
    case "consent_required":
      // Nada é gravado no servidor antes do aceite.
      reconnect = false;
      askConsent(m.version, m.retention_days);
      break;

    case "ready":
      state.me = m.me;
      state.isAdmin = m.is_admin;
      state.voiceEnabled = m.voice_enabled;
      state.voiceStates.clear();
      // O chat reconectou com a chamada ainda ativa: reafirma a presença na voz.
      if (typeof voice !== "undefined" && voice.room && voice.channelId) {
        send({ type: "voice_join", channel_id: voice.channelId });
      }
      state.servers = m.servers;
      state.online = m.online;
      state.messages.clear();
      state.hasMore.clear();
      state.loadingOlder.clear();
      state.reports.clear();
      setStatus(null);
      if (state.offline) playSfx("reconnect");
      state.offline = false;
      sp.lists.clear();
      for (const s of state.servers) if (s.can_moderate) send({ type: "fetch_reports", server_id: s.id });
      if (pendingInvite) {
        send({ type: "join_server", code: pendingInvite });
        pendingInvite = null;
      }
      refresh();
      break;

    case "server_updated": {
      upsertServer(m.server);
      if (!applyFocus()) refresh();
      refreshSettings(m.server.id);
      break;
    }

    case "server_removed": {
      const had = state.servers.some((s) => s.id === m.server_id);
      state.servers = state.servers.filter((s) => s.id !== m.server_id);
      state.reports.delete(m.server_id);
      if (had) refresh();
      refreshSettings(m.server_id);
      break;
    }

    case "focus":
      state.focus = { server_id: m.server_id, channel_id: m.channel_id };
      applyFocus();
      break;

    case "message": {
      const msg = m.message;
      const list = state.messages.get(msg.channel_id);
      if (list) list.push(msg);
      state.typing.get(msg.channel_id)?.delete(msg.author.id);
      if (msg.author.id !== state.me?.id && (mentionsMe(msg) || msg.reply_to?.author?.id === state.me?.id)) playSfx("notify");
      if (msg.channel_id === state.channelId) {
        renderMessages(msg.author.id === state.me?.id ? "bottom" : "auto");
        renderTyping();
      } else {
        state.unread.add(msg.channel_id);
        // Menção a mim em outro canal: contador vermelho no canal e no servidor.
        if (msg.author.id !== state.me?.id && mentionsMe(msg)) {
          state.mentionCounts.set(msg.channel_id, (state.mentionCounts.get(msg.channel_id) ?? 0) + 1);
          renderServers();
        }
        renderChannels();
      }
      break;
    }

    case "message_edited": {
      const msg = state.messages.get(m.channel_id)?.find((x) => x.id === m.message_id);
      if (!msg) break;
      msg.content = m.content;
      msg.edited_at = m.edited_at;
      if (m.channel_id === state.channelId) renderMessages("keep");
      break;
    }

    case "reaction_update": {
      const msg = state.messages.get(m.channel_id)?.find((x) => x.id === m.message_id);
      if (!msg) break;
      let r = msg.reactions.find((x) => x.emoji === m.emoji);
      if (m.on) {
        if (!r) msg.reactions.push((r = { emoji: m.emoji, users: [] }));
        if (!r.users.includes(m.user_id)) r.users.push(m.user_id);
      } else if (r) {
        r.users = r.users.filter((u) => u !== m.user_id);
        if (r.users.length === 0) msg.reactions = msg.reactions.filter((x) => x !== r);
      }
      if (m.channel_id === state.channelId) renderMessages("keep");
      break;
    }

    case "upload_token":
      if (m.kind === "sound") onSoundUploadToken(m);
      else onUploadToken(m);
      break;

    case "sounds":
      onSounds(m);
      break;

    case "sounds_changed":
      onSoundsChanged(m);
      break;

    case "sound_played":
      onSoundPlayed(m);
      break;

    case "message_deleted": {
      const list = state.messages.get(m.channel_id);
      if (!list) break;
      const i = list.findIndex((x) => x.id === m.message_id);
      if (i >= 0) list.splice(i, 1);
      if (m.channel_id === state.channelId) renderMessages("keep");
      break;
    }

    case "messages_purged": {
      const server = state.servers.find((s) => s.id === m.server_id);
      for (const c of server?.channels ?? []) {
        const list = state.messages.get(c.id);
        if (list) state.messages.set(c.id, list.filter((x) => x.author.id !== m.user_id));
      }
      renderMessages("keep");
      break;
    }

    case "user_purged":
      purgeUser(m.user_id);
      break;

    case "profile_updated":
      for (const list of state.messages.values()) {
        for (const msg of list) {
          if (msg.author.id === m.user.id) msg.author = m.user;
        }
      }
      for (const s of state.servers) {
        const i = s.members?.findIndex((u) => u.id === m.user.id) ?? -1;
        if (i >= 0) s.members[i] = m.user;
      }
      renderMessages("keep");
      renderMembers();
      break;

    case "history": {
      state.hasMore.set(m.channel_id, m.has_more);
      state.loadingOlder.delete(m.channel_id);
      if (m.before) {
        // Página antiga: entra no começo da lista.
        const list = state.messages.get(m.channel_id) ?? [];
        state.messages.set(m.channel_id, [...m.messages, ...list]);
        if (m.channel_id === state.channelId) renderMessages("prepend");
      } else {
        state.messages.set(m.channel_id, m.messages);
        if (m.channel_id === state.channelId) renderMessages("bottom");
      }
      break;
    }

    case "presence":
      state.online = m.online;
      renderMembers();
      break;

    case "typing": {
      if (m.user.id === state.me?.id) break;
      if (!state.typing.has(m.channel_id)) state.typing.set(m.channel_id, new Map());
      state.typing.get(m.channel_id).set(m.user.id, { name: m.user.name, until: Date.now() + 4000 });
      if (m.channel_id === state.channelId) renderTyping();
      break;
    }

    case "reports":
      state.reports.set(m.server_id, m.reports);
      renderServers();
      refreshSettings(m.server_id);
      break;

    case "my_data":
      downloadJson(m.data);
      break;

    case "data_deleted":
      reconnect = false;
      clearLocalData();
      disconnectRoom();
      // A conexão é encerrada logo em seguida; limpa a própria tela aqui.
      if (state.me) purgeUser(state.me.id);
      state.online = [];
      renderMembers();
      closeSettings();
      $("#goodbye-text").textContent =
        `Seus dados foram eliminados do servidor e deste navegador (${m.messages_removed} mensagem(ns) removida(s)).`;
      show("#goodbye");
      break;

    case "voice_token":
      onVoiceToken(m);
      break;

    case "voice_states":
      state.voiceStates.set(m.server_id, m.states);
      onVoiceStatesChanged();
      renderChannels();
      renderVoiceView();
      break;

    case "voice_ended":
      onVoiceEnded(m);
      break;

    case "info":
      toast(m.message, true);
      break;

    case "error":
      if (typeof voice !== "undefined" && voice.connecting) {
        voice.connecting = false;
        renderVoiceAll();
      }
      toast(m.message);
      break;
  }
}

function purgeUser(userId) {
  for (const [id, list] of state.messages) {
    state.messages.set(id, list.filter((x) => x.author.id !== userId));
  }
  renderMessages("keep");
}

/** Abre o servidor/canal sugerido pelo servidor, se já estiver disponível. */
function applyFocus() {
  const f = state.focus;
  if (!f) return false;
  const server = state.servers.find((s) => s.id === f.server_id);
  if (!server || (f.channel_id && !server.channels.some((c) => c.id === f.channel_id))) return false;
  state.focus = null;
  state.serverId = server.id;
  state.channelId = f.channel_id ?? null;
  refresh(true);
  return true;
}

// ---------- consentimento (LGPD art. 8º) ----------
function askConsent(version, retentionDays) {
  const name = $("#consent-name");
  const policy = $("#consent-policy");
  const age = $("#consent-age");
  const ok = $("#consent-ok");
  name.value = identity.name || "";
  policy.checked = false;
  age.checked = false;
  $("#retention-days").textContent = retentionDays;
  if (identity.consent && identity.consent < version) {
    $("#consent-intro").textContent = "A Política de Privacidade foi atualizada. Leia e aceite de novo para continuar.";
  }

  // Aceite precisa ser ativo: nada vem marcado por padrão.
  const update = () => { ok.disabled = !(policy.checked && age.checked && name.value.trim()); };
  policy.onchange = age.onchange = name.oninput = update;
  update();

  show("#consent");
  name.focus();
  $("#consent-card").onsubmit = (e) => {
    e.preventDefault();
    if (ok.disabled) return;
    identity.name = name.value.trim();
    identity.consent = version;
    store("concord.identity", identity);
    hide("#consent");
    renderMe();
    connect();
  };
}

// ---------- direitos do titular (LGPD art. 18) ----------
function downloadJson(data) {
  const blob = new Blob([JSON.stringify(data, null, 2)], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const a = el("a");
  a.href = url;
  a.download = `concord-meus-dados-${new Date().toISOString().slice(0, 10)}.json`;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

async function rename() {
  const name = await askText({ title: "Mudar apelido", value: identity.name, maxLength: 32 });
  if (!name) return;
  identity.name = name;
  store("concord.identity", identity);
  renderMe();
  hello();
}

async function deleteMyData() {
  if (await confirmTyping("Apagar meus dados", "Todas as suas mensagens e seu registro serão eliminados.", "APAGAR")) {
    send({ type: "delete_my_data" });
  }
}
$("#goodbye-restart").onclick = () => location.reload();

// ---------- tela de configurações (docs/DESIGN.md › Tela de configurações) ----------
const settingsState = { kind: null, section: null, serverId: null };

function openSettings(kind, section, serverId = null) {
  closeMenu();
  Object.assign(settingsState, { kind, section, serverId });
  renderSettings();
  show("#settings");
}

function closeSettings() {
  settingsState.kind = null;
  hide("#settings");
}

document.addEventListener("keydown", (e) => {
  // Esc fecha a tela, a menos que um diálogo esteja aberto por cima.
  if (e.key === "Escape" && settingsState.kind && $("#modal").classList.contains("hidden")) closeSettings();
});

/** Redesenha a tela se ela estiver mostrando este servidor (ou a do usuário). */
function refreshSettings(serverId) {
  if (!settingsState.kind) return;
  if (settingsState.kind === "server" && serverId && settingsState.serverId !== serverId) return;
  renderSettings();
}

function renderSettings() {
  const def = settingsState.kind === "server" ? serverSettingsDef() : userSettingsDef();
  if (!def) return closeSettings();
  const items = def.groups.flatMap((g) => g.items.filter(Boolean));
  let current = items.find((i) => i.id === settingsState.section && i.render);
  if (!current) {
    current = items.find((i) => i.render);
    settingsState.section = current.id;
  }

  const root = $("#settings");
  root.replaceChildren();
  const navWrap = el("div", "settings-nav");
  const nav = el("nav");
  nav.setAttribute("aria-label", def.title);
  for (const g of def.groups) {
    const groupItems = g.items.filter(Boolean);
    if (groupItems.length === 0) continue;
    nav.append(el("div", "nav-label", g.label));
    for (const it of groupItems) {
      const cls = [it === current ? "active" : "", it.danger ? "danger" : ""].join(" ").trim() || null;
      const b = button(it.label, cls, () => {
        if (it.action) return it.action();
        settingsState.section = it.id;
        renderSettings();
      });
      if (it.count) b.append(el("span", "badge", String(it.count)));
      nav.append(b);
    }
  }
  navWrap.append(nav);

  const content = el("div", "settings-content");
  current.render(content);

  const close = el("div", "settings-close");
  const x = button("✕", null, closeSettings);
  x.title = "Fechar (Esc)";
  x.setAttribute("aria-label", "Fechar configurações");
  close.append(x, el("span", null, "ESC"));
  root.append(navWrap, content, close);
}

function settingsHeader(c, title, lead) {
  c.append(el("h1", null, title));
  if (lead) c.append(el("p", "lead", lead));
}

function settingsSection(c, title, help) {
  const s = el("section");
  if (title) s.append(el("h3", null, title));
  if (help) s.append(el("p", "help", help));
  c.append(s);
  return s;
}

function textField(value, maxLength, placeholder = "") {
  const inp = el("input", "field");
  inp.type = "text";
  inp.value = value ?? "";
  inp.maxLength = maxLength;
  inp.placeholder = placeholder;
  return inp;
}

function switchToggle(checked, onChange) {
  const label = el("label", "switch");
  const inp = el("input");
  inp.type = "checkbox";
  inp.checked = checked;
  inp.onchange = () => onChange(inp.checked);
  label.append(inp, el("span"));
  return label;
}

// ---- configurações do usuário ----
function userSettingsDef() {
  return {
    title: "Configurações do usuário",
    groups: [
      { label: "Configurações do usuário", items: [
        { id: "account", label: "Minha conta", render: renderAccountSettings },
        { id: "voice", label: "Voz e vídeo", render: renderVoiceSettingsSection },
        { id: "sounds", label: "Sons", render: renderSoundSettings },
      ] },
      { label: "Privacidade", items: [
        { id: "privacy", label: "Privacidade e dados", render: renderPrivacySettings },
      ] },
    ],
  };
}

function profilePreview(name, color) {
  const card = el("div", "preview-card");
  const banner = el("div", "banner");
  banner.style.background = `linear-gradient(180deg, ${color}, #1f1f24)`;
  const body = el("div", "pc-body");
  const av = el("div", "pc-icon round", [...(name || "?")][0]);
  av.style.background = color;
  av.style.color = "#fff";
  const meta = el("div", "pc-meta");
  meta.append(el("span", "dot on", "Online"));
  body.append(av, el("h4", null, name || "…"), meta);
  card.append(banner, body);
  return card;
}

function renderAccountSettings(c) {
  settingsHeader(c, "Minha conta", "Sem cadastro: seu perfil é só um apelido e uma cor, guardados neste navegador e no servidor (LGPD, art. 18, III: você pode corrigi-los quando quiser).");
  const split = el("div", "settings-split");
  const form = el("div");
  let name = identity.name;
  let color = identity.color;
  const preview = el("div");
  const updatePreview = () => preview.replaceChildren(profilePreview(name, color));

  const s1 = settingsSection(form, "Apelido", "Como você aparece para os outros, inclusive nas mensagens antigas.");
  const nameInput = textField(name, 32, "Seu apelido");
  nameInput.oninput = () => { name = nameInput.value.trim(); updatePreview(); };
  s1.append(nameInput);

  const s2 = settingsSection(form, "Cor", "Cor do seu nome e do seu avatar.");
  const sw = el("div", "swatches");
  for (const col of COLORS) {
    const b = el("button", col === color ? "swatch on" : "swatch");
    b.type = "button";
    b.style.background = col;
    b.title = col;
    b.setAttribute("aria-label", `Cor ${col}`);
    b.onclick = () => {
      color = col;
      sw.querySelectorAll(".swatch").forEach((x) => x.classList.toggle("on", x === b));
      updatePreview();
    };
    sw.append(b);
  }
  s2.append(sw);

  const s3 = settingsSection(form, "Seu id pseudônimo", "Gerado a partir de um segredo que só existe neste navegador. Serve, por exemplo, para virar administrador da instância (CONCORD_ADMINS).");
  const idRow = el("div", "copy-row");
  const idInput = textField(state.me?.id ?? "", 64);
  idInput.readOnly = true;
  idRow.append(idInput, button("Copiar", null, () => copyText(idInput.value, idInput)));
  s3.append(idRow);

  const save = el("section");
  const row = el("div", "row");
  row.append(button("Salvar alterações", "btn primary", () => {
    if (!name) return toast("O apelido não pode ficar vazio");
    identity.name = name;
    identity.color = color;
    store("concord.identity", identity);
    renderMe();
    hello();
    toast("Perfil atualizado", true);
  }));
  save.append(row);
  form.append(save);

  updatePreview();
  split.append(form, preview);
  c.append(split);
}

function renderVoiceSettingsSection(c) {
  settingsHeader(c, "Voz e vídeo", "Preferências de microfone, saída de áudio e transmissão. Ficam só neste navegador.");
  const s = settingsSection(c, "Resumo");
  const modeText = vset.mode === "ptt" ? `Apertar para falar (tecla ${vset.pttKey})` : "Ativação por voz";
  const list = el("div", "stack");
  list.append(
    el("div", null, `Modo de entrada: ${modeText}`),
    el("div", null, `Supressão de ruído: ${vset.noise ? "ligada" : "desligada"} · Cancelamento de eco: ${vset.echo ? "ligado" : "desligado"}`),
    el("div", null, `Codec de vídeo: ${vset.codec === "auto" ? "automático (GPU)" : vset.codec.toUpperCase()} · Qualidade preferida da tela: ${vset.preset}`),
  );
  s.append(list);
  const s2 = settingsSection(c);
  s2.append(button("Alterar dispositivos e preferências", "btn primary", async () => {
    await voiceSettings();
    renderSettings();
  }));
}

function renderPrivacySettings(c) {
  settingsHeader(c, "Privacidade e dados", "Seus direitos como titular (LGPD, art. 18), sem precisar pedir a ninguém.");
  const s = settingsSection(c);
  const stack = el("div", "stack");
  const row = (title, desc, onClick, danger) => {
    const b = el("button", danger ? "row-btn danger" : "row-btn");
    b.type = "button";
    b.append(el("strong", null, title), el("span", null, desc));
    b.onclick = onClick;
    return b;
  };
  const policy = el("a", "row-btn");
  policy.href = "/privacidade";
  policy.target = "_blank";
  policy.rel = "noopener";
  policy.append(el("strong", null, "Política de Privacidade"), el("span", null, "O que coletamos, por quê e por quanto tempo."));
  stack.append(
    row("Baixar meus dados", "Arquivo JSON com perfil, aceite, servidores, mensagens, denúncias, moderação e status de voz.", () => send({ type: "export_my_data" })),
    policy,
    row("Apagar meus dados e sair", "Remove suas mensagens e revoga o consentimento. Servidores seus passam para um moderador. Não pode ser desfeito.", deleteMyData, true),
  );
  s.append(stack);
}

// ---- configurações do servidor ----
function serverSettingsDef() {
  const s = state.servers.find((x) => x.id === settingsState.serverId);
  if (!s?.can_moderate) return null;
  const reports = state.reports.get(s.id)?.length ?? 0;
  return {
    title: `Configurações de ${s.name}`,
    groups: [
      { label: s.name, items: [
        s.can_manage && { id: "profile", label: "Perfil do servidor", render: (c) => renderServerProfile(c, s) },
        { id: "invites", label: "Convites", render: (c) => renderInvites(c, s) },
      ] },
      { label: "Moderação", items: [
        { id: "reports", label: "Denúncias", count: reports, render: (c) => renderReports(c, s) },
        { id: "bans", label: "Banidos", count: s.bans?.length ?? 0, render: (c) => renderBans(c, s) },
      ] },
      { label: "Zona de perigo", items: [
        s.can_manage && { id: "delete", label: "Apagar servidor", danger: true, action: () => deleteServer(s) },
      ] },
    ],
  };
}

function serverPreview(s, name, isPublic) {
  const card = el("div", "preview-card");
  const body = el("div", "pc-body");
  const onlineIds = new Set(state.online.map((u) => u.id));
  const online = s.members ? s.members.filter((u) => onlineIds.has(u.id)).length : state.online.length;
  const meta = el("div", "pc-meta");
  meta.append(el("span", "dot on", `${online} online`));
  if (s.members) meta.append(el("span", "dot", `${s.members.length} membros`));
  body.append(el("div", "pc-icon", initials(name || "?")), el("h4", null, name || "…"), meta, el("p", "muted", isPublic ? "Público: qualquer pessoa desta instância pode entrar." : "Privado: só com convite."));
  card.append(el("div", "banner"), body);
  return card;
}

function renderServerProfile(c, s) {
  settingsHeader(c, "Perfil do servidor", "Como o servidor aparece para quem participa e nos convites.");
  const split = el("div", "settings-split");
  const form = el("div");
  let name = s.name;
  let isPublic = s.public;
  const preview = el("div");
  const update = () => preview.replaceChildren(serverPreview(s, name, isPublic));

  const s1 = settingsSection(form, "Nome");
  const inp = textField(name, 50);
  inp.oninput = () => { name = inp.value.trim(); update(); };
  s1.append(inp);

  const s2 = settingsSection(form);
  const tr = el("div", "toggle-row");
  const txt = el("div");
  txt.append(el("h3", null, "Servidor público"), el("p", "help", "Ligado: qualquer pessoa desta instância vê e participa. Desligado: só entra quem tiver o convite (padrão, privacidade por padrão)."));
  tr.append(txt, switchToggle(isPublic, (v) => { isPublic = v; update(); }));
  s2.append(tr);

  const s3 = el("section");
  s3.append(button("Salvar alterações", "btn primary", () => {
    if (!name) return toast("O nome não pode ficar vazio");
    send({ type: "update_server", server_id: s.id, name, public: isPublic });
    toast("Alterações enviadas", true);
  }));
  form.append(s3);
  update();
  split.append(form, preview);
  c.append(split);
}

function renderInvites(c, s) {
  settingsHeader(c, "Convites", s.public ? "Este servidor é público, mas o link leva direto para ele." : "Quem tiver o link pode entrar. Gerar um novo invalida o atual.");
  const sec = settingsSection(c, "Link de convite");
  const row = el("div", "copy-row");
  const inp = textField(inviteLink(s), 300);
  inp.readOnly = true;
  row.append(inp, button("Copiar", null, () => copyText(inp.value, inp)));
  sec.append(row);
  const sec2 = settingsSection(c, "Gerar novo link", "Use se o link vazou para quem não deveria entrar.");
  sec2.append(button("Gerar novo link", "btn secondary", () => send({ type: "regenerate_invite", server_id: s.id })));
}

function renderReports(c, s) {
  settingsHeader(c, "Denúncias", "Os moderadores veem a mensagem e o motivo, mas nunca quem denunciou.");
  const reports = state.reports.get(s.id) ?? [];
  const sec = settingsSection(c);
  if (reports.length === 0) {
    sec.append(el("div", "empty-note", "Nenhuma denúncia pendente."));
    return;
  }
  const stack = el("div", "stack");
  for (const r of reports) {
    const item = el("div", "item");
    const head = el("div", "item-head");
    head.append(el("strong", null, categoryLabel(r.category)), el("span", null, `#${r.channel_name} · ${formatTime(r.created_at)}`));
    const quote = el("div", "item-quote");
    const who = el("strong", null, r.message.author.name + ": ");
    who.style.color = r.message.author.color;
    quote.append(who, r.message.content);
    item.append(head, quote);
    if (r.details) item.append(el("div", "muted", `Detalhes: ${r.details}`));
    const actions = el("div", "item-actions");
    actions.append(
      button("Apagar mensagem", "danger", () => send({ type: "resolve_report", report_id: r.id, delete_message: true })),
      button("Banir autor", null, () => banUser(s, r.message.author)),
      button("Descartar denúncia", null, () => send({ type: "resolve_report", report_id: r.id, delete_message: false })),
    );
    item.append(actions);
    stack.append(item);
  }
  sec.append(stack);
}

function renderBans(c, s) {
  settingsHeader(c, "Banidos", "Banimentos expiram com o prazo de retenção. A pessoa banida recebeu o motivo.");
  const bans = s.bans ?? [];
  const sec = settingsSection(c);
  if (bans.length === 0) {
    sec.append(el("div", "empty-note", "Ninguém banido."));
    return;
  }
  const stack = el("div", "stack");
  for (const b of bans) {
    const item = el("div", "item");
    const head = el("div", "item-head");
    head.append(el("strong", null, b.name), el("span", null, formatTime(b.at)));
    item.append(head, el("div", null, `Motivo: ${b.reason}`));
    const actions = el("div", "item-actions");
    actions.append(button("Revogar banimento", null, () => send({ type: "unban_user", server_id: s.id, user_id: b.user_id })));
    item.append(actions);
    stack.append(item);
  }
  sec.append(stack);
}

// ---------- servidores: criar, entrar, gerenciar ----------
async function createServer() {
  const r = await dialog({
    title: "Criar servidor",
    desc: "Você será dono(a) e poderá nomear moderadores.",
    submit: "Criar",
    fields: [
      { name: "name", label: "Nome do servidor", placeholder: "Meu servidor", required: true },
      { name: "public", type: "checkbox", label: "Público: qualquer pessoa desta instância pode ver e participar. Desmarcado, só entra quem tiver o convite." },
    ],
  });
  if (r) send({ type: "create_server", name: r.name, public: r.public });
}

async function joinServer() {
  const code = await askText({ title: "Entrar com convite", desc: "Cole o link ou o código do convite.", placeholder: "https://…/convite/AbC123…", maxLength: 200, submit: "Entrar" });
  if (!code) return;
  const m = code.match(/([A-Za-z0-9]{6,})\/?$/);
  if (!m) return toast("Convite inválido");
  send({ type: "join_server", code: m[1] });
}

function inviteLink(server) {
  return `${location.origin}/convite/${server.invite_code}`;
}

async function showInvite(server) {
  const r = await dialog({
    title: `Convidar para ${server.name}`,
    desc: server.public
      ? "Este servidor é público, mas o link leva direto para ele."
      : "Quem tiver este link pode entrar. Gere um novo para invalidar o atual.",
    fields: [{ type: "copy", value: inviteLink(server) }],
    submit: "Gerar novo link",
    cancel: "Fechar",
  });
  if (r) send({ type: "regenerate_invite", server_id: server.id });
}

async function deleteServer(server) {
  if (await confirmTyping(`Apagar "${server.name}"`, "Todos os canais e mensagens serão apagados para sempre.", server.name)) {
    send({ type: "delete_server", server_id: server.id });
  }
}

async function leaveServer(server) {
  const r = await dialog({ title: `Sair de "${server.name}"`, desc: server.public ? "Você continuará vendo o servidor, por ser público, mas deixará de ser membro." : "Para voltar, você precisará de um novo convite.", submit: "Sair", danger: true });
  if (r) send({ type: "leave_server", server_id: server.id });
}

$("#add-server").onclick = (e) =>
  showMenu(e, [
    { label: "Criar servidor", icon: "plus", onClick: createServer },
    { label: "Entrar com convite", icon: "link", onClick: joinServer },
  ]);

$("#server-header").onclick = (e) => {
  const s = currentServer();
  if (!s) return;
  const reports = state.reports.get(s.id)?.length ?? 0;
  const member = s.role === "member" || s.role === "moderator";
  showMenu(e, [
    s.can_moderate && { label: "Convidar pessoas", icon: "invite", onClick: () => showInvite(s) },
    s.can_moderate && { label: "Configurações do servidor", icon: "settings", onClick: () => openSettings("server", s.can_manage ? "profile" : "invites", s.id) },
    s.can_moderate && { label: "Criar canal de texto", icon: "plus", onClick: () => createChannel(s, "text") },
    s.can_moderate && { label: "Criar canal de voz", icon: "volume", onClick: () => createChannel(s, "voice") },
    "-",
    s.can_moderate && { label: reports ? `Denúncias (${reports})` : "Denúncias", icon: "flag", onClick: () => openSettings("server", "reports", s.id) },
    s.can_moderate && { label: "Banidos", icon: "ban", onClick: () => openSettings("server", "bans", s.id) },
    "-",
    { label: "Configurações do usuário", icon: "user", onClick: () => openSettings("user", "account") },
    "-",
    member && { label: "Sair do servidor", icon: "leave", danger: true, onClick: () => leaveServer(s) },
    s.can_manage && { label: "Apagar servidor", icon: "trash", danger: true, onClick: () => deleteServer(s) },
  ]);
};

// ---------- moderação ----------
function userMenu(e, user) {
  const s = currentServer();
  if (!s || !state.me) return;
  const isMe = user.id === state.me.id;
  const role = roleIn(s, user.id);
  const inMyCall = !isMe && voice.room?.getParticipantByIdentity(user.id);
  const canAct = !isMe && s.can_moderate && role !== "owner" && (role !== "mod" || s.can_manage);
  showMenu(e, [
    isMe && { label: "Minha conta", icon: "user", onClick: () => openSettings("user", "account") },
    inMyCall && { slider: { label: "Volume do usuário", value: vset.volumes[user.id] ?? 1, onInput: (v) => setUserVolume(user.id, v) } },
    inMyCall && voice.watching.has(user.id) && {
      slider: { label: "Volume da transmissão", value: vset.streamVolumes[user.id] ?? 1, onInput: (v) => setStreamVolume(user.id, v) },
    },
    inMyCall && "-",
    !isMe && s.can_manage && role !== "owner" && {
      label: "Moderador(a)",
      icon: "star",
      checked: role === "mod",
      onClick: () => send({ type: "set_moderator", server_id: s.id, user_id: user.id, enabled: role !== "mod" }),
    },
    ...(canAct ? voiceModerationItems(s, user) : []),
    canAct && "-",
    canAct && { label: `Banir ${user.name}`, icon: "ban", danger: true, onClick: () => banUser(s, user) },
  ]);
}

async function banUser(server, user) {
  const r = await dialog({
    title: `Banir ${user.name}`,
    desc: `A pessoa sai de "${server.name}" e é avisada do motivo.`,
    submit: "Banir",
    danger: true,
    fields: [
      { name: "reason", type: "textarea", label: "Motivo (obrigatório)", maxLength: 200, required: true, placeholder: "Ex.: spam repetido no #geral" },
      { name: "delete", type: "checkbox", label: "Apagar também todas as mensagens dela neste servidor" },
    ],
  });
  if (r) send({ type: "ban_user", server_id: server.id, user_id: user.id, reason: r.reason, delete_messages: r.delete });
}

async function reportMessage(m) {
  const r = await dialog({
    title: "Denunciar mensagem",
    desc: "Os moderadores verão a mensagem e o motivo, mas não quem denunciou.",
    submit: "Enviar denúncia",
    fields: [
      { name: "category", type: "select", label: "Motivo", options: REPORT_CATEGORIES, value: "spam" },
      { name: "details", type: "textarea", label: "Detalhes (opcional)", maxLength: 500, placeholder: "Contexto que ajude a moderação" },
    ],
  });
  if (r) send({ type: "report_message", channel_id: m.channel_id, message_id: m.id, category: r.category, details: r.details });
}

async function deleteChannel(server, channel) {
  const r = await dialog({ title: `Apagar #${channel.name}`, desc: "O canal e todas as mensagens dele serão apagados para sempre.", submit: "Apagar", danger: true });
  if (r) send({ type: "delete_channel", channel_id: channel.id });
}

// ---------- navegação ----------
function ensureSelection() {
  if (!currentServer()) state.serverId = state.servers[0]?.id ?? null;
  const server = currentServer();
  if (server && !currentChannel()) {
    const last = load(`concord.lastChannel.${server.id}`, null);
    const firstText = server.channels.find((c) => c.kind !== "voice") ?? server.channels[0];
    state.channelId = server.channels.some((c) => c.id === last) ? last : firstText?.id ?? null;
  }
  if (!server) state.channelId = null;
  if (state.channelId && currentChannel()?.kind !== "voice" && !state.messages.has(state.channelId)) {
    send({ type: "fetch_history", channel_id: state.channelId });
  }
  store("concord.server", state.serverId);
  store("concord.channel", state.channelId);
}

/** Re-renderiza tudo; só volta ao fim das mensagens se o canal mudou. */
let renderedChannel = null;
function refresh(forceBottom) {
  ensureSelection();
  renderServers();
  renderChannels();
  renderMembers();
  renderMe();
  const changed = renderedChannel !== state.channelId;
  renderedChannel = state.channelId;
  renderChat(changed || forceBottom ? "bottom" : "keep");
}

function selectServer(id) {
  state.serverId = id;
  state.channelId = null;
  refresh();
}

function selectChannel(id) {
  state.channelId = id;
  state.unread.delete(id);
  state.mentionCounts.delete(id);
  clearComposer(); // resposta e imagens pendentes pertencem ao canal anterior
  store(`concord.lastChannel.${state.serverId}`, id);
  refresh();
  // Como no Discord: clicar num canal de voz já entra na chamada.
  if (currentChannel()?.kind === "voice") voiceJoin(id);
  else if (!isMobile()) $("#input").focus(); // no celular, não abre o teclado sozinho
  setDrawer(null);
}

// ---------- celular: gavetas (docs/DESIGN.md › Celular) ----------
const mobileQuery = window.matchMedia("(max-width: 760px)");
const narrowQuery = window.matchMedia("(max-width: 1000px)");
const isMobile = () => mobileQuery.matches;

/** "nav" (servidores e canais), "members" ou null. Só vale em telas estreitas. */
function setDrawer(which) {
  if (which === "nav" && !isMobile()) which = null;
  if (which === "members" && !narrowQuery.matches) which = null;
  document.body.classList.toggle("nav-open", which === "nav");
  document.body.classList.toggle("members-open", which === "members");
}

$("#nav-toggle").onclick = () => setDrawer(document.body.classList.contains("nav-open") ? null : "nav");
$("#members-toggle").onclick = () => {
  if (narrowQuery.matches) return setDrawer(document.body.classList.contains("members-open") ? null : "members");
  // Tela larga: mostra/esconde a coluna, como no Discord.
  const hidden = document.body.classList.toggle("members-hidden");
  store("concord.membersHidden", hidden);
};
document.body.classList.toggle("members-hidden", load("concord.membersHidden", false));
$("#scrim").onclick = () => setDrawer(null);
mobileQuery.addEventListener("change", () => setDrawer(null));
narrowQuery.addEventListener("change", () => setDrawer(null));
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && (document.body.classList.contains("nav-open") || document.body.classList.contains("members-open"))) setDrawer(null);
});

// Deslizar: da esquerda abre os canais; para a esquerda abre os membros (ou fecha a gaveta).
{
  let sx = 0, sy = 0, tracking = false;
  $("#app").addEventListener("touchstart", (e) => {
    tracking = narrowQuery.matches && e.touches.length === 1
      && !e.target.closest("input, textarea, .upload-chips, .vv-grid, .attachments, .menu, .soundpad");
    sx = e.touches[0].clientX;
    sy = e.touches[0].clientY;
  }, { passive: true });
  $("#app").addEventListener("touchend", (e) => {
    if (!tracking) return;
    tracking = false;
    const dx = e.changedTouches[0].clientX - sx;
    const dy = e.changedTouches[0].clientY - sy;
    if (Math.abs(dx) < 70 || Math.abs(dy) > Math.abs(dx) * 0.6) return;
    const body = document.body.classList;
    if (dx > 0) setDrawer(body.contains("members-open") ? null : "nav");
    else setDrawer(body.contains("nav-open") ? null : "members");
  }, { passive: true });
}

// ---------- renderização ----------
function renderServers() {
  const list = $("#server-list");
  list.replaceChildren();
  for (const s of state.servers) {
    const b = el("button", "server-icon" + (s.id === state.serverId ? " active" : ""), initials(s.name));
    const pending = s.can_moderate ? state.reports.get(s.id)?.length ?? 0 : 0;
    b.title = s.name + (s.public ? " (público)" : " (privado)") + (pending ? ` — ${pending} denúncia(s)` : "");
    const mentions = s.channels.reduce((n, c) => n + (state.mentionCounts.get(c.id) ?? 0), 0);
    if (mentions) b.append(el("span", "ping", String(mentions)));
    else if (!s.public) b.append(el("span", "lock", "🔒"));
    b.onclick = () => selectServer(s.id);
    list.append(b);
  }
  const totalMentions = [...state.mentionCounts.values()].reduce((a, b) => a + b, 0);
  $("#nav-ping").textContent = String(totalMentions);
  $("#nav-ping").classList.toggle("hidden", totalMentions === 0);
}

const ICON_MIC_OFF = "🔇";
const ICON_DEAF = "🎧";

function renderChannels() {
  const server = currentServer();
  $("#server-name").textContent = server?.name ?? "Nenhum servidor";
  const box = $("#channels");
  box.replaceChildren();
  if (!server) return;

  // Categorias recolhíveis; recolhida, mostra só o canal aberto (e quem está em voz).
  const section = (title, kind) => {
    const key = `${server.id}:${kind}`;
    const collapsed = state.collapsed.has(key);
    const channels = server.channels.filter((c) => (c.kind ?? "text") === kind);
    const head = el("div", collapsed ? "category collapsed" : "category");
    const toggle = button("", "cat-toggle", () => {
      if (state.collapsed.has(key)) state.collapsed.delete(key);
      else state.collapsed.add(key);
      store("concord.collapsed", [...state.collapsed]);
      renderChannels();
    });
    toggle.setAttribute("aria-expanded", String(!collapsed));
    toggle.append(el("span", "chev", "▾"), el("span", null, title));
    head.append(toggle);
    if (server.can_moderate) {
      const add = button("+", "cat-add", () => createChannel(server, kind));
      add.title = kind === "voice" ? "Criar canal de voz" : "Criar canal de texto";
      add.setAttribute("aria-label", add.title);
      head.append(add);
    }
    const list = el("ul", "channel-list");
    const voiceNow = new Set((state.voiceStates.get(server.id) ?? []).map((s) => s.channel_id));
    for (const c of channels) {
      if (collapsed && c.id !== state.channelId && !voiceNow.has(c.id)) continue;
      list.append(channelItem(server, c));
    }
    box.append(head, list);
  };
  section("Canais de texto", "text");
  section("Canais de voz", "voice");
}

function channelItem(server, c) {
  const li = el("li");
  const row = el("div", "ch-row" + (c.id === state.channelId ? " active" : "") + (state.unread.has(c.id) ? " unread" : ""));
  row.classList.add(c.kind === "voice" ? "voice" : "text");
  const name = el("span", "ch-name", c.name);
  row.append(name);
  const mentions = state.mentionCounts.get(c.id);
  if (mentions) row.append(el("span", "ch-ping", String(mentions)));
  if (c.kind === "voice" && c.user_limit) {
    // Pílula "pessoas | limite".
    const n = (state.voiceStates.get(server.id) ?? []).filter((s) => s.channel_id === c.id).length;
    const pill = el("span", "ch-count");
    pill.title = `${n} de ${c.user_limit} pessoas`;
    pill.append(el("span", null, String(n).padStart(2, "0")), el("span", null, String(c.user_limit).padStart(2, "0")));
    row.append(pill);
  }
  if (server.can_moderate) {
    const gear = button("⚙", "ch-tool", (e) => { e.stopPropagation(); channelSettings(server, c); });
    gear.title = "Configurações do canal";
    row.append(gear);
    if (server.channels.length > 1) {
      const del = button("×", "ch-tool", (e) => { e.stopPropagation(); deleteChannel(server, c); });
      del.title = "Apagar canal";
      row.append(del);
    }
  }
  row.onclick = () => selectChannel(c.id);
  li.append(row);

  if (c.kind === "voice") {
    if (c.status) li.append(el("div", "ch-voice-status", c.status));
    const people = (state.voiceStates.get(server.id) ?? []).filter((s) => s.channel_id === c.id);
    if (people.length) {
      const ul = el("ul", "ch-voice-users");
      for (const s of people) {
        const u = el("li");
        const a = avatar(s.user);
        a.classList.add("tiny");
        a.dataset.speaker = s.user.id;
        const nm = el("span", "vu-name", s.user.name);
        u.append(a, nm);
        if (s.streaming) u.append(el("span", "live-badge small", "AO VIVO"));
        if (s.camera) u.append(el("span", "vu-flag", "📷"));
        if (s.server_deafened) u.append(flag(ICON_DEAF, "Áudio desativado por um moderador", true));
        else if (s.server_muted) u.append(flag(ICON_MIC_OFF, "Silenciado por um moderador", true));
        else if (s.deafened) u.append(flag(ICON_DEAF, "Áudio desativado"));
        else if (s.muted) u.append(flag(ICON_MIC_OFF, "Microfone desligado"));
        u.onclick = (e) => userMenu(e, s.user);
        ul.append(u);
      }
      li.append(ul);
    }
  }
  return li;
}

function flag(symbol, title, byMod) {
  const f = el("span", byMod ? "vu-flag by-mod" : "vu-flag", symbol);
  f.title = title;
  return f;
}

async function createChannel(server, kind) {
  const voiceKind = kind === "voice";
  const raw = await askText({
    title: voiceKind ? "Criar canal de voz" : "Criar canal de texto",
    desc: `Em ${server.name}`,
    placeholder: voiceKind ? "Sala de jogos" : "novo-canal",
    maxLength: 32,
    submit: "Criar",
  });
  if (raw) send({ type: "create_channel", server_id: server.id, name: raw, kind });
}

function renderChat(scroll) {
  const ch = currentChannel();
  const isVoice = ch?.kind === "voice";
  $("#channel-icon").textContent = isVoice ? "🔊" : "#";
  $("#channel-name").textContent = ch?.name ?? "";
  $("#channel-status").textContent = isVoice && ch.status ? ch.status : "";
  $("#text-view").classList.toggle("hidden", isVoice);
  $("#voice-view").classList.toggle("hidden", !isVoice);
  if (isVoice) {
    renderVoiceView();
    return;
  }
  $("#input").placeholder = ch ? `Conversar em #${ch.name}` : "";
  $("#input").disabled = !ch;
  renderMessages(scroll);
  renderTyping();
}

function roleBadge(server, userId) {
  const role = roleIn(server, userId);
  if (role === "owner") return el("span", "badge owner", "DONO");
  if (role === "mod") return el("span", "badge mod", "MOD");
  return null;
}

function renderMessages(scroll) {
  const box = $("#messages");
  const nearBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 120;
  const prevTop = box.scrollTop;
  const prevHeight = box.scrollHeight;
  box.replaceChildren();

  const server = currentServer();
  const ch = currentChannel();
  if (!ch) {
    if (!server) {
      const intro = el("div", "channel-intro");
      intro.append(el("h1", null, "Nenhum servidor por aqui"), el("p", null, "Use o botão + para criar um servidor ou entrar com um convite."));
      box.append(intro);
    }
    return;
  }
  const list = state.messages.get(ch.id);
  if (!list) {
    box.append(el("div", "empty", "Carregando…"));
    return;
  }

  if (state.hasMore.get(ch.id)) {
    box.append(el("div", "empty", state.loadingOlder.has(ch.id) ? "Carregando mensagens antigas…" : "Role para cima para ver mensagens antigas"));
  } else {
    const intro = el("div", "channel-intro");
    intro.append(el("div", "intro-icon", "#"), el("h1", null, `Bem-vindo(a) a #${ch.name}!`), el("p", null, `Este é o começo do canal #${ch.name}.`));
    box.append(intro);
  }

  let prev = null;
  for (const m of list) {
    // Divisória de data quando o dia muda (ou antes da primeira mensagem).
    const day = new Date(m.timestamp).toDateString();
    const newDay = !prev || new Date(prev.timestamp).toDateString() !== day;
    if (newDay) {
      const label = new Date(m.timestamp).toLocaleDateString("pt-BR", { day: "numeric", month: "long", year: "numeric" });
      const div = el("div", "date-divider", label);
      div.setAttribute("role", "separator");
      box.append(div);
    }
    // Respostas nunca são agrupadas: a prévia precisa do cabeçalho.
    const grouped = !newDay && !m.reply_to && prev && prev.author.id === m.author.id && m.timestamp - prev.timestamp < 5 * 60 * 1000;
    const row = el("div", grouped ? "msg grouped" : "msg");
    row.dataset.mid = m.id;
    if (mentionsMe(m) || (m.reply_to?.author?.id === state.me?.id && m.author.id !== state.me?.id)) row.classList.add("mentioned");
    if (m.reply_to) row.prepend(replyPreviewEl(m));
    const body = el("div", "msg-body");

    if (grouped) {
      row.append(el("div", "msg-gutter", shortTime(m.timestamp)));
    } else {
      const gutter = el("div", "msg-gutter");
      gutter.append(avatar(m.author, true));
      row.append(gutter);
      const head = el("div", "msg-head");
      const name = el("span", "msg-author", m.author.name);
      name.style.color = m.author.color;
      name.onclick = (e) => userMenu(e, m.author);
      head.append(name);
      const badge = roleBadge(server, m.author.id);
      if (badge) head.append(badge);
      head.append(el("span", "msg-time", formatTime(m.timestamp)));
      body.append(head);
    }

    if (editing === m.id) {
      body.append(editorEl(m));
    } else if (m.content) {
      const content = el("div", "msg-content");
      content.append(renderContent(m.content));
      if (m.edited_at) {
        const tag = el("span", "edited", "(editado)");
        tag.title = `Editado em ${formatTime(m.edited_at)}`;
        content.append(" ", tag);
      }
      body.append(content);
    }
    if (m.attachments?.length) body.append(attachmentsEl(m));
    if (m.reactions?.length) body.append(reactionsEl(m));
    row.append(body);
    row.append(messageActionsEl(m, server));
    const openMsgMenu = (e) => {
      if (e.target.closest("a, .att-img, textarea, .reaction")) return;
      e.preventDefault();
      messageMenu(e, m, server);
    };
    row.oncontextmenu = openMsgMenu;
    longPress(row, openMsgMenu);

    box.append(row);
    prev = m;
  }

  if (scroll === "bottom" || (scroll === "auto" && nearBottom)) {
    box.scrollTop = box.scrollHeight;
  } else if (scroll === "prepend") {
    // Mantém na tela a mesma mensagem que a pessoa estava vendo.
    box.scrollTop = box.scrollHeight - prevHeight + prevTop;
  } else {
    box.scrollTop = prevTop;
  }
}

// Perto do topo: busca a página anterior (paginação por id no servidor).
$("#messages").addEventListener("scroll", () => {
  const box = $("#messages");
  const id = state.channelId;
  if (box.scrollTop > 150 || !id || !state.hasMore.get(id) || state.loadingOlder.has(id)) return;
  const oldest = state.messages.get(id)?.[0];
  if (!oldest) return;
  state.loadingOlder.add(id);
  send({ type: "fetch_history", channel_id: id, before: oldest.id });
});

function renderTyping() {
  const now = Date.now();
  const map = state.typing.get(state.channelId);
  const names = [];
  if (map) {
    for (const [id, t] of map) {
      if (t.until < now) map.delete(id);
      else names.push(t.name);
    }
  }
  let text = "";
  if (names.length === 1) text = `${names[0]} está digitando…`;
  else if (names.length === 2) text = `${names[0]} e ${names[1]} estão digitando…`;
  else if (names.length > 2) text = "Várias pessoas estão digitando…";
  $("#typing").textContent = text;
}

/**
 * Servidor privado: membros dele, separados em online e offline.
 * Servidor público: quem está online na instância (todos podem participar).
 */
function renderMembers() {
  const server = currentServer();
  const onlineById = new Map(state.online.map((u) => [u.id, u]));
  let online, offline;
  if (server?.members) {
    // O perfil de quem está online é o mais atual.
    online = server.members.filter((u) => onlineById.has(u.id)).map((u) => onlineById.get(u.id));
    offline = server.members.filter((u) => !onlineById.has(u.id));
    $("#members-title").textContent = `Membros — ${server.members.length}`;
  } else {
    online = state.online;
    offline = [];
    $("#members-title").textContent = server ? "Servidor público" : "";
  }
  const byName = (a, b) => a.name.localeCompare(b.name, "pt-BR");
  const list = $("#member-list");
  list.replaceChildren();
  const section = (title, users, isOnline) => {
    if (users.length === 0) return;
    list.append(el("li", "member-section", `${title} — ${users.length}`));
    for (const u of [...users].sort(byName)) {
      const li = el("li", isOnline ? null : "offline");
      const a = avatar(u);
      if (isOnline) a.classList.add("status");
      const name = el("span", "name", u.name);
      name.style.color = u.color;
      li.append(a, name);
      const badge = roleBadge(server, u.id);
      if (badge) li.append(badge);
      li.onclick = (e) => userMenu(e, u);
      list.append(li);
    }
  };
  // Seções por papel (online primeiro), depois offline.
  const role = (u) => roleIn(server, u.id);
  section("Dono", online.filter((u) => role(u) === "owner"), true);
  section("Moderação", online.filter((u) => role(u) === "mod"), true);
  section("Online", online.filter((u) => !role(u)), true);
  section("Offline", offline, false);
}

function renderMe() {
  const name = identity.name || "…";
  $("#me-name").textContent = name;
  const a = $("#me-avatar");
  a.textContent = [...name][0] || "?";
  a.style.background = identity.color;
  const s = currentServer();
  const role = s && state.me ? roleIn(s, state.me.id) : null;
  $("#me-sub").textContent =
    role === "owner" ? "Dono(a) deste servidor" : role === "mod" ? "Moderador(a)" : state.isAdmin ? "Admin da instância" : "Online";
}

// ---------- ações ----------
$("#me-info").onclick = () => openSettings("user", "account");
$("#me-avatar").onclick = () => openSettings("user", "account");

// O compositor (envio, imagens, menções, emojis) fica em chat.js.
setInterval(renderTyping, 1000);

// ---------- início ----------
renderMe();
connect();
