"use strict";

// ---------- utilidades ----------
const $ = (sel) => document.querySelector(sel);

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
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

// Transforma URLs em links sem usar innerHTML (evita XSS).
function renderContent(text) {
  const frag = document.createDocumentFragment();
  const re = /https?:\/\/[^\s<]+/g;
  let last = 0;
  for (const m of text.matchAll(re)) {
    frag.append(text.slice(last, m.index));
    const a = el("a", null, m[0]);
    a.href = m[0];
    a.target = "_blank";
    a.rel = "noopener noreferrer";
    frag.append(a);
    last = m.index + m[0].length;
  }
  frag.append(text.slice(last));
  return frag;
}

let toastTimer = null;
function toast(msg) {
  const t = $("#toast");
  t.textContent = msg;
  t.classList.remove("hidden");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.add("hidden"), 3500);
}

function show(sel) { $(sel).classList.remove("hidden"); }
function hide(sel) { $(sel).classList.add("hidden"); }

// Modal simples que devolve o texto digitado (ou null se cancelar).
function askText({ title, desc = "", placeholder = "", value = "", maxLength = 50 }) {
  return new Promise((resolve) => {
    const input = $("#modal-input");
    const form = $("#modal-card");
    $("#modal-title").textContent = title;
    $("#modal-desc").textContent = desc;
    input.placeholder = placeholder;
    input.value = value;
    input.maxLength = maxLength;
    show("#modal");
    input.focus();
    input.select();

    const finish = (result) => {
      hide("#modal");
      form.onsubmit = null;
      $("#modal-cancel").onclick = null;
      $("#modal").onkeydown = null;
      resolve(result);
    };
    form.onsubmit = (e) => {
      e.preventDefault();
      const v = input.value.trim();
      if (v) finish(v);
    };
    $("#modal-cancel").onclick = () => finish(null);
    $("#modal").onkeydown = (e) => { if (e.key === "Escape") finish(null); };
  });
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
  servers: [],
  serverId: load("concord.server", null),
  channelId: load("concord.channel", null),
  messages: new Map(), // channelId -> [Message]
  online: [],
  typing: new Map(), // channelId -> Map(userId -> { name, until })
  unread: new Set(),
  pendingServer: null,
  pendingChannel: null,
};
let ws = null;
let reconnect = true;

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

// ---------- conexão ----------
function connect() {
  reconnect = true;
  const proto = location.protocol === "https:" ? "wss" : "ws";
  ws = new WebSocket(`${proto}://${location.host}/ws`);
  ws.onopen = hello;
  ws.onmessage = (e) => handle(JSON.parse(e.data));
  ws.onclose = () => {
    if (!reconnect) return;
    setStatus("Sem conexão com o servidor — tentando reconectar…");
    setTimeout(connect, 1500);
  };
}

function setStatus(text) {
  const s = $("#status");
  s.textContent = text || "";
  s.classList.toggle("hidden", !text);
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
      state.servers = m.servers;
      state.online = m.online;
      state.messages.clear();
      setStatus(null);
      ensureSelection();
      renderAll();
      break;

    case "server_created":
      state.servers.push(m.server);
      if (state.pendingServer === m.server.name) {
        state.pendingServer = null;
        selectServer(m.server.id);
      } else {
        renderServers();
      }
      break;

    case "channel_created": {
      const server = state.servers.find((s) => s.id === m.server_id);
      if (!server) break;
      server.channels.push(m.channel);
      if (state.pendingChannel === m.channel.name && server.id === state.serverId) {
        state.pendingChannel = null;
        selectChannel(m.channel.id);
      } else if (server.id === state.serverId) {
        renderChannels();
      }
      break;
    }

    case "message": {
      const msg = m.message;
      const list = state.messages.get(msg.channel_id);
      if (list) list.push(msg);
      state.typing.get(msg.channel_id)?.delete(msg.author.id);
      if (msg.channel_id === state.channelId) {
        renderMessages(msg.author.id === state.me?.id ? "bottom" : "auto");
        renderTyping();
      } else {
        state.unread.add(msg.channel_id);
        renderChannels();
      }
      break;
    }

    case "message_deleted": {
      const list = state.messages.get(m.channel_id);
      if (!list) break;
      const i = list.findIndex((x) => x.id === m.message_id);
      if (i >= 0) list.splice(i, 1);
      if (m.channel_id === state.channelId) renderMessages("keep");
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
      renderMessages("keep");
      break;

    case "history":
      state.messages.set(m.channel_id, m.messages);
      if (m.channel_id === state.channelId) renderMessages("bottom");
      break;

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

    case "my_data":
      downloadJson(m.data);
      break;

    case "data_deleted":
      reconnect = false;
      clearLocalData();
      // A conexão é encerrada logo em seguida; limpa a própria tela aqui.
      if (state.me) purgeUser(state.me.id);
      state.online = [];
      renderMembers();
      hide("#privacy");
      $("#goodbye-text").textContent =
        `Seus dados foram eliminados do servidor e deste navegador (${m.messages_removed} mensagem(ns) removida(s)).`;
      show("#goodbye");
      break;

    case "error":
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

$("#open-privacy").onclick = () => show("#privacy");
$("#privacy-close").onclick = () => hide("#privacy");
$("#privacy").onkeydown = (e) => { if (e.key === "Escape") hide("#privacy"); };
$("#export-data").onclick = () => send({ type: "export_my_data" });
$("#privacy-rename").onclick = () => { hide("#privacy"); rename(); };
$("#delete-data").onclick = async () => {
  const typed = await askText({
    title: "Apagar meus dados",
    desc: 'Todas as suas mensagens e seu registro serão eliminados. Digite APAGAR para confirmar.',
    placeholder: "APAGAR",
    maxLength: 10,
  });
  if (typed?.toUpperCase() !== "APAGAR") return;
  send({ type: "delete_my_data" });
};
$("#goodbye-restart").onclick = () => location.reload();

// ---------- navegação ----------
function ensureSelection() {
  if (!currentServer()) state.serverId = state.servers[0]?.id ?? null;
  const server = currentServer();
  if (server && !currentChannel()) state.channelId = server.channels[0]?.id ?? null;
  if (state.channelId && !state.messages.has(state.channelId)) {
    send({ type: "fetch_history", channel_id: state.channelId });
  }
  store("concord.server", state.serverId);
  store("concord.channel", state.channelId);
}

function selectServer(id) {
  state.serverId = id;
  state.channelId = load(`concord.lastChannel.${id}`, null);
  ensureSelection();
  renderAll();
}

function selectChannel(id) {
  state.channelId = id;
  state.unread.delete(id);
  store(`concord.lastChannel.${state.serverId}`, id);
  ensureSelection();
  renderChannels();
  renderChat();
  $("#input").focus();
}

// ---------- renderização ----------
function renderAll() {
  renderServers();
  renderChannels();
  renderChat();
  renderMembers();
  renderMe();
}

function renderServers() {
  const list = $("#server-list");
  list.replaceChildren();
  for (const s of state.servers) {
    const b = el("button", "server-icon" + (s.id === state.serverId ? " active" : ""), initials(s.name));
    b.title = s.name;
    b.onclick = () => selectServer(s.id);
    list.append(b);
  }
}

function renderChannels() {
  const server = currentServer();
  $("#server-name").textContent = server?.name ?? "";
  const list = $("#channel-list");
  list.replaceChildren();
  if (!server) return;
  for (const c of server.channels) {
    let cls = c.id === state.channelId ? "active" : "";
    if (state.unread.has(c.id)) cls += " unread";
    const li = el("li", cls.trim() || null, c.name);
    li.onclick = () => selectChannel(c.id);
    list.append(li);
  }
}

function renderChat() {
  const ch = currentChannel();
  $("#channel-name").textContent = ch?.name ?? "";
  $("#input").placeholder = ch ? `Conversar em #${ch.name}` : "";
  $("#input").disabled = !ch;
  renderMessages("bottom");
  renderTyping();
}

function renderMessages(scroll) {
  const box = $("#messages");
  const nearBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 120;
  const prevTop = box.scrollTop;
  box.replaceChildren();

  const ch = currentChannel();
  if (!ch) return;
  const list = state.messages.get(ch.id);
  if (!list) {
    box.append(el("div", "empty", "Carregando…"));
    return;
  }

  const intro = el("div", "channel-intro");
  intro.append(el("h1", null, `Bem-vindo(a) a #${ch.name}!`), el("p", null, `Este é o começo do canal #${ch.name}.`));
  box.append(intro);

  let prev = null;
  for (const m of list) {
    const grouped = prev && prev.author.id === m.author.id && m.timestamp - prev.timestamp < 5 * 60 * 1000;
    const row = el("div", grouped ? "msg grouped" : "msg");
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
      head.append(name, el("span", "msg-time", formatTime(m.timestamp)));
      body.append(head);
    }

    const content = el("div", "msg-content");
    content.append(renderContent(m.content));
    body.append(content);
    row.append(body);

    if (m.author.id === state.me?.id) {
      const del = el("button", "msg-delete", "Apagar");
      del.onclick = () => send({ type: "delete_message", channel_id: m.channel_id, message_id: m.id });
      row.append(del);
    }

    box.append(row);
    prev = m;
  }

  if (scroll === "bottom" || (scroll === "auto" && nearBottom)) {
    box.scrollTop = box.scrollHeight;
  } else {
    box.scrollTop = prevTop;
  }
}

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

function renderMembers() {
  $("#members-title").textContent = `Online — ${state.online.length}`;
  const list = $("#member-list");
  list.replaceChildren();
  for (const u of state.online) {
    const li = el("li");
    const name = el("span", "name", u.name);
    name.style.color = u.color;
    li.append(avatar(u), name);
    list.append(li);
  }
}

function renderMe() {
  const name = identity.name || "…";
  $("#me-name").textContent = name;
  const a = $("#me-avatar");
  a.textContent = [...name][0] || "?";
  a.style.background = identity.color;
}

// ---------- ações ----------
$("#add-server").onclick = async () => {
  const name = await askText({ title: "Criar servidor", desc: "Dê um nome para o seu servidor.", placeholder: "Meu servidor" });
  if (!name) return;
  state.pendingServer = name;
  send({ type: "create_server", name });
};

$("#add-channel").onclick = async () => {
  const server = currentServer();
  if (!server) return;
  const raw = await askText({ title: "Criar canal", desc: `Em ${server.name}`, placeholder: "novo-canal", maxLength: 32 });
  if (!raw) return;
  // Mesma normalização que o servidor faz, para reconhecer o canal criado.
  state.pendingChannel = raw.toLowerCase().replace(/[^\p{L}\p{N}_\s-]/gu, "").replace(/[\s-]+/g, "-").replace(/^-+|-+$/g, "");
  send({ type: "create_channel", server_id: server.id, name: raw });
};

$("#rename").onclick = rename;

const input = $("#input");
let lastTypingSent = 0;

function autosize() {
  input.style.height = "auto";
  input.style.height = Math.min(input.scrollHeight, 200) + "px";
}

input.addEventListener("input", () => {
  autosize();
  const now = Date.now();
  if (input.value.trim() && now - lastTypingSent > 2500 && state.channelId) {
    lastTypingSent = now;
    send({ type: "typing", channel_id: state.channelId });
  }
});

input.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    const content = input.value.trim();
    if (!content || !state.channelId) return;
    send({ type: "send_message", channel_id: state.channelId, content });
    input.value = "";
    lastTypingSent = 0;
    autosize();
  }
});

$("#composer").onsubmit = (e) => e.preventDefault();

setInterval(renderTyping, 1000);

// ---------- início ----------
renderMe();
connect();
