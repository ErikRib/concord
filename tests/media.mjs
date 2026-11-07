// Teste de mídia real: dois navegadores numa chamada do Concord via LiveKit.
// Áudio (microfone falso), presença na barra lateral e transmissão de tela 1080p60.
//
// Uso (servidor de teste na porta 3999, com LiveKit):
//   npm install --prefix tests
//   node tests/media.mjs            # sem janela (fonte de vídeo congelada: fps baixo é esperado)
//   HEADED=1 node tests/media.mjs   # com janelas, para medir a fluidez real com GPU
import puppeteer from "puppeteer-core";
import { createHash } from "node:crypto";

// Ana é administradora da instância (modera o servidor público "Concord").
// Inicie o servidor com CONCORD_ADMINS igual à saída de `node tests/media.mjs --admin-id`.
const ANA = "1".repeat(64);
if (process.argv[2] === "--admin-id") {
  const h = createHash("sha256").update("concord-user-id:" + ANA).digest().subarray(0, 16).toString("hex");
  console.log(`${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`);
  process.exit(0);
}

const CHROME = process.env.CHROME ?? "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const BASE = process.env.BASE ?? "http://localhost:3999";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failures = 0;
const check = (cond, label) => { console.log(`${cond ? "OK  " : "FAIL"} ${label}`); if (!cond) failures++; };

const browser = await puppeteer.launch({
  executablePath: CHROME,
  headless: !process.env.HEADED,
  args: [
    "--use-fake-ui-for-media-stream", // aceita permissões de mídia
    "--use-fake-device-for-media-stream", // microfone/câmera falsos (tom e imagem de teste)
    "--auto-select-tab-capture-source-by-title=Alvo", // getDisplayMedia escolhe a aba "Alvo"
    "--autoplay-policy=no-user-gesture-required",
    "--disable-background-timer-throttling",
    "--disable-renderer-backgrounding",
    "--disable-backgrounding-occluded-windows",
    "--window-size=1920,1080",
  ],
});

// Aba que será transmitida: animação a 60 fps em 1920x1080.
const LivekitClientQualityHigh = 2; // VideoQuality.HIGH
const target = await browser.newPage();
await target.setViewport({ width: 1920, height: 1080 });
await target.goto(`data:text/html,${encodeURIComponent(`<!doctype html><title>Alvo</title><body style="margin:0;background:#000">
<canvas id=c width=1920 height=1080></canvas><script>
const c=document.getElementById('c').getContext('2d');let t=0;window.frames0=0;setInterval(()=>{window.fps=window.frames0;window.frames0=0},1000);
(function f(){t++;window.frames0++;c.fillStyle='hsl('+(t%360)+',70%,30%)';c.fillRect(0,0,1920,1080);
c.fillStyle='#fff';c.font='120px sans-serif';c.fillText('Concord '+t,100+(t*7)%1400,500);
for(let i=0;i<40;i++){c.fillRect((t*13+i*97)%1920,(i*53+t*5)%1080,40,40)}requestAnimationFrame(f)})();
const ac=new AudioContext();const o=ac.createOscillator();o.frequency.value=330;o.connect(ac.destination);o.start();
</script>`)}`);

async function person(ctx, secret, name) {
  const page = await ctx.newPage();
  await page.setViewport({ width: 1280, height: 800 });
  page.on("pageerror", (e) => console.log(`[${name}] erro na página: ${e.message}`));
  await page.evaluateOnNewDocument((secret, name) => {
    try {
      localStorage.setItem("concord.identity", JSON.stringify({ secret, name, color: "#3ba55c", consent: 6 }));
    } catch {}
  }, secret, name);
  await page.goto(BASE);
  await page.waitForFunction(() => state.me && state.servers.length > 0, { timeout: 10000 });
  return page;
}

// Diagnóstico: tipo de conexão (UDP/TCP), banda estimada e camadas enviadas.
async function pcDiag(page) {
  return page.evaluate(async () => {
    const t = voice.room.engine.pcManager?.publisher;
    const pc = t?.pc ?? t?._pc;
    const out = {};
    if (pc) {
      const stats = await pc.getStats();
      stats.forEach((r) => {
        if (r.type === "candidate-pair" && r.nominated && r.state === "succeeded") {
          out.bweMbps = ((r.availableOutgoingBitrate ?? 0) / 1e6).toFixed(2);
          const lc = stats.get(r.localCandidateId);
          const rc = stats.get(r.remoteCandidateId);
          out.via = `${lc?.protocol}/${lc?.candidateType} -> ${rc?.protocol}:${rc?.port}`;
        }
      });
    } else {
      out.erro = "sem acesso ao PeerConnection";
    }
    const pub = voice.room.localParticipant.getTrackPublication("screen_share");
    if (pub) {
      const l = await pub.videoTrack.getSenderStats();
      out.camadas = l.map((x) => `${x.rid}:${x.frameWidth}x${x.frameHeight}@${Math.round(x.framesPerSecond ?? 0)} ${x.qualityLimitationReason} alvo=${((x.targetBitrate ?? 0) / 1e6).toFixed(1)}M`);
    }
    return out;
  });
}

const ctxA = browser.defaultBrowserContext();
const ctxB = await browser.createBrowserContext(); // outro "navegador": outra identidade
const A = await person(ctxA, ANA, "Ana");
const B = await person(ctxB, "2".repeat(64), "Bia");

const voiceId = await A.evaluate(() => {
  const s = state.servers.find((x) => x.name === "Concord");
  selectServer(s.id);
  return s.channels.find((c) => c.kind === "voice").id;
});
for (const p of [A, B]) {
  await p.evaluate((id) => {
    selectServer(state.servers.find((x) => x.name === "Concord").id);
    selectChannel(id);
  }, voiceId);
}
const connected = (p) => p.waitForFunction(() => voice.room?.state === "connected" && voice.room.remoteParticipants.size === 1, { timeout: 20000 });
await Promise.all([connected(A), connected(B)]);
check(true, "Ana e Bia conectadas à mesma sala do LiveKit");

// Áudio: Ana publica o microfone; Bia recebe e o áudio está chegando.
await sleep(3000);
const audio = await B.evaluate(async () => {
  const p = [...voice.room.remoteParticipants.values()][0];
  const pub = p.getTrackPublication(LivekitClient.Track.Source.Microphone);
  const stats = pub?.track ? await pub.track.getReceiverStats?.() : null;
  return { subscribed: !!pub?.isSubscribed, bytes: stats?.bytesReceived ?? 0, codec: pub?.mimeType };
});
check(audio.subscribed && audio.bytes > 1000, `Bia recebe o áudio da Ana (${audio.codec}, ${audio.bytes} bytes)`);
const sidebar = await B.evaluate(() => document.querySelectorAll(".ch-voice-users li").length);
check(sidebar === 2, `barra lateral mostra as 2 pessoas no canal de voz (${sidebar})`);

A.on("console", async (m) => {
  const t = m.text();
  if (/start-bitrate|publishing video/i.test(t)) {
    const args = await Promise.all(m.args().map((a) => a.jsonValue().catch(() => "?")));
    console.log("[Ana console]", JSON.stringify(args).slice(0, 400));
  }
});
await B.setViewport({ width: 1920, height: 1080 });
await A.evaluate(() => LivekitClient.setLogLevel("debug"));

// Transmissão de tela da Ana em 1080p60, pelo diálogo de verdade.
await A.click("#vp-screen");
await A.waitForSelector("#modal-card select");
await A.evaluate(() => {
  const [preset, hint] = document.querySelectorAll("#modal-card select");
  preset.value = "1080p60";
  hint.value = "motion";
});
await A.click("#modal-card button[type=submit]");
await A.waitForFunction(() => voice.room.localParticipant.isScreenShareEnabled, { timeout: 15000 });
check(true, "Ana começou a transmitir a tela");

// Bia vê o "AO VIVO" e clica em assistir.
await B.waitForFunction(() => document.querySelector(".tile.screen .primary-btn"), { timeout: 10000 });
await B.evaluate(() => document.querySelector(".tile.screen .primary-btn").click()); // clique atômico (a chamada redesenha os blocos)
for (let k = 1; k <= 6; k++) {
  await sleep(5000);
  const srcFps = await target.evaluate(() => window.fps);
  const tile = await B.evaluate(() => { const v = document.querySelector(".tile.screen video"); return v ? `${v.clientWidth}x${v.clientHeight}` : "-"; });
  console.log(`t=${k * 5}s fonte=${srcFps}fps video-Bia=${tile}`, JSON.stringify(await pcDiag(A)));
}

const sender = await A.evaluate(async () => {
  const pub = voice.room.localParticipant.getTrackPublication(LivekitClient.Track.Source.ScreenShare);
  const layers = await pub.videoTrack.getSenderStats();
  const s = pub.videoTrack.mediaStreamTrack.getSettings();
  return {
    codec: pub.mimeType,
    capture: `${s.width}x${s.height}@${Math.round(s.frameRate ?? 0)}`,
    layers: layers.map((l) => `${l.rid ?? "-"}:${l.frameWidth}x${l.frameHeight}@${Math.round(l.framesPerSecond ?? 0)} lim=${l.qualityLimitationReason}`),
  };
});
const receiver = await B.evaluate(async () => {
  const p = [...voice.room.remoteParticipants.values()][0];
  const pub = p.getTrackPublication(LivekitClient.Track.Source.ScreenShare);
  const s1 = await pub.videoTrack.getReceiverStats();
  await new Promise((r) => setTimeout(r, 2000));
  const s2 = await pub.videoTrack.getReceiverStats();
  const secs = (s2.timestamp - s1.timestamp) / 1000;
  const mbps = ((s2.bytesReceived - s1.bytesReceived) * 8) / secs / 1e6;
  const fps = (s2.framesDecoded - s1.framesDecoded) / secs;
  return { w: s2.frameWidth, h: s2.frameHeight, fps, mbps: mbps.toFixed(2), statsText: document.querySelector(".tile-stats")?.textContent };
});
console.log("Ana (envio):", JSON.stringify(sender));
console.log("Bia (recebe):", JSON.stringify(receiver));
check(receiver.w >= 1280 && receiver.h >= 720, `Bia recebe a tela em ${receiver.w}x${receiver.h}`);
// Informativo: sem GPU e com a aba de origem congelada, o ambiente de teste não gera 60 fps reais.
check((receiver.fps ?? 0) > 0, `vídeo chegando na Bia: ${Math.round(receiver.fps ?? 0)} fps decodificados (${receiver.mbps} Mbps)`);
check(!!receiver.statsText, `estatísticas aparecem no bloco da transmissão: "${receiver.statsText}"`);

// Áudio da transmissão (a aba "Alvo" toca um tom): precisa chegar a quem assiste.
const shareAudio = await B.evaluate(async () => {
  const p = [...voice.room.remoteParticipants.values()][0];
  const pub = p.getTrackPublication(LivekitClient.Track.Source.ScreenShareAudio);
  const st = await pub?.audioTrack?.getReceiverStats?.();
  return { published: !!pub, subscribed: !!pub?.isSubscribed, bytes: st?.bytesReceived ?? 0, quality: pub?.videoQuality };
});
check(shareAudio.subscribed && shareAudio.bytes > 1000, `Bia recebe o áudio da transmissão (${JSON.stringify(shareAudio)})`);
const shareAudioSender = await A.evaluate(() => {
  const t = voice.room.localParticipant.getTrackPublication(LivekitClient.Track.Source.ScreenShareAudio)?.audioTrack?.mediaStreamTrack;
  const s = t?.getSettings() ?? {};
  return { ec: s.echoCancellation, ns: s.noiseSuppression, agc: s.autoGainControl };
});
check(shareAudioSender.ec === false && shareAudioSender.ns === false, `áudio da transmissão sai sem o processamento de voz (${JSON.stringify(shareAudioSender)})`);
const quality = await B.evaluate(() => {
  const p = [...voice.room.remoteParticipants.values()][0];
  return p.getTrackPublication(LivekitClient.Track.Source.ScreenShare)?.videoQuality;
});
check(quality === LivekitClientQualityHigh, `quem assiste pede a qualidade original da transmissão (${quality})`);

// ---------- moderação de voz aplicada no LiveKit ----------
const bobId = await B.evaluate(() => state.me.id);
const micOfBia = () =>
  A.evaluate((id) => {
    const p = voice.room.getParticipantByIdentity(id);
    const pub = p?.getTrackPublication(LivekitClient.Track.Source.Microphone);
    return { exists: !!pub, muted: pub ? pub.isMuted : true };
  }, bobId);
check(!(await micOfBia()).muted, "antes da moderação, o microfone da Bia está ativo para a Ana");

await A.evaluate((id) => {
  const s = state.servers.find((x) => x.name === "Concord");
  send({ type: "voice_moderate", server_id: s.id, user_id: id, action: "mute" });
}, bobId);
await sleep(2500);
check((await micOfBia()).muted, "silenciar no servidor muta a trilha da Bia no LiveKit");
const perm = await B.evaluate(() => voice.room.localParticipant.permissions?.canPublishSources ?? []);
check(!perm.includes(2) && !perm.includes("MICROPHONE") && !perm.includes("microphone"),
  `LiveKit retirou a permissão de microfone da Bia (${JSON.stringify(perm)})`);

// Cliente "modificado": tenta religar o microfone chamando o SDK direto, sem a interface.
const bypass = await B.evaluate(async () => {
  try {
    await voice.room.localParticipant.setMicrophoneEnabled(true);
    return "sem erro";
  } catch (e) {
    return `erro: ${e.message}`;
  }
});
await sleep(2000);
check((await micOfBia()).muted, `burlar pelo SDK não funciona; a Ana continua sem ouvir a Bia (${bypass})`);

await A.evaluate((id) => {
  const s = state.servers.find((x) => x.name === "Concord");
  send({ type: "voice_moderate", server_id: s.id, user_id: id, action: "unmute" });
}, bobId);
await sleep(2500);
await B.evaluate(() => applyMic());
// A volta passa pelo LiveKit (permissão + nova publicação): espera até 10 s, sem tempo fixo.
let micBack = false;
for (let k = 0; k < 20 && !micBack; k++) {
  await sleep(500);
  micBack = !(await micOfBia()).muted;
}
check(micBack, "remover o silêncio devolve o microfone da Bia");

// Desconectar: o LiveKit derruba a sessão da Bia mesmo que o navegador dela não "saia".
await B.evaluate(() => { window.onVoiceEnded = () => {}; }); // simula cliente que ignora o aviso
await A.evaluate((id) => {
  const s = state.servers.find((x) => x.name === "Concord");
  send({ type: "voice_moderate", server_id: s.id, user_id: id, action: "disconnect" });
}, bobId);
await A.waitForFunction(() => voice.room.remoteParticipants.size === 0, { timeout: 10000 }).catch(() => {});
check(await A.evaluate(() => voice.room.remoteParticipants.size === 0), "desconectar remove a Bia da sala no próprio LiveKit");

await A.evaluate(() => voiceLeave());
check(true, "Ana sai da chamada");

await browser.close();
console.log(failures ? `\n${failures} falha(s)` : "\nTudo certo");
process.exit(failures ? 1 : 0);
