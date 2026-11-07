// Teste de ponta a ponta do Concord (moderação, voz e chat). Uso: node tests/e2e.mjs [porta] | node tests/e2e.mjs --admin-id
import { createHash } from "node:crypto";
import zlib from "node:zlib";

const S = { a: "a".repeat(64), b: "b".repeat(64), c: "c".repeat(64), d: "d".repeat(64), e: "e".repeat(64) };
const idOf = (secret) => {
  const h = createHash("sha256").update("concord-user-id:" + secret).digest().subarray(0, 16).toString("hex");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
};
if (process.argv[2] === "--admin-id") {
  console.log(idOf(S.d));
  process.exit(0);
}

const port = process.argv[2] || 3999;
const base = `http://localhost:${port}`;
let failures = 0;
const check = (cond, label) => { console.log(`${cond ? "OK  " : "FAIL"} ${label}`); if (!cond) failures++; };
const type = (t) => (m) => m.type === t;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function client(origin) {
  const ws = new WebSocket(`ws://localhost:${port}/ws`, origin ? { headers: { Origin: origin } } : undefined);
  const inbox = [];
  const waiters = [];
  ws.onmessage = (e) => {
    const m = JSON.parse(e.data);
    const i = waiters.findIndex((w) => w.pred(m));
    if (i >= 0) waiters.splice(i, 1)[0].resolve(m); else inbox.push(m);
  };
  const closed = new Promise((r) => (ws.onclose = r));
  const opened = new Promise((r, j) => { ws.onopen = r; ws.onerror = j; });
  const c = {
    ws, opened, closed, inbox,
    send: (o) => ws.send(JSON.stringify(o)),
    wait(pred, ms = 3000) {
      const i = inbox.findIndex(pred);
      if (i >= 0) return Promise.resolve(inbox.splice(i, 1)[0]);
      return new Promise((resolve, reject) => {
        const w = { pred, resolve };
        waiters.push(w);
        setTimeout(() => { const k = waiters.indexOf(w); if (k >= 0) { waiters.splice(k, 1); reject(new Error("timeout")); } }, ms);
      });
    },
    async none(pred, ms = 600) {
      try { await c.wait(pred, ms); return false; } catch { return true; }
    },
    async error() { return (await c.wait(type("error"))).message; },
  };
  return c;
}

async function login(secret, name) {
  const c = client();
  await c.opened;
  c.send({ type: "hello", secret, name, color: "#123456", consent_version: 6 });
  c.ready = await c.wait(type("ready"));
  c.id = c.ready.me.id;
  return c;
}

// ---------- básico ----------
const res = await fetch(base + "/");
check(res.headers.get("content-security-policy")?.includes("default-src 'self'"), "CSP presente");
const pol = await (await fetch(base + "/privacidade")).text();
check(pol.includes("Versão 6") && pol.includes("Teste &lt;Ltda&gt;") && !pol.includes("{{"), "política v6 renderizada e escapada");
check((await fetch(base + "/convite/abc123XYZ")).status === 200, "rota /convite/:code entrega o app");

let c0 = client(); await c0.opened;
c0.send({ type: "hello", secret: S.a, name: "Ana", color: "#ff0000", consent_version: 1 });
check((await c0.wait(type("consent_required"))).version === 6, "aceite antigo não vale mais: pede consentimento da v6");
await c0.closed;

let blocked = false;
try { await client("https://malicioso.com").opened; } catch { blocked = true; }
check(blocked, "conexão de outra origem é recusada");

// ---------- servidor privado ----------
const A = await login(S.a, "Ana");
const B = await login(S.b, "Bia");
const C = await login(S.c, "Caio");
const D = await login(S.d, "Admin");
const lobby = A.ready.servers.find((s) => s.name === "Concord");
check(lobby && lobby.public && lobby.owner_id === null, "servidor padrão é público e sem dono");
check(!lobby.can_moderate && D.ready.is_admin && D.ready.servers.find((s) => s.id === lobby.id).can_moderate, "admin modera o servidor padrão; pessoa comum não");

A.send({ type: "create_server", name: "Privado", public: false });
const focus = await A.wait(type("focus"));
const priv = (await A.wait((m) => m.type === "server_updated" && m.server.id === focus.server_id)).server;
check(priv.role === "owner" && priv.can_manage && priv.invite_code?.length === 12 && !priv.public, "dono recebe o servidor privado com convite");
check(await B.none((m) => m.type === "server_updated" && m.server.name === "Privado"), "quem não foi convidado não fica sabendo do servidor");
const ch = priv.channels[0].id;

A.send({ type: "send_message", channel_id: ch, content: "segredo" });
await A.wait(type("message"));
check(await B.none(type("message")), "mensagem de servidor privado não vaza");
B.send({ type: "fetch_history", channel_id: ch });
check((await B.error()) === "Você não tem permissão para isso", "não membro não lê o histórico");
B.send({ type: "send_message", channel_id: ch, content: "invasão" });
check((await B.error()) === "Você não tem permissão para isso", "não membro não envia mensagem");

B.send({ type: "join_server", code: "naoexiste123" });
check((await B.error()).includes("inválido"), "convite inválido é recusado");
B.send({ type: "join_server", code: priv.invite_code });
const bJoin = (await B.wait((m) => m.type === "server_updated" && m.server.id === priv.id)).server;
check(bJoin.role === "member" && !bJoin.can_moderate && bJoin.invite_code === null && bJoin.bans === null, "membro entra sem ver convite nem banidos");
const names = (bJoin.members ?? []).map((u) => u.name).sort().join(",");
check(names === "Ana,Bia", "servidor privado lista seus membros (para online/offline)");
check(lobby.members === null, "servidor público não expõe lista de membros");

// ---------- permissões ----------
B.send({ type: "create_channel", server_id: priv.id, name: "x" });
check((await B.error()) === "Você não tem permissão para isso", "membro não cria canal");
B.send({ type: "fetch_history", channel_id: ch });
const hist = await B.wait(type("history"));
B.send({ type: "delete_message", channel_id: ch, message_id: hist.messages[0].id });
check((await B.error()) === "Você não tem permissão para isso", "membro não apaga mensagem alheia");
B.send({ type: "set_moderator", server_id: priv.id, user_id: B.id, enabled: true });
check((await B.error()) === "Você não tem permissão para isso", "membro não se promove a moderador");

A.send({ type: "set_moderator", server_id: priv.id, user_id: B.id, enabled: true });
const bMod = (await B.wait((m) => m.type === "server_updated" && m.server.can_moderate)).server;
check(bMod.role === "moderator" && !!bMod.invite_code && !bMod.can_manage, "dono promove moderador (sem poderes de dono)");

C.send({ type: "join_server", code: bMod.invite_code });
await C.wait((m) => m.type === "server_updated" && m.server.id === priv.id);
C.send({ type: "send_message", channel_id: ch, content: "spam spam spam" });
const cMsg = (await C.wait((m) => m.type === "message" && m.message.author.id === C.id)).message;
B.send({ type: "delete_message", channel_id: ch, message_id: cMsg.id });
check((await A.wait((m) => m.type === "message_deleted" && m.message_id === cMsg.id)) != null, "moderador apaga mensagem alheia");

// ---------- denúncias ----------
C.send({ type: "send_message", channel_id: ch, content: "o CPF do fulano é 123" });
const cMsg2 = (await C.wait((m) => m.type === "message" && m.message.author.id === C.id)).message;
A.send({ type: "report_message", channel_id: ch, message_id: cMsg2.id, category: "dados_pessoais", details: "expôs dados" });
await A.wait(type("info"));
const reps = await B.wait((m) => m.type === "reports" && m.reports.length === 1);
const raw = JSON.stringify(reps);
check(!raw.includes("reporter") && !raw.includes(A.id), "moderador vê a denúncia sem saber quem denunciou");
check(await C.none((m) => m.type === "reports"), "membro comum não recebe denúncias");
A.send({ type: "report_message", channel_id: ch, message_id: cMsg2.id, category: "spam" });
check((await A.error()).includes("já denunciou"), "denúncia duplicada é recusada");
C.send({ type: "report_message", channel_id: ch, message_id: cMsg2.id, category: "spam" });
check((await C.error()).includes("própria"), "não dá para denunciar a própria mensagem");
B.send({ type: "resolve_report", report_id: reps.reports[0].id, delete_message: true });
await A.wait((m) => m.type === "message_deleted" && m.message_id === cMsg2.id);
check((await B.wait((m) => m.type === "reports" && m.reports.length === 0)) != null, "resolver denúncia apaga a mensagem e limpa a fila");

// ---------- banimento ----------
await sleep(2500); // B já fez muitas ações; respeita o limite de taxa (≈3/s)
B.send({ type: "ban_user", server_id: priv.id, user_id: A.id, reason: "teste" });
check((await B.error()).includes("dono"), "moderador não bane o dono");
B.send({ type: "ban_user", server_id: priv.id, user_id: C.id, reason: "exposição de dados pessoais", delete_messages: true });
const info = await C.wait(type("info"));
check(info.message.includes("Privado") && info.message.includes("exposição de dados pessoais"), "banido é avisado do servidor e do motivo");
check((await C.wait((m) => m.type === "server_removed" && m.server_id === priv.id)) != null, "servidor some para o banido");
C.send({ type: "fetch_history", channel_id: ch });
check((await C.error()) === "Você não tem permissão para isso", "banido não lê mais nada");
C.send({ type: "join_server", code: bMod.invite_code });
check((await C.error()).includes("não pode entrar"), "banido não volta pelo convite");
const bansView = (await B.wait((m) => m.type === "server_updated" && m.server.bans?.length === 1)).server.bans[0];
check(bansView.name === "Caio" && bansView.reason === "exposição de dados pessoais", "moderador vê a lista de banidos");

C.send({ type: "export_my_data" });
const cExp = (await C.wait(type("my_data"))).data;
check(cExp.banimentos_recebidos.length === 1 && cExp.registro_de_moderacao.some((e) => e.meu_papel === "alvo da ação"),
  "exportação do banido traz o banimento e as ações sobre ele");
A.send({ type: "export_my_data" });
const aExp = (await A.wait(type("my_data"))).data;
check(aExp.denuncias_feitas.length === 0 && aExp.servidores.some((s) => s.papel === "owner"),
  "exportação do dono traz o servidor; denúncia resolvida já não existe");

// ---------- chat: respostas, edição, reações e imagens ----------
// PNG 2x1 válido com um bloco de texto (metadado) que precisa ser removido no envio.
function pngWithMetadata() {
  const chunk = (t, data) => {
    const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
    const td = Buffer.concat([Buffer.from(t, "ascii"), data]);
    const crc = Buffer.alloc(4); crc.writeUInt32BE(zlib.crc32(td));
    return Buffer.concat([len, td, crc]);
  };
  const ihdr = Buffer.alloc(13); ihdr.writeUInt32BE(2, 0); ihdr.writeUInt32BE(1, 4); ihdr[8] = 8; ihdr[9] = 2;
  const raw = Buffer.from([0, 255, 0, 0, 0, 0, 255]);
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk("IHDR", ihdr),
    chunk("tEXt", Buffer.from("Location\0GPS-SEGREDO -23.5,-46.6")),
    chunk("IDAT", zlib.deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}
async function beginUpload(c, channel_id, size) {
  c.send({ type: "upload_begin", channel_id, size });
  return c.wait(type("upload_token"));
}
const post = (g, body, name = "foto.png") =>
  fetch(`${base}/anexos/${g.upload_id}?token=${encodeURIComponent(g.token)}&name=${encodeURIComponent(name)}`, { method: "POST", body });

let attUrl;
{
  check((await fetch(base + "/favicon.svg")).headers.get("content-type")?.startsWith("image/svg+xml"), "favicon servido");

  A.send({ type: "send_message", channel_id: ch, content: "pergunta original" });
  const orig = (await B.wait((m) => m.type === "message" && m.message.content === "pergunta original")).message;
  check(orig.reactions.length === 0 && orig.attachments.length === 0 && orig.edited_at === null, "mensagem nova vem sem reações, imagens nem edição");

  // resposta
  B.send({ type: "send_message", channel_id: ch, content: "resposta", reply_to: orig.id });
  const rep = (await A.wait((m) => m.type === "message" && m.message.content === "resposta")).message;
  check(rep.reply_to?.id === orig.id && rep.reply_to.author?.name === "Ana" && rep.reply_to.excerpt === "pergunta original",
    "resposta traz a prévia da mensagem original");
  B.send({ type: "send_message", channel_id: ch, content: "x", reply_to: "0190a8b0-0000-7000-8000-000000000000" });
  check((await B.error()).includes("não existe"), "resposta a mensagem inexistente é recusada");

  // edição
  B.send({ type: "edit_message", channel_id: ch, message_id: orig.id, content: "hackeado" });
  check((await B.error()) === "Só quem escreveu pode editar", "ninguém edita mensagem alheia (nem moderador)");
  A.send({ type: "edit_message", channel_id: ch, message_id: orig.id, content: "   " });
  check((await A.error()).includes("vazia"), "edição não deixa a mensagem vazia");
  A.send({ type: "edit_message", channel_id: ch, message_id: orig.id, content: "pergunta corrigida" });
  const ed = await B.wait((m) => m.type === "message_edited" && m.message_id === orig.id);
  check(ed.content === "pergunta corrigida" && ed.edited_at > 0, "edição chega para todos com a marca de editada");

  // reações
  B.send({ type: "react", channel_id: ch, message_id: orig.id, emoji: "👍", on: true });
  const ru = await A.wait((m) => m.type === "reaction_update" && m.message_id === orig.id);
  check(ru.emoji === "👍" && ru.user_id === B.id && ru.on, "reação chega para o servidor");
  B.send({ type: "react", channel_id: ch, message_id: orig.id, emoji: "👍", on: true });
  check(await A.none((m) => m.type === "reaction_update" && m.message_id === orig.id, 400), "reação repetida não gera evento");
  await sleep(3400); // o limite é 10 ações em rajada e 3/s; o teste respeita
  for (const bad of ["abc", "<script>", "👍 👍", ""]) {
    B.send({ type: "react", channel_id: ch, message_id: orig.id, emoji: bad, on: true });
    check((await B.error()) === "Reação inválida", `reação inválida recusada: ${JSON.stringify(bad)}`);
  }
  await sleep(3400);
  for (let k = 0; k < 19; k++) { await sleep(340); A.send({ type: "react", channel_id: ch, message_id: orig.id, emoji: String.fromCodePoint(0x1f600 + k), on: true }); }
  for (let k = 0; k < 19; k++) await A.wait((m) => m.type === "reaction_update" && m.user_id === A.id);
  A.send({ type: "react", channel_id: ch, message_id: orig.id, emoji: "🦀", on: true });
  check((await A.error()).includes("Limite de 20"), "limite de 20 reações diferentes por mensagem");
  A.send({ type: "react", channel_id: ch, message_id: orig.id, emoji: "👍", on: true });
  check((await B.wait((m) => m.type === "reaction_update" && m.user_id === A.id && m.emoji === "👍")).on, "emoji já existente ainda aceita mais gente");
  C.send({ type: "react", channel_id: ch, message_id: orig.id, emoji: "👍", on: true });
  check((await C.error()) === "Você não tem permissão para isso", "banido não reage");

  // imagens
  await sleep(3400);
  C.send({ type: "upload_begin", channel_id: ch, size: 100 });
  check((await C.error()) === "Você não tem permissão para isso", "banido não recebe autorização de envio");
  B.send({ type: "upload_begin", channel_id: ch, size: 50 * 1024 * 1024 });
  check((await B.error()).includes("MB"), "imagem grande demais é recusada antes do envio");

  const png = pngWithMetadata();
  const g1 = await beginUpload(B, ch, png.length);
  check((await post({ ...g1, token: "x".repeat(g1.token.length) }, png)).status === 403, "envio com autorização falsa é recusado");
  const g2 = await beginUpload(B, ch, png.length);
  const up = await post(g2, png, "../../férias\u0007.jpeg");
  const upJ = await up.json();
  check(up.status === 201 && upJ.width === 2 && upJ.height === 1 && upJ.name === "férias.png", "imagem aceita, com nome limpo e extensão real");
  check((await post(g2, png)).status === 403, "autorização de envio é de uso único");
  // Outros arquivos: HTML vira download protegido, GIF sem o byte final é aceito,
  // MP4 perde a localização e as datas, foto HEIC (com GPS) é recusada.
  const html = Buffer.from("<html><script>alert(1)</script>");
  const g3 = await beginUpload(B, ch, html.length);
  const r3 = await post(g3, html, "pagina.html");
  const j3 = await r3.json();
  check(r3.status === 201 && j3.mime === "application/octet-stream" && j3.name === "pagina.html", "arquivo qualquer é aceito, com tipo neutro");
  const gif = Buffer.from([0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 1, 0, 1, 0, 0x80, 0, 0, 255, 255, 255, 0, 0, 0,
    0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 1, 0]); // sem o 0x3B final
  const g4 = await beginUpload(B, ch, gif.length);
  const r4 = await post(g4, gif, "meme.gif");
  const j4 = await r4.json();
  check(r4.status === 201 && j4.mime === "image/gif", "GIF sem o byte final (comum na internet) é aceito");
  const box = (kind, body) => { const h = Buffer.alloc(8); h.writeUInt32BE(body.length + 8); h.write(kind, 4, "latin1"); return Buffer.concat([h, body]); };
  const mvhd = Buffer.alloc(100); mvhd.fill(0xAB, 4, 12);
  const mp4 = Buffer.concat([
    box("ftyp", Buffer.from("isom\0\0\0\0isommp41", "latin1")),
    box("moov", Buffer.concat([box("mvhd", mvhd), box("udta", box("\u00a9xyz", Buffer.from("+23.5503-046.6339/")))])),
    box("mdat", Buffer.alloc(2000, 7)),
  ]);
  const g5 = await beginUpload(B, ch, mp4.length);
  const r5 = await post(g5, mp4, "video.mp4");
  const j5 = await r5.json();
  check(r5.status === 201 && j5.mime === "video/mp4", "vídeo MP4 aceito");
  const heic = box("ftyp", Buffer.from("heic\0\0\0\0mif1heic", "latin1"));
  const g6 = await beginUpload(B, ch, heic.length);
  check((await post(g6, heic, "foto.heic")).status === 415, "foto HEIC (que guarda GPS) é recusada com orientação");

  // GIF colado: o servidor busca o original, mas nunca na rede interna (SSRF).
  await sleep(3400);
  const fromUrl = async (url) => {
    const rid = Math.random().toString(36).slice(2);
    B.send({ type: "upload_from_url", channel_id: ch, url, request_id: rid });
    return B.wait((m) => m.type === "url_upload" && m.request_id === rid, 30000);
  };
  for (const bad of ["https://127.0.0.1/a.gif", "https://169.254.169.254/latest/meta-data/", "https://10.0.0.1/a.gif",
    "https://[::1]/a.gif", "https://localhost/a.gif", "http://upload.wikimedia.org/a.gif", "file:///etc/passwd"]) {
    const r = await fromUrl(bad);
    check(!r.ok && !r.id, `busca de GIF recusa endereço interno ou inseguro: ${bad}`);
  }
  const gifUrl = "https://upload.wikimedia.org/wikipedia/commons/2/2c/Rotating_earth_%28large%29.gif";
  const remote = await fromUrl(gifUrl);
  if (!remote.ok && /buscar/.test(remote.error ?? "")) {
    console.log("AVISO sem acesso à internet; busca de GIF real pulada");
  } else {
    check(remote.ok && remote.mime === "image/gif" && remote.width > 0 && remote.name.endsWith(".gif"),
      `GIF colado vira o GIF original (${remote.name}, ${remote.size} bytes)`);
  }
  const notImage = await fromUrl("https://upload.wikimedia.org/wikipedia/commons/");
  check(!notImage.ok, "endereço que não é imagem é recusado");

  A.send({ type: "send_message", channel_id: ch, content: "roubando", attachments: [upJ.id] });
  check((await A.error()).includes("Arquivo inválido"), "não dá para anexar o arquivo enviado por outra pessoa");
  B.send({ type: "send_message", channel_id: ch, content: "", attachments: [upJ.id, j3.id, j4.id, j5.id] });
  const withImg = (await A.wait((m) => m.type === "message" && m.message.attachments.length === 4)).message;
  attUrl = withImg.attachments[0].url;
  const byId = (id) => withImg.attachments.find((a) => a.id === id);
  const dl = await fetch(base + byId(j3.id).url);
  check(dl.headers.get("content-type") === "application/octet-stream" && dl.headers.get("content-disposition")?.startsWith("attachment")
    && dl.headers.get("content-security-policy")?.includes("sandbox"), "HTML enviado é só baixado, nunca aberto como página do Concord");
  const vid = await fetch(base + byId(j5.id).url);
  const vidBytes = Buffer.from(await vid.arrayBuffer());
  check(vid.headers.get("content-type") === "video/mp4" && vid.headers.get("content-disposition")?.startsWith("inline")
    && !vidBytes.includes("+23.5503") && !vidBytes.includes("udta") && !vidBytes.includes(Buffer.alloc(4, 0xAB)) && vidBytes.length === mp4.length,
    "vídeo sem localização nem data de gravação, e do mesmo tamanho (continua tocando)");
  const part = await fetch(base + byId(j5.id).url, { headers: { Range: "bytes=0-9" } });
  check(part.status === 206 && (await part.arrayBuffer()).byteLength === 10 && part.headers.get("content-range") === `bytes 0-9/${mp4.length}`,
    "vídeo aceita pedidos parciais (avançar e voltar)");
  check(withImg.attachments[0].mime === "image/png" && /^\/anexos\/[0-9a-f-]+\?exp=\d+&sig=/.test(attUrl), "mensagem só com imagem leva link assinado");
  const img = await fetch(base + attUrl);
  const bytes = Buffer.from(await img.arrayBuffer());
  check(img.status === 200 && img.headers.get("content-type") === "image/png" && img.headers.get("cache-control")?.startsWith("private"),
    "imagem é entregue com cache privado");
  check(!bytes.includes("GPS-SEGREDO") && bytes.subarray(1, 4).toString() === "PNG", "metadados removidos da imagem guardada");
  check((await fetch(base + attUrl.replace(/sig=.{4}/, "sig=AAAA"))).status === 404, "link com assinatura adulterada não abre");
  check((await fetch(base + attUrl.replace(/exp=\d+/, "exp=9999999999999"))).status === 404, "não dá para estender a validade do link");

  // histórico e exportação
  await sleep(3400);
  B.send({ type: "fetch_history", channel_id: ch });
  const h = await B.wait(type("history"));
  const hOrig = h.messages.find((m) => m.id === orig.id);
  check(hOrig.content === "pergunta corrigida" && hOrig.edited_at && hOrig.reactions.find((r) => r.emoji === "👍")?.users.length === 2,
    "histórico traz edição e reações");
  check(h.messages.some((m) => m.attachments[0]?.id === upJ.id && m.attachments[0].url.includes("sig=")), "histórico traz imagens com link assinado");
  B.send({ type: "export_my_data" });
  const bExp = (await B.wait(type("my_data"))).data;
  check(bExp.reacoes.some((r) => r.emoji === "👍") && bExp.arquivos.length >= 4 && bExp.mensagens.some((m) => m.resposta_a),
    "exportação traz reações, arquivos e respostas");
}

// ---------- paginação do histórico (banco de dados) ----------
{
  const lobbyCh = lobby.channels[0].id;
  const senders = [];
  for (let k = 0; k < 6; k++) senders.push(await login(createHash("sha256").update("pag" + k).digest("hex"), `P${k}`));
  for (const [k, s] of senders.entries()) {
    for (let j = 0; j < 10; j++) s.send({ type: "send_message", channel_id: lobbyCh, content: `pag ${k}-${j}` });
  }
  await sleep(1500);
  D.send({ type: "fetch_history", channel_id: lobbyCh });
  const p1 = await D.wait((m) => m.type === "history" && !m.before);
  check(p1.messages.length === 50 && p1.has_more, "primeira página traz 50 mensagens e avisa que há mais");
  D.send({ type: "fetch_history", channel_id: lobbyCh, before: p1.messages[0].id });
  const p2 = await D.wait((m) => m.type === "history" && m.before);
  const older = p2.messages.every((m) => m.id < p1.messages[0].id);
  check(p2.messages.length === 10 && !p2.has_more && older, "página anterior traz o restante, em ordem, sem repetir");
  for (const s of senders) s.ws.close();
}

// ---------- voz ----------
if (!A.ready.voice_enabled) {
  console.log("AVISO voz desativada nesta instância (sem LiveKit); testes de voz pulados");
} else {
  const textCh = priv.channels.find((c) => c.kind === "text").id;
  const voiceCh = priv.channels.find((c) => c.kind === "voice");
  check(voiceCh && voiceCh.bitrate === 64 && voiceCh.user_limit === 0, "servidor novo já vem com canal de voz");
  A.send({ type: "voice_join", channel_id: textCh });
  check((await A.error()).includes("não é um canal de voz"), "não entra em canal de texto como voz");
  C.send({ type: "voice_join", channel_id: voiceCh.id });
  check((await C.error()) === "Você não tem permissão para isso", "banido/não membro não recebe token de voz");

  A.send({ type: "voice_join", channel_id: voiceCh.id });
  const tok = await A.wait(type("voice_token"));
  const lkUrl = (tok.url ?? `ws://localhost:${tok.port}`).replace(/^ws/, "http");
  const v = await fetch(`${lkUrl}/rtc/validate?access_token=${encodeURIComponent(tok.token)}`);
  check(v.status === 200, `LiveKit aceita o token emitido pelo Concord (HTTP ${v.status})`);
  const bad = await fetch(`${lkUrl}/rtc/validate?access_token=${encodeURIComponent(tok.token.slice(0, -4) + "AAAA")}`);
  check(bad.status === 401, `LiveKit recusa token adulterado (HTTP ${bad.status})`);

  const vs = await B.wait((m) => m.type === "voice_states" && m.states.some((s) => s.user.id === A.id));
  check(vs.states[0].channel_id === voiceCh.id, "membros veem quem está no canal de voz");
  check(await D.none((m) => m.type === "voice_states" && m.server_id === priv.id, 300), "quem não é do servidor (nem o admin) não vê a voz dele");

  A.send({ type: "voice_update", muted: true, deafened: false, streaming: true, camera: false });
  const vu = await B.wait((m) => m.type === "voice_states" && m.states.some((s) => s.muted && s.streaming));
  check(vu != null, "mutado e \"ao vivo\" aparecem para os outros");

  // B é moderador: pode definir o status mesmo fora do canal.
  B.send({ type: "set_voice_status", channel_id: voiceCh.id, status: "aviso da moderação" });
  const modStatus = await B.wait((m) => m.type === "server_updated" && m.server.channels.some((c) => c.status === "aviso da moderação"));
  check(modStatus != null, "moderador define status do canal de voz");
  A.send({ type: "set_voice_status", channel_id: voiceCh.id, status: "jogando xadrez" });
  const st1 = await B.wait((m) => m.type === "server_updated" && m.server.channels.some((c) => c.status === "jogando xadrez"));
  check(!JSON.stringify(st1).includes("status_by"), "status do canal aparece, sem dizer quem definiu");

  A.send({ type: "update_channel", channel_id: voiceCh.id, user_limit: 1, bitrate: 128 });
  await A.wait((m) => m.type === "server_updated" && m.server.channels.some((c) => c.user_limit === 1 && c.bitrate === 128));
  A.send({ type: "update_channel", channel_id: voiceCh.id, bitrate: 999 });
  check((await A.error()).includes("8 a 384"), "taxa de bits fora da faixa é recusada");

  A.send({ type: "voice_leave" });
  const cleared = await B.wait((m) => m.type === "server_updated" && m.server.channels.some((c) => c.id === voiceCh.id && c.status === null));
  check(cleared != null, "status some quando o canal de voz esvazia (minimização)");
  await sleep(2500);

  // ---------- moderação de voz ----------
  const jwtSources = (t) => JSON.parse(Buffer.from(t.split(".")[1], "base64url")).video.canPublishSources;
  A.send({ type: "update_channel", channel_id: voiceCh.id, user_limit: 0 });
  B.send({ type: "voice_join", channel_id: voiceCh.id });
  const bTok = await B.wait(type("voice_token"));
  check(jwtSources(bTok.token).includes("microphone"), "token normal permite publicar o microfone");

  B.send({ type: "voice_moderate", server_id: priv.id, user_id: A.id, action: "mute" });
  check((await B.error()).includes("dono"), "moderador não silencia o dono");
  A.send({ type: "voice_moderate", server_id: priv.id, user_id: B.id, action: "mute" });
  check((await B.wait(type("info"))).message.includes("silenciou"), "silenciado é avisado");
  const mutedSt = await B.wait((m) => m.type === "voice_states" && m.states.some((s) => s.user.id === B.id && s.server_muted));
  check(mutedSt != null, "todos veem o ícone de silenciado pela moderação");
  B.send({ type: "voice_join", channel_id: voiceCh.id }); // reentrar não burla o silêncio
  const bTok2 = await B.wait(type("voice_token"));
  check(!jwtSources(bTok2.token).includes("microphone"), "token de quem foi silenciado não permite microfone");

  A.send({ type: "create_channel", server_id: priv.id, name: "Sala 2", kind: "voice" });
  const sala2 = (await A.wait((m) => m.type === "server_updated" && m.server.channels.some((c) => c.name === "Sala 2"))).server
    .channels.find((c) => c.name === "Sala 2");
  A.send({ type: "voice_move", user_id: B.id, channel_id: sala2.id });
  const moved = await B.wait((m) => m.type === "voice_token" && m.moved);
  check(moved.channel_id === sala2.id && !jwtSources(moved.token).includes("microphone"), "mover entrega token do novo canal (e mantém o silêncio)");
  await B.wait((m) => m.type === "voice_states" && m.states.some((s) => s.user.id === B.id && s.channel_id === sala2.id));
  B.inbox.length = 0; // descarta mensagens antigas da fila
  B.send({ type: "voice_leave", channel_id: voiceCh.id }); // saída atrasada do canal antigo
  check(await B.none((m) => m.type === "voice_states" && m.server_id === priv.id && !m.states.some((s) => s.user.id === B.id), 600),
    "saída atrasada do canal antigo não tira a pessoa do canal novo");

  await sleep(2000);
  A.send({ type: "voice_moderate", server_id: priv.id, user_id: B.id, action: "unmute" });
  await B.wait((m) => m.type === "voice_states" && m.states.some((s) => s.user.id === B.id && !s.server_muted));
  A.send({ type: "voice_moderate", server_id: priv.id, user_id: B.id, action: "disconnect" });
  check((await B.wait(type("voice_ended"))).reason.includes("desconectou"), "desconectado pelo moderador é avisado");
  const gone = await A.wait((m) => m.type === "voice_states" && !m.states.some((s) => s.user.id === B.id));
  check(gone != null, "desconectado some da chamada");
  await sleep(2000);
}

// ---------- Soundpad ----------
function wavFile(seconds, withMeta = true) {
  const rate = 48000, n = Math.round(seconds * rate);
  const meta = withMeta ? Buffer.concat([Buffer.from("LIST"), Buffer.from([16, 0, 0, 0]), Buffer.from("INFOIART"), Buffer.from([4, 0, 0, 0]), Buffer.from("Ana\0")]) : Buffer.alloc(0);
  const fmt = Buffer.alloc(24);
  fmt.write("fmt ", 0); fmt.writeUInt32LE(16, 4); fmt.writeUInt16LE(1, 8); fmt.writeUInt16LE(1, 10);
  fmt.writeUInt32LE(rate, 12); fmt.writeUInt32LE(rate * 2, 16); fmt.writeUInt16LE(2, 20); fmt.writeUInt16LE(16, 22);
  const data = Buffer.alloc(8 + n * 2);
  data.write("data", 0); data.writeUInt32LE(n * 2, 4);
  for (let i = 0; i < n; i++) data.writeInt16LE(Math.round(Math.sin(i / 20) * 8000), 8 + i * 2);
  const body = Buffer.concat([Buffer.from("WAVE"), meta, fmt, data]);
  const head = Buffer.alloc(8); head.write("RIFF", 0); head.writeUInt32LE(body.length, 4);
  return Buffer.concat([head, body]);
}
const postSound = (g, body) => fetch(`${base}/sons/${g.upload_id}?token=${encodeURIComponent(g.token)}`, { method: "POST", body });
async function soundToken(c, name, size, emoji) {
  c.send({ type: "sound_upload_begin", server_id: priv.id, size, name, emoji });
  return c.wait((m) => m.type === "upload_token" && m.kind === "sound");
}
let keptSoundUrl;
{
  await sleep(3400);
  C.send({ type: "sound_upload_begin", server_id: priv.id, size: 1000, name: "x" });
  check((await C.error()) === "Você não tem permissão para isso", "Soundpad: quem não é do servidor não envia som");
  B.send({ type: "sound_upload_begin", server_id: priv.id, size: 1000, name: "x", emoji: "abc" });
  check((await B.error()) === "Emoji inválido", "Soundpad: emoji inválido recusado");

  const w = wavFile(1);
  const g1 = await soundToken(B, "Risada", w.length, "😂");
  const r1 = await postSound(g1, w);
  const j1 = await r1.json();
  check(r1.status === 201 && j1.duration_ms === 1000, "Soundpad: moderador envia som WAV");
  await A.wait((m) => m.type === "sounds_changed" && m.server_id === priv.id);
  check(true, "Soundpad: servidor avisa que a lista mudou");

  const gImg = await beginUpload(B, ch, 100);
  check((await postSound(gImg, w)).status === 403, "Soundpad: autorização de imagem não serve para som");
  const g2 = await soundToken(B, "Longo", 700000);
  check((await postSound(g2, wavFile(6, false))).status === 415, "Soundpad: som com mais de 5 s é recusado");
  const g3 = await soundToken(B, "Falso", 100);
  check((await postSound(g3, Buffer.from("ID3\u0003 isto seria um mp3"))).status === 415, "Soundpad: arquivo que não é WAV PCM é recusado");
  const g4 = await soundToken(B, "Mantido", w.length);
  await postSound(g4, wavFile(0.5));

  A.send({ type: "fetch_sounds", server_id: priv.id });
  const list = await A.wait((m) => m.type === "sounds" && m.server_id === priv.id);
  const ris = list.sounds.find((s) => s.name === "Risada");
  check(list.can_manage && list.sounds.length === 2 && ris?.emoji === "😂" && !JSON.stringify(list).includes("uploader"),
    "Soundpad: lista traz os sons (sem dizer quem enviou)");
  keptSoundUrl = list.sounds.find((s) => s.name === "Mantido").url;
  const got = await fetch(base + ris.url);
  const gotBytes = Buffer.from(await got.arrayBuffer());
  check(got.status === 200 && got.headers.get("content-type") === "audio/wav" && !gotBytes.includes("LIST") && !gotBytes.includes("Ana"),
    "Soundpad: som entregue sem metadados");
  check((await fetch(base + ris.url.replace("/sons/", "/anexos/"))).status === 404, "Soundpad: link de som não abre como imagem");
  check((await fetch(base + ris.url.replace(/sig=.{4}/, "sig=AAAA"))).status === 404, "Soundpad: link adulterado não abre");

  B.send({ type: "export_my_data" });
  const bx = (await B.wait(type("my_data"))).data;
  check(bx.sons_do_soundpad.length === 2 && bx.sons_do_soundpad[0].link_para_baixar, "Soundpad: exportação traz os sons enviados");

  await sleep(3400);
  A.send({ type: "play_sound", sound: "builtin:buzina" });
  check((await A.error()).includes("canal de voz"), "Soundpad: fora da voz não toca");
  if (A.ready.voice_enabled) {
    const vc = priv.channels.find((c) => c.kind === "voice").id;
    A.send({ type: "voice_join", channel_id: vc });
    await A.wait(type("voice_token"));
    A.send({ type: "play_sound", sound: "builtin:buzina" });
    const sp1 = await B.wait((m) => m.type === "sound_played");
    check(sp1.user_id === A.id && sp1.channel_id === vc && sp1.sound === "builtin:buzina", "Soundpad: som padrão chega para o servidor");
    A.send({ type: "play_sound", sound: ris.id });
    check(await B.none((m) => m.type === "sound_played", 500), "Soundpad: intervalo mínimo entre sons");
    await sleep(1200);
    A.send({ type: "play_sound", sound: ris.id });
    check((await B.wait((m) => m.type === "sound_played")).sound === ris.id, "Soundpad: som do servidor toca");
    A.send({ type: "play_sound", sound: "builtin:naoexiste" });
    check((await A.error()) === "Som não encontrado", "Soundpad: som inexistente recusado");
    A.send({ type: "voice_leave" });
    await sleep(500);
  }
  A.send({ type: "delete_sound", sound_id: ris.id });
  await B.wait((m) => m.type === "sounds_changed");
  check((await fetch(base + ris.url)).status === 404, "Soundpad: som apagado some");
}

// ---------- convite, admin, eliminação ----------
A.send({ type: "regenerate_invite", server_id: priv.id });
const newInv = (await A.wait((m) => m.type === "server_updated" && m.server.invite_code !== priv.invite_code)).server.invite_code;
const E = await login(S.e, "Edu");
E.send({ type: "join_server", code: priv.invite_code });
check((await E.error()).includes("inválido"), "convite antigo deixa de funcionar ao gerar outro");
check(!D.ready.servers.some((s) => s.id === priv.id) && (await D.none((m) => m.type === "server_updated" && m.server.id === priv.id, 300)),
  "admin da instância não enxerga servidor privado");
check(newInv.length === 12, "novo convite gerado");

A.send({ type: "delete_my_data" });
await A.wait(type("data_deleted"));
const heir = (await B.wait((m) => m.type === "server_updated" && m.server.id === priv.id && m.server.role === "owner")).server;
check(heir.can_manage, "ao apagar os dados do dono, o servidor passa para o moderador");

B.send({ type: "delete_my_data" });
await B.wait(type("data_deleted"));
check((await fetch(base + attUrl)).status === 404, "ao apagar os dados, as imagens da pessoa somem junto");
check((await fetch(base + keptSoundUrl)).status === 404, "ao apagar os dados, os sons enviados pela pessoa somem junto");

for (const c of [B, C, D, E]) c.ws.close();
console.log(failures ? `\n${failures} falha(s)` : "\nTudo certo");
process.exit(failures ? 1 : 0);
