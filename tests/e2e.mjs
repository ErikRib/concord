// Teste de ponta a ponta do Concord (v2: moderação). Uso: node tests/e2e.mjs [porta] | node tests/e2e.mjs --admin-id
import { createHash } from "node:crypto";

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
  c.send({ type: "hello", secret, name, color: "#123456", consent_version: 3 });
  c.ready = await c.wait(type("ready"));
  c.id = c.ready.me.id;
  return c;
}

// ---------- básico ----------
const res = await fetch(base + "/");
check(res.headers.get("content-security-policy")?.includes("default-src 'self'"), "CSP presente");
const pol = await (await fetch(base + "/privacidade")).text();
check(pol.includes("Versão 3") && pol.includes("Teste &lt;Ltda&gt;") && !pol.includes("{{"), "política v3 renderizada e escapada");
check((await fetch(base + "/convite/abc123XYZ")).status === 200, "rota /convite/:code entrega o app");

let c0 = client(); await c0.opened;
c0.send({ type: "hello", secret: S.a, name: "Ana", color: "#ff0000", consent_version: 1 });
check((await c0.wait(type("consent_required"))).version === 3, "aceite antigo não vale mais: pede consentimento da v3");
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
  const lkUrl = (tok.url ?? "ws://localhost:7880").replace(/^ws/, "http");
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
  await sleep(1500);
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

for (const c of [B, C, D, E]) c.ws.close();
console.log(failures ? `\n${failures} falha(s)` : "\nTudo certo");
process.exit(failures ? 1 : 0);
