"use strict";
// Recursos de chat: emojis e reações, respostas, menções, edição, imagens e
// visualizador. Usa as funções globais do app.js (state, send, el, button, icon…).
// Design: docs/DESIGN.md › Mensagens, Compositor, Seletor de emoji.

// ---------- emojis ----------
// [emoji, palavras-chave para a busca]
const EMOJI_GROUPS = [
  ["Rostos", [
    ["😀", "sorrir feliz"], ["😃", "sorrir alegre"], ["😄", "riso"], ["😁", "sorriso dentes"], ["😆", "rindo"],
    ["😅", "suor nervoso"], ["🤣", "rolando de rir"], ["😂", "chorar de rir kkk"], ["🙂", "leve sorriso"],
    ["😉", "piscar"], ["😊", "feliz corado"], ["😇", "anjo"], ["🥰", "apaixonado amor"], ["😍", "olhos coração"],
    ["🤩", "estrelas uau"], ["😘", "beijo"], ["😋", "delicioso"], ["😛", "língua"], ["😜", "piscando língua"],
    ["🤪", "louco doido"], ["🤔", "pensando hmm"], ["🤨", "desconfiado"], ["😐", "neutro"], ["😑", "sem expressão"],
    ["🙄", "revirar olhos"], ["😏", "sorriso de lado"], ["😬", "tenso"], ["😌", "aliviado"], ["😴", "dormindo sono"],
    ["😷", "máscara doente"], ["🤯", "explodindo mente"], ["🥳", "festa aniversário"], ["😎", "óculos legal"],
    ["🤓", "nerd"], ["😕", "confuso"], ["😮", "surpreso"], ["😲", "chocado"], ["🥺", "pidão por favor"],
    ["😢", "triste chorando"], ["😭", "chorando muito"], ["😱", "grito medo"], ["😤", "bufando"], ["😡", "bravo raiva"],
    ["🤬", "xingando"], ["💀", "caveira morri"], ["🤡", "palhaço"], ["👻", "fantasma"], ["🤖", "robô"], ["💩", "cocô"],
  ]],
  ["Gestos", [
    ["👍", "joinha positivo sim"], ["👎", "negativo não"], ["👌", "ok perfeito"], ["✌️", "paz vitória"],
    ["🤞", "dedos cruzados sorte"], ["🤟", "amo você"], ["🤙", "liga"], ["👋", "tchau oi"], ["👏", "palmas"],
    ["🙌", "comemorar"], ["🙏", "por favor obrigado"], ["💪", "força"], ["🫡", "continência"], ["🤝", "acordo"],
    ["👀", "olhos vendo"], ["🫶", "coração mãos"],
  ]],
  ["Corações e festa", [
    ["❤️", "coração amor vermelho"], ["🧡", "coração laranja"], ["💛", "coração amarelo"], ["💚", "coração verde"],
    ["💙", "coração azul"], ["💜", "coração roxo"], ["🖤", "coração preto"], ["🤍", "coração branco"],
    ["💔", "coração partido"], ["💯", "cem"], ["✨", "brilho"], ["🔥", "fogo"], ["⭐", "estrela"], ["🎉", "festa"],
    ["🎊", "confete"], ["🎁", "presente"],
  ]],
  ["Coisas", [
    ["🎮", "jogo videogame"], ["🕹️", "controle"], ["🎧", "fone música"], ["🎵", "música nota"], ["📚", "livros estudo"],
    ["💻", "computador"], ["📱", "celular"], ["☕", "café"], ["🍕", "pizza"], ["🍔", "hambúrguer"], ["🍿", "pipoca"],
    ["🍺", "cerveja"], ["⚽", "futebol bola"], ["🏆", "troféu"], ["🚀", "foguete"], ["💡", "ideia"], ["📌", "fixar"],
    ["🔔", "sino aviso"], ["⏰", "despertador"],
  ]],
  ["Símbolos", [
    ["✅", "certo feito"], ["❌", "errado não"], ["⚠️", "atenção aviso"], ["❓", "pergunta"], ["❗", "exclamação"],
    ["➕", "mais"], ["🆗", "ok"], ["🔴", "vermelho"], ["🟢", "verde"], ["🔵", "azul"], ["🇧🇷", "brasil bandeira"],
  ]],
];

const recentEmojis = () => load("concord.emojiRecent", ["👍", "😂", "❤️", "🔥", "🎉"]);
function rememberEmoji(e) {
  store("concord.emojiRecent", [e, ...recentEmojis().filter((x) => x !== e)].slice(0, 16));
}

/** Seletor de emoji ancorado num elemento. `onPick(emoji)`. */
function openEmojiPicker(anchor, onPick) {
  closeMenu();
  const pop = el("div", "emoji-picker");
  pop.setAttribute("role", "dialog");
  pop.setAttribute("aria-label", "Escolher emoji");
  pop.onclick = (e) => e.stopPropagation();
  const search = el("input", "field");
  search.type = "text";
  search.placeholder = "Buscar emoji (ex.: riso, coração, festa)";
  const body = el("div", "ep-body");
  const footer = el("div", "ep-footer", "Passe o mouse num emoji");
  const pick = (e) => {
    rememberEmoji(e);
    closeMenu();
    onPick(e);
  };
  const render = () => {
    const q = search.value.trim().toLowerCase();
    body.replaceChildren();
    const groups = q
      ? [["Resultados", EMOJI_GROUPS.flatMap(([, list]) => list).filter(([e, k]) => k.includes(q) || e === q)]]
      : [["Usados recentemente", recentEmojis().map((e) => [e, ""])], ...EMOJI_GROUPS];
    for (const [title, list] of groups) {
      if (list.length === 0) continue;
      body.append(el("div", "ep-title", title));
      const grid = el("div", "ep-grid");
      for (const [e, k] of list) {
        const b = button(e, "ep-emoji", () => pick(e));
        b.title = k || e;
        b.onmouseenter = () => { footer.textContent = k ? `${e}  ${k}` : e; };
        grid.append(b);
      }
      body.append(grid);
    }
    if (!body.children.length) body.append(el("div", "empty-note", "Nenhum emoji encontrado"));
  };
  search.oninput = render;
  search.onkeydown = (e) => {
    if (e.key === "Enter") {
      const first = body.querySelector(".ep-emoji");
      if (first) first.click();
    }
  };
  pop.append(search, body, footer);
  render();
  document.body.append(pop);
  const r = anchor.getBoundingClientRect();
  const left = Math.max(8, Math.min(r.right - pop.offsetWidth, window.innerWidth - pop.offsetWidth - 8));
  let top = r.top - pop.offsetHeight - 8;
  if (top < 8) top = Math.min(r.bottom + 8, window.innerHeight - pop.offsetHeight - 8);
  pop.style.left = `${left}px`;
  pop.style.top = `${top}px`;
  openMenu = pop; // fecha com clique fora/Esc, como os menus
  search.focus();
}

// ---------- pessoas (para menções e reações) ----------
function knownUsers() {
  const map = new Map();
  const server = currentServer();
  for (const u of server?.members ?? []) map.set(u.id, u);
  for (const u of state.online) map.set(u.id, u);
  for (const m of state.messages.get(state.channelId) ?? []) if (!map.has(m.author.id)) map.set(m.author.id, m.author);
  if (state.me) map.set(state.me.id, { ...state.me, name: identity.name, color: identity.color });
  return map;
}

function userById(id) {
  return knownUsers().get(id) ?? null;
}

const mentionsMe = (m) => !!state.me && m.content.includes(`<@${state.me.id}>`);

// ---------- compositor ----------
const composer = {
  replyTo: null, // { id, author }
  uploads: [], // { key, file, blob, url, status: "uploading" | "ready" | "error", id }
  mentionMap: new Map(), // "@Nome" -> id
  uploadWaiters: [],
};
const input = $("#input");
let lastTypingSent = 0;

function autosize() {
  input.style.height = "auto";
  input.style.height = Math.min(input.scrollHeight, 200) + "px";
}

function setReply(m) {
  composer.replyTo = { id: m.id, author: m.author };
  $("#reply-text").replaceChildren("Respondendo a ", el("strong", null, m.author.name));
  show("#reply-bar");
  input.focus();
}

function clearReply() {
  composer.replyTo = null;
  hide("#reply-bar");
}

function clearComposer() {
  clearReply();
  for (const u of composer.uploads) URL.revokeObjectURL(u.url);
  composer.uploads = [];
  composer.mentionMap.clear();
  editing = null;
  renderUploads();
}
$("#reply-cancel").onclick = clearReply;

/** Troca "@Nome" pelos tokens de menção antes de enviar. */
function encodeMentions(text) {
  let out = text;
  for (const [label, id] of composer.mentionMap) out = out.split(label).join(`<@${id}>`);
  return out;
}

async function sendComposer() {
  const raw = input.value.trim();
  const busy = composer.uploads.some((u) => u.status === "uploading");
  if (busy) return toast("Aguarde as imagens terminarem de enviar");
  const ready = composer.uploads.filter((u) => u.status === "ready").map((u) => u.id);
  if (!raw && ready.length === 0) return;
  if (!state.channelId) return;
  playSfx("message_sent");
  send({
    type: "send_message",
    channel_id: state.channelId,
    content: encodeMentions(raw),
    reply_to: composer.replyTo?.id ?? null,
    attachments: ready,
  });
  input.value = "";
  composer.mentionMap.clear();
  for (const u of composer.uploads) URL.revokeObjectURL(u.url);
  composer.uploads = [];
  renderUploads();
  clearReply();
  lastTypingSent = 0;
  autosize();
  hideMentionSuggest();
}

input.addEventListener("input", () => {
  autosize();
  updateMentionSuggest();
  const now = Date.now();
  if (input.value.trim() && now - lastTypingSent > 2500 && state.channelId) {
    lastTypingSent = now;
    send({ type: "typing", channel_id: state.channelId });
  }
});

input.addEventListener("keydown", (e) => {
  if (mentionKeydown(e)) return;
  if (e.key === "Escape" && composer.replyTo) {
    clearReply();
    return;
  }
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    sendComposer();
  }
});

$("#composer").onsubmit = (e) => e.preventDefault();
$("#emoji-btn").onclick = (e) => {
  e.stopPropagation();
  openEmojiPicker(e.currentTarget, (emoji) => {
    const start = input.selectionStart ?? input.value.length;
    input.value = input.value.slice(0, start) + emoji + input.value.slice(input.selectionEnd ?? start);
    input.focus();
    input.selectionStart = input.selectionEnd = start + emoji.length;
    autosize();
  });
};

// ---------- menções: sugestões ao digitar @ ----------
let suggest = { items: [], index: 0, start: 0 };

function hideMentionSuggest() {
  hide("#mention-suggest");
  suggest.items = [];
}

function updateMentionSuggest() {
  const caret = input.selectionStart ?? input.value.length;
  const before = input.value.slice(0, caret);
  const m = before.match(/(^|\s)@([^\s@]{0,32})$/);
  if (!m) return hideMentionSuggest();
  const q = m[2].toLowerCase();
  const users = [...knownUsers().values()]
    .filter((u) => u.id !== state.me?.id && u.name.toLowerCase().includes(q))
    .sort((a, b) => a.name.localeCompare(b.name, "pt-BR"))
    .slice(0, 8);
  if (users.length === 0) return hideMentionSuggest();
  suggest = { items: users, index: 0, start: caret - m[2].length - 1 };
  renderMentionSuggest();
}

function renderMentionSuggest() {
  const box = $("#mention-suggest");
  box.replaceChildren(el("div", "ms-title", "Pessoas"));
  suggest.items.forEach((u, i) => {
    const row = el("button", i === suggest.index ? "ms-item active" : "ms-item");
    row.type = "button";
    const name = el("span", null, u.name);
    name.style.color = u.color;
    row.append(avatar(u), name);
    row.onmousedown = (e) => {
      e.preventDefault();
      pickMention(u);
    };
    box.append(row);
  });
  show("#mention-suggest");
}

function pickMention(u) {
  const label = `@${u.name}`;
  composer.mentionMap.set(label, u.id);
  const caret = input.selectionStart ?? input.value.length;
  input.value = `${input.value.slice(0, suggest.start)}${label} ${input.value.slice(caret)}`;
  const pos = suggest.start + label.length + 1;
  input.selectionStart = input.selectionEnd = pos;
  hideMentionSuggest();
  input.focus();
}

function mentionKeydown(e) {
  if (!suggest.items.length || $("#mention-suggest").classList.contains("hidden")) return false;
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    const n = suggest.items.length;
    suggest.index = (suggest.index + (e.key === "ArrowDown" ? 1 : n - 1)) % n;
    renderMentionSuggest();
    return true;
  }
  if (e.key === "Enter" || e.key === "Tab") {
    e.preventDefault();
    pickMention(suggest.items[suggest.index]);
    return true;
  }
  if (e.key === "Escape") {
    hideMentionSuggest();
    return true;
  }
  return false;
}
input.addEventListener("blur", () => setTimeout(hideMentionSuggest, 150));

// ---------- arquivos ----------
const MAX_FILES = 10;
const MAX_FILE_MB = 25;
// Fotos recodificadas aqui (perdem todos os metadados). HEIC só abre no Safari; nos outros
// navegadores vai como está e o servidor pede outro formato (em vez de repassar o GPS).
const REENCODE = ["image/png", "image/jpeg", "image/webp", "image/avif", "image/bmp", "image/heic", "image/heif"];
const EXECUTABLE = /\.(exe|msi|bat|cmd|com|scr|ps1|vbs|js|jar|apk|dmg|pkg|sh|lnk|reg|hta)$/i;

function formatBytes(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/**
 * Imagens: recodifica no navegador, aplica a rotação da foto e descarta TODOS os metadados
 * (EXIF/GPS etc.). GIF vai como está (para manter a animação; o servidor limpa). Outros
 * arquivos vão como estão (vídeos MP4/MOV têm a localização removida no servidor).
 */
async function prepareFile(file) {
  if (!REENCODE.includes(file.type)) return file;
  let bitmap;
  try {
    bitmap = await createImageBitmap(file, { imageOrientation: "from-image" });
  } catch {
    return file; // formato que o navegador não abre (ex.: HEIC fora do Safari)
  }
  return reencode(file, bitmap);
}

async function reencode(file, bitmap) {
  const scale = Math.min(1, 4096 / Math.max(bitmap.width, bitmap.height));
  const canvas = document.createElement("canvas");
  canvas.width = Math.round(bitmap.width * scale);
  canvas.height = Math.round(bitmap.height * scale);
  canvas.getContext("2d").drawImage(bitmap, 0, 0, canvas.width, canvas.height);
  bitmap.close?.();
  const type = ["image/png", "image/bmp"].includes(file.type) ? "image/png" : file.type === "image/webp" ? "image/webp" : "image/jpeg";
  return await new Promise((resolve, reject) =>
    canvas.toBlob((b) => (b ? resolve(b) : reject(new Error("falha ao processar"))), type, 0.9),
  );
}

function waitUploadToken() {
  return new Promise((resolve, reject) => {
    const w = { resolve, reject };
    composer.uploadWaiters.push(w);
    setTimeout(() => {
      const i = composer.uploadWaiters.indexOf(w);
      if (i >= 0) {
        composer.uploadWaiters.splice(i, 1);
        reject(new Error("sem resposta do servidor"));
      }
    }, 15000);
  });
}

/** Resposta do servidor a `upload_begin` (chegam na ordem dos pedidos). */
function onUploadToken(m) {
  composer.uploadWaiters.shift()?.resolve(m);
}

async function addFiles(files) {
  if (!state.channelId || currentChannel()?.kind === "voice") return;
  for (const file of files) {
    if (composer.uploads.length >= MAX_FILES) {
      toast(`No máximo ${MAX_FILES} arquivos por mensagem`);
      break;
    }
    if (file.size > MAX_FILE_MB * 1024 * 1024) {
      toast(`${file.name} tem ${formatBytes(file.size)}; o limite é ${MAX_FILE_MB} MB`);
      continue;
    }
    if (file.size === 0) {
      toast(`${file.name} está vazio`);
      continue;
    }
    const item = { key: Math.random().toString(36).slice(2), file, blob: null, url: "", status: "uploading", id: null };
    composer.uploads.push(item);
    renderUploads();
    try {
      item.blob = await prepareFile(file);
      if (item.blob.size > MAX_FILE_MB * 1024 * 1024) throw new Error(`passou de ${MAX_FILE_MB} MB`);
      if (item.blob.type.startsWith("image/")) item.url = URL.createObjectURL(item.blob);
      renderUploads();
      const tokenWait = waitUploadToken();
      send({ type: "upload_begin", channel_id: state.channelId, size: item.blob.size });
      const t = await tokenWait;
      const name = encodeURIComponent(file.name || "arquivo");
      const res = await fetch(`/anexos/${t.upload_id}?token=${encodeURIComponent(t.token)}&name=${name}`, {
        method: "POST",
        body: item.blob,
        headers: { "Content-Type": item.blob.type || "application/octet-stream" },
      });
      if (!res.ok) {
        const err = await res.json().catch(() => ({}));
        throw new Error(err.error || `HTTP ${res.status}`);
      }
      item.id = (await res.json()).id;
      item.status = "ready";
    } catch (e) {
      item.status = "error";
      toast(`Não foi possível enviar ${file.name || "o arquivo"}: ${e.message}`);
    }
    renderUploads();
  }
}

function renderUploads() {
  const box = $("#upload-chips");
  box.replaceChildren();
  box.classList.toggle("hidden", composer.uploads.length === 0);
  for (const u of composer.uploads) {
    const chip = el("div", `chip ${u.status}`);
    if (u.url) {
      const img = el("img");
      img.src = u.url;
      img.alt = u.file.name;
      chip.append(img);
    } else {
      const ext = (u.file.name.match(/\.([a-z0-9]{1,8})$/i)?.[1] ?? "").toUpperCase();
      const ph = el("div", "chip-ph");
      ph.append(icon(fileIconName(u.file.type, u.file.name), "chip-icon"), el("span", "chip-ext", ext || formatBytes(u.file.size)));
      chip.append(ph);
    }
    chip.append(el("span", "chip-name", u.status === "uploading" ? "Enviando…" : u.status === "error" ? "Falhou" : u.file.name));
    const rm = button("✕", "chip-remove", () => {
      URL.revokeObjectURL(u.url);
      composer.uploads = composer.uploads.filter((x) => x !== u);
      renderUploads();
    });
    rm.title = "Remover";
    rm.setAttribute("aria-label", `Remover ${u.file.name}`);
    chip.append(rm);
    box.append(chip);
  }
}

$("#attach-btn").onclick = () => $("#file-input").click();
$("#file-input").onchange = (e) => {
  addFiles([...e.target.files]);
  e.target.value = "";
};
input.addEventListener("paste", (e) => {
  const files = [...(e.clipboardData?.files ?? [])];
  if (files.length) {
    e.preventDefault();
    addFiles(files);
  }
});
$("#text-view").addEventListener("dragover", (e) => {
  if ([...(e.dataTransfer?.types ?? [])].includes("Files")) {
    e.preventDefault();
    $("#text-view").classList.add("dropping");
  }
});
$("#text-view").addEventListener("dragleave", (e) => {
  if (e.target === $("#text-view")) $("#text-view").classList.remove("dropping");
});
$("#text-view").addEventListener("drop", (e) => {
  e.preventDefault();
  $("#text-view").classList.remove("dropping");
  addFiles([...(e.dataTransfer?.files ?? [])]);
});

// ---------- partes da mensagem ----------
function replyPreviewEl(m) {
  const r = m.reply_to;
  const box = el("div", "reply-preview");
  box.append(el("span", "rp-line"));
  if (!r.author) {
    box.append(el("span", "rp-deleted", "Mensagem original apagada"));
    return box;
  }
  const a = avatar(r.author);
  a.classList.add("micro");
  const name = el("span", "rp-name", r.author.name);
  name.style.color = r.author.color;
  const text = el("span", "rp-text");
  text.append(renderContent(r.excerpt));
  box.append(a, name, text);
  box.onclick = () => {
    const target = document.querySelector(`[data-mid="${r.id}"]`);
    if (target) {
      target.scrollIntoView({ block: "center", behavior: "smooth" });
      target.classList.add("flash");
      setTimeout(() => target.classList.remove("flash"), 1200);
    } else {
      toast("A mensagem original está mais acima; role para carregá-la");
    }
  };
  return box;
}

function fileIconName(mime, name) {
  if (mime.startsWith("video/")) return "film";
  if (mime.startsWith("audio/")) return "music";
  if (/\.(zip|rar|7z|tar|gz|bz2|xz)$/i.test(name)) return "archive";
  if (EXECUTABLE.test(name)) return "alert";
  return "file";
}

/** Imagens em grade; vídeo e áudio com player; o resto vira um cartão para baixar. */
function attachmentsEl(m) {
  const wrap = el("div", "att-list");
  const images = m.attachments.filter((a) => a.mime.startsWith("image/"));
  if (images.length) {
    const box = el("div", images.length > 1 ? "attachments grid" : "attachments");
    for (const a of images) {
      const img = el("img", "att-img");
      img.src = a.url;
      img.alt = a.name;
      img.loading = "lazy";
      img.width = a.width;
      img.height = a.height;
      img.onclick = () => openLightbox(a);
      box.append(img);
    }
    wrap.append(box);
  }
  for (const a of m.attachments) {
    if (a.mime.startsWith("image/")) continue;
    if (a.mime.startsWith("video/")) {
      const v = el("video", "att-video");
      v.src = a.url;
      v.controls = true;
      v.preload = "metadata";
      v.playsInline = true;
      const card = el("div", "att-media");
      card.append(v, fileCard(a, true));
      wrap.append(card);
      continue;
    }
    if (a.mime.startsWith("audio/")) {
      const card = el("div", "att-media audio");
      const au = el("audio");
      au.src = a.url;
      au.controls = true;
      au.preload = "metadata";
      card.append(fileCard(a, true), au);
      wrap.append(card);
      continue;
    }
    wrap.append(fileCard(a));
  }
  return wrap;
}

function fileCard(a, compact) {
  const card = el("div", compact ? "file-card compact" : "file-card");
  card.append(icon(fileIconName(a.mime, a.name), "fc-icon"));
  const info = el("div", "fc-info");
  const link = el("a", "fc-name", a.name);
  link.href = a.url;
  link.download = a.name;
  link.rel = "noopener";
  info.append(link, el("span", "fc-size", formatBytes(a.size)));
  if (EXECUTABLE.test(a.name)) {
    info.append(el("span", "fc-warn", "Programa ou script: só abra se confiar em quem enviou."));
  }
  card.append(info);
  const dl = el("a", "icon-btn fc-dl");
  dl.href = a.url;
  dl.download = a.name;
  dl.title = "Baixar";
  dl.setAttribute("aria-label", `Baixar ${a.name}`);
  dl.append(icon("download"));
  card.append(dl);
  return card;
}

function reactionsEl(m) {
  const box = el("div", "reactions");
  for (const r of m.reactions) {
    const mine = r.users.includes(state.me?.id);
    const b = el("button", mine ? "reaction mine" : "reaction");
    b.type = "button";
    b.append(el("span", "r-emoji", r.emoji), el("span", "r-count", String(r.users.length)));
    const names = r.users.map((id) => userById(id)?.name ?? "alguém");
    b.title = `${names.slice(0, 10).join(", ")}${names.length > 10 ? ` e mais ${names.length - 10}` : ""} reagiu com ${r.emoji}`;
    b.setAttribute("aria-pressed", String(mine));
    b.onclick = () => send({ type: "react", channel_id: m.channel_id, message_id: m.id, emoji: r.emoji, on: !mine });
    box.append(b);
  }
  const add = el("button", "reaction add");
  add.type = "button";
  add.title = "Adicionar reação";
  add.setAttribute("aria-label", "Adicionar reação");
  add.append(icon("smile", "r-add-icon"));
  add.onclick = (e) => {
    e.stopPropagation();
    pickReaction(e.currentTarget, m);
  };
  box.append(add);
  return box;
}

function pickReaction(anchor, m) {
  openEmojiPicker(anchor, (emoji) => {
    const already = m.reactions.find((r) => r.emoji === emoji)?.users.includes(state.me?.id);
    send({ type: "react", channel_id: m.channel_id, message_id: m.id, emoji, on: !already });
  });
}

/** Barra de ações no hover (docs/DESIGN.md › Mensagens). */
function messageActionsEl(m, server) {
  const mine = m.author.id === state.me?.id;
  const bar = el("div", "msg-actions");
  const act = (name, title, fn, danger) => {
    const b = el("button", danger ? "danger" : null);
    b.type = "button";
    b.title = title;
    b.setAttribute("aria-label", title);
    b.append(icon(name, "ma-icon"));
    b.onclick = (e) => {
      e.stopPropagation();
      fn(e);
    };
    return b;
  };
  bar.append(act("smile", "Adicionar reação", (e) => pickReaction(e.currentTarget, m)));
  bar.append(act("reply", "Responder", () => setReply(m)));
  if (mine) bar.append(act("edit", "Editar", () => startEdit(m)));
  bar.append(act("more", "Mais opções", (e) => messageMenu(e, m, server)));
  return bar;
}

/** Menu da mensagem: botão "mais", clique direito ou toque longo (celular). */
function messageMenu(e, m, server) {
  const mine = m.author.id === state.me?.id;
  const anchor = e.currentTarget;
  showMenu(e, [
        { label: "Adicionar reação", icon: "smile", onClick: () => pickReaction(anchor, m) },
        { label: "Responder", icon: "reply", onClick: () => setReply(m) },
        mine && { label: "Editar mensagem", icon: "edit", onClick: () => startEdit(m) },
        { label: "Copiar texto", icon: "copy", onClick: () => copyText(plainText(m.content)) },
        "-",
        !mine && { label: "Denunciar mensagem", icon: "flag", danger: true, onClick: () => reportMessage(m) },
        (mine || server?.can_moderate) && {
          label: "Apagar mensagem",
          icon: "trash",
          danger: true,
          onClick: () => send({ type: "delete_message", channel_id: m.channel_id, message_id: m.id }),
        },
  ]);
}

/** Texto com as menções trocadas por @nome (para copiar e editar). */
function plainText(content) {
  return content.replace(/<@([0-9a-f-]{36})>/g, (_, id) => `@${userById(id)?.name ?? "desconhecido"}`);
}

// ---------- edição ----------
let editing = null; // id da mensagem em edição

function startEdit(m) {
  editing = m.id;
  renderMessages("keep");
}

/** Editor dentro da mensagem: Enter salva, Esc cancela. */
function editorEl(m) {
  const wrap = el("div", "msg-editor");
  const ta = el("textarea", "field");
  // Menções aparecem como @nome; ao salvar voltam a ser tokens.
  const map = new Map();
  ta.value = m.content.replace(/<@([0-9a-f-]{36})>/g, (_, id) => {
    const label = `@${userById(id)?.name ?? "desconhecido"}`;
    map.set(label, id);
    return label;
  });
  ta.maxLength = 2000;
  const save = () => {
    let content = ta.value.trim();
    for (const [label, id] of map) content = content.split(label).join(`<@${id}>`);
    editing = null;
    if (content && content !== m.content) {
      send({ type: "edit_message", channel_id: m.channel_id, message_id: m.id, content });
    }
    renderMessages("keep");
  };
  ta.onkeydown = (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      save();
    } else if (e.key === "Escape") {
      editing = null;
      renderMessages("keep");
    }
  };
  const hint = el("div", "editor-hint");
  hint.append("Esc para ", button("cancelar", "link-btn", () => { editing = null; renderMessages("keep"); }), " · Enter para ", button("salvar", "link-btn", save));
  wrap.append(ta, hint);
  setTimeout(() => {
    ta.focus();
    ta.selectionStart = ta.selectionEnd = ta.value.length;
    ta.style.height = `${Math.min(ta.scrollHeight + 2, 300)}px`;
  });
  return wrap;
}

// ---------- visualizador de imagem ----------
function openLightbox(a) {
  const box = $("#lightbox");
  box.replaceChildren();
  const img = el("img");
  img.src = a.url;
  img.alt = a.name;
  const bar = el("div", "lb-bar");
  const open = el("a", null, "Abrir original");
  open.href = a.url;
  open.target = "_blank";
  open.rel = "noopener";
  bar.append(el("span", null, `${a.name} · ${a.width}×${a.height} · ${(a.size / 1024).toFixed(0)} KB`), open);
  box.append(img, bar);
  show("#lightbox");
  box.onclick = (e) => { if (e.target === box) hide("#lightbox"); };
}
document.addEventListener("keydown", (e) => { if (e.key === "Escape") hide("#lightbox"); });
