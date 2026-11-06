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
    (card.querySelector("input[type=text]:not([readonly]), textarea, select") || ok).focus();

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

// ---------- menu suspenso ----------
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
  const list = items.filter(Boolean);
  if (list.length === 0) return;
  const menu = el("div", "menu");
  for (const it of list) {
    if (it === "-") {
      menu.append(el("div", "menu-sep"));
      continue;
    }
    menu.append(button(it.label, it.danger ? "menu-item danger" : "menu-item", () => { closeMenu(); it.onClick(); }));
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
  online: [],
  typing: new Map(), // channelId -> Map(userId -> { name, until })
  unread: new Set(),
  reports: new Map(), // serverId -> [ReportView]
  focus: null, // { server_id, channel_id } pedido pelo servidor
  panel: null, // painel aberto: "reports" | "bans"
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
      state.servers = m.servers;
      state.online = m.online;
      state.messages.clear();
      state.reports.clear();
      setStatus(null);
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
      if (state.panel === "bans" && m.server.id === state.serverId) renderBansPanel();
      break;
    }

    case "server_removed": {
      const had = state.servers.some((s) => s.id === m.server_id);
      state.servers = state.servers.filter((s) => s.id !== m.server_id);
      state.reports.delete(m.server_id);
      if (had) refresh();
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

    case "reports":
      state.reports.set(m.server_id, m.reports);
      renderServers();
      if (state.panel === "reports" && m.server_id === state.serverId) renderReportsPanel();
      break;

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

    case "info":
      toast(m.message, true);
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

$("#open-privacy").onclick = () => show("#privacy");
$("#privacy-close").onclick = () => hide("#privacy");
$("#privacy").onkeydown = (e) => { if (e.key === "Escape") hide("#privacy"); };
$("#export-data").onclick = () => send({ type: "export_my_data" });
$("#privacy-rename").onclick = () => { hide("#privacy"); rename(); };
$("#delete-data").onclick = async () => {
  if (await confirmTyping("Apagar meus dados", "Todas as suas mensagens e seu registro serão eliminados.", "APAGAR")) {
    send({ type: "delete_my_data" });
  }
};
$("#goodbye-restart").onclick = () => location.reload();

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

async function serverSettings(server) {
  const r = await dialog({
    title: "Configurações do servidor",
    submit: "Salvar",
    fields: [
      { name: "name", label: "Nome", value: server.name, required: true },
      { name: "public", type: "checkbox", value: server.public, label: "Público: qualquer pessoa desta instância pode ver e participar." },
    ],
  });
  if (r) send({ type: "update_server", server_id: server.id, name: r.name, public: r.public });
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
    { label: "Criar servidor", onClick: createServer },
    { label: "Entrar com convite", onClick: joinServer },
  ]);

$("#server-header").onclick = (e) => {
  const s = currentServer();
  if (!s) return;
  const reports = state.reports.get(s.id)?.length ?? 0;
  showMenu(e, [
    s.can_moderate && { label: "Convidar pessoas", onClick: () => showInvite(s) },
    s.can_moderate && { label: `Denúncias (${reports})`, onClick: () => openPanel("reports") },
    s.can_moderate && { label: `Banidos (${s.bans?.length ?? 0})`, onClick: () => openPanel("bans") },
    s.can_manage && { label: "Configurações do servidor", onClick: () => serverSettings(s) },
    (s.role === "member" || s.role === "moderator") && "-",
    (s.role === "member" || s.role === "moderator") && { label: "Sair do servidor", danger: true, onClick: () => leaveServer(s) },
    s.can_manage && "-",
    s.can_manage && { label: "Apagar servidor", danger: true, onClick: () => deleteServer(s) },
  ]);
};

// ---------- moderação ----------
function userMenu(e, user) {
  const s = currentServer();
  if (!s || !state.me) return;
  const isMe = user.id === state.me.id;
  const role = roleIn(s, user.id);
  showMenu(e, [
    isMe && { label: "Mudar meu apelido", onClick: rename },
    !isMe && s.can_manage && role !== "owner" && {
      label: role === "mod" ? "Remover moderador" : "Tornar moderador",
      onClick: () => send({ type: "set_moderator", server_id: s.id, user_id: user.id, enabled: role !== "mod" }),
    },
    !isMe && s.can_moderate && role !== "owner" && (role !== "mod" || s.can_manage) && {
      label: "Banir do servidor", danger: true, onClick: () => banUser(s, user),
    },
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

function openPanel(kind) {
  state.panel = kind;
  if (kind === "reports") renderReportsPanel();
  else renderBansPanel();
  show("#panel");
}

function closePanel() {
  state.panel = null;
  hide("#panel");
}
$("#panel-close").onclick = closePanel;
$("#panel").onkeydown = (e) => { if (e.key === "Escape") closePanel(); };

function renderReportsPanel() {
  const s = currentServer();
  if (!s?.can_moderate) return closePanel();
  const reports = state.reports.get(s.id) ?? [];
  $("#panel-title").textContent = `Denúncias — ${s.name}`;
  const body = $("#panel-body");
  body.replaceChildren();
  if (reports.length === 0) {
    body.append(el("div", "empty-note", "Nenhuma denúncia pendente."));
    return;
  }
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
    body.append(item);
  }
}

function renderBansPanel() {
  const s = currentServer();
  if (!s?.can_moderate) return closePanel();
  $("#panel-title").textContent = `Banidos — ${s.name}`;
  const body = $("#panel-body");
  body.replaceChildren();
  const bans = s.bans ?? [];
  if (bans.length === 0) {
    body.append(el("div", "empty-note", "Ninguém banido."));
    return;
  }
  for (const b of bans) {
    const item = el("div", "item");
    const head = el("div", "item-head");
    head.append(el("strong", null, b.name), el("span", null, formatTime(b.at)));
    item.append(head, el("div", null, `Motivo: ${b.reason}`));
    const actions = el("div", "item-actions");
    actions.append(button("Revogar banimento", null, () => send({ type: "unban_user", server_id: s.id, user_id: b.user_id })));
    item.append(actions);
    body.append(item);
  }
}

// ---------- navegação ----------
function ensureSelection() {
  if (!currentServer()) state.serverId = state.servers[0]?.id ?? null;
  const server = currentServer();
  if (server && !currentChannel()) {
    const last = load(`concord.lastChannel.${server.id}`, null);
    state.channelId = server.channels.some((c) => c.id === last) ? last : server.channels[0]?.id ?? null;
  }
  if (!server) state.channelId = null;
  if (state.channelId && !state.messages.has(state.channelId)) {
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
  if (state.panel && !currentServer()?.can_moderate) closePanel();
}

function selectServer(id) {
  state.serverId = id;
  state.channelId = null;
  refresh();
}

function selectChannel(id) {
  state.channelId = id;
  state.unread.delete(id);
  store(`concord.lastChannel.${state.serverId}`, id);
  refresh();
  $("#input").focus();
}

// ---------- renderização ----------
function renderServers() {
  const list = $("#server-list");
  list.replaceChildren();
  for (const s of state.servers) {
    const b = el("button", "server-icon" + (s.id === state.serverId ? " active" : ""), initials(s.name));
    const pending = s.can_moderate ? state.reports.get(s.id)?.length ?? 0 : 0;
    b.title = s.name + (s.public ? " (público)" : " (privado)") + (pending ? ` — ${pending} denúncia(s)` : "");
    if (!s.public) b.append(el("span", "lock", "🔒"));
    b.onclick = () => selectServer(s.id);
    list.append(b);
  }
}

function renderChannels() {
  const server = currentServer();
  $("#server-name").textContent = server?.name ?? "Nenhum servidor";
  $("#add-channel").classList.toggle("hidden", !server?.can_moderate);
  const list = $("#channel-list");
  list.replaceChildren();
  if (!server) return;
  for (const c of server.channels) {
    let cls = c.id === state.channelId ? "active" : "";
    if (state.unread.has(c.id)) cls += " unread";
    const li = el("li", cls.trim() || null);
    li.append(el("span", "ch-name", c.name));
    if (server.can_moderate && server.channels.length > 1) {
      const del = button("×", "ch-del", (e) => { e.stopPropagation(); deleteChannel(server, c); });
      del.title = "Apagar canal";
      li.append(del);
    }
    li.onclick = () => selectChannel(c.id);
    list.append(li);
  }
}

function renderChat(scroll) {
  const ch = currentChannel();
  $("#channel-name").textContent = ch?.name ?? "";
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
      name.onclick = (e) => userMenu(e, m.author);
      head.append(name);
      const badge = roleBadge(server, m.author.id);
      if (badge) head.append(badge);
      head.append(el("span", "msg-time", formatTime(m.timestamp)));
      body.append(head);
    }

    const content = el("div", "msg-content");
    content.append(renderContent(m.content));
    body.append(content);
    row.append(body);

    const mine = m.author.id === state.me?.id;
    const actions = el("div", "msg-actions");
    if (!mine) actions.append(button("Denunciar", null, () => reportMessage(m)));
    if (mine || server?.can_moderate) {
      actions.append(button("Apagar", "danger", () => send({ type: "delete_message", channel_id: m.channel_id, message_id: m.id })));
    }
    row.append(actions);

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
  const rank = (u) => ({ owner: 0, mod: 1 })[roleIn(server, u.id)] ?? 2;
  const byRole = (a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name, "pt-BR");

  const list = $("#member-list");
  list.replaceChildren();
  const section = (title, users, isOnline) => {
    if (users.length === 0) return;
    list.append(el("li", "member-section", `${title} — ${users.length}`));
    for (const u of [...users].sort(byRole)) {
      const li = el("li", isOnline ? null : "offline");
      const name = el("span", "name", u.name);
      name.style.color = u.color;
      li.append(avatar(u), name);
      const badge = roleBadge(server, u.id);
      if (badge) li.append(badge);
      li.onclick = (e) => userMenu(e, u);
      list.append(li);
    }
  };
  section("Online", online, true);
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
$("#add-channel").onclick = async () => {
  const server = currentServer();
  if (!server) return;
  const raw = await askText({ title: "Criar canal", desc: `Em ${server.name}`, placeholder: "novo-canal", maxLength: 32, submit: "Criar" });
  if (raw) send({ type: "create_channel", server_id: server.id, name: raw });
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
