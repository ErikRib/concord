"use strict";
// Canais de voz, câmera e transmissão de tela (LiveKit). Usa as funções globais do app.js.
// Áudio e vídeo vão direto entre o navegador e o LiveKit, criptografados, e nunca são gravados.

const LK = window.LivekitClient;
const SRC = LK?.Track.Source;

// [chave, rótulo, largura, altura, fps, bitrate máximo]
const SCREEN_PRESETS = [
  ["720p30", "720p · 30 fps", 1280, 720, 30, 3_000_000],
  ["1080p30", "1080p · 30 fps", 1920, 1080, 30, 5_000_000],
  ["1080p60", "1080p · 60 fps (recomendado)", 1920, 1080, 60, 8_000_000],
  ["1440p60", "1440p · 60 fps", 2560, 1440, 60, 14_000_000],
  ["4k30", "4K · 30 fps", 3840, 2160, 30, 16_000_000],
  ["4k60", "4K · 60 fps", 3840, 2160, 60, 25_000_000],
];

const CODECS = [
  ["h264", "H.264 — mais compatível, com aceleração de hardware na maioria das GPUs"],
  ["vp9", "VP9 — ótimo para texto e código"],
  ["av1", "AV1 — o mais eficiente; exige GPU recente para codificar bem"],
];

// Preferências pessoais: ficam só neste navegador.
const vset = Object.assign(
  {
    input: "default",
    output: "default",
    noise: true,
    echo: true,
    agc: true,
    mode: "vad", // "vad" = ativação por voz; "ptt" = apertar para falar
    pttKey: "KeyV",
    codec: "h264",
    preset: "1080p60",
    hint: "motion",
    shareAudio: true,
    volumes: {}, // id pseudônimo -> volume (0 a 2)
  },
  load("concord.voice", {}),
);
const saveVset = () => store("concord.voice", vset);

const voice = {
  room: null,
  channelId: null,
  serverId: null,
  connecting: false,
  muted: load("concord.muted", false),
  deafened: false,
  pttDown: false,
  speaking: new Set(), // identidades falando agora
  watching: new Set(), // identidades cuja transmissão estou assistindo
  focus: null, // chave do bloco em destaque
  videoEls: new Map(), // sid da trilha -> <video>, para não piscar ao redesenhar
  statsTimer: null,
  lastBytes: new Map(),
  videoQuality: "auto",
};

// ---------- entrar e sair ----------
function voiceJoin(channelId) {
  if (!state.voiceEnabled) return toast("Os canais de voz estão desativados nesta instância");
  if (!LK) return toast("Não foi possível carregar o módulo de voz");
  if (voice.room && voice.channelId === channelId) return;
  if (voice.connecting) return;
  voice.connecting = true;
  renderVoiceView();
  send({ type: "voice_join", channel_id: channelId });
}

function micConstraints() {
  return {
    deviceId: vset.input === "default" ? undefined : { exact: vset.input },
    noiseSuppression: vset.noise,
    echoCancellation: vset.echo,
    autoGainControl: vset.agc,
  };
}

async function onVoiceToken(m) {
  // Movido por um moderador: só a aba que está na chamada troca de sala.
  if (m.moved && !voice.room) return;
  // Reconexão do chat com a chamada ainda ativa: só reafirma o estado.
  if (voice.room && voice.channelId === m.channel_id) {
    voice.connecting = false;
    syncVoiceState();
    return;
  }
  if (voice.room) await disconnectRoom();
  voice.channelId = m.channel_id;
  voice.serverId = m.server_id;
  voice.videoQuality = m.video_quality;

  const room = new LK.Room({
    adaptiveStream: true, // recebe a camada do tamanho em que o vídeo aparece na tela
    dynacast: true, // quem transmite deixa de enviar camadas que ninguém está vendo
    webAudioMix: true, // permite volume individual acima de 100%
    audioCaptureDefaults: micConstraints(),
    audioOutput: vset.output === "default" ? undefined : { deviceId: vset.output },
    publishDefaults: {
      audioPreset: { maxBitrate: m.bitrate * 1000 },
      dtx: true,
      red: true,
      videoCodec: vset.codec,
    },
  });
  voice.room = room;
  wireRoom(room);
  const url = m.url ?? `${location.protocol === "https:" ? "wss" : "ws"}://${location.hostname}:${m.port}`;
  try {
    await room.connect(url, m.token, { autoSubscribe: false });
  } catch (e) {
    console.error("Falha ao conectar à voz", url, e);
    const channel = m.channel_id;
    cleanupVoice();
    send({ type: "voice_leave", channel_id: channel });
    // O motivo real ajuda a separar rede, extensão do navegador e servidor.
    const reason = String(e?.message ?? e).slice(0, 160);
    toast(`Não foi possível conectar à voz (${reason}). Se usa bloqueador de anúncios ou rede corporativa, tente em outra rede ou numa janela anônima.`);
    renderVoiceAll();
    return;
  }
  voice.connecting = false;
  for (const p of room.remoteParticipants.values()) {
    for (const pub of p.trackPublications.values()) autoSubscribe(pub, p);
  }
  await applyMic(true);
  room.startAudio().catch(() => {});
  syncVoiceState();
  startStats();
  renderVoiceAll();
}

async function voiceLeave() {
  const channel = voice.room ? voice.channelId : null;
  await disconnectRoom();
  if (channel) send({ type: "voice_leave", channel_id: channel });
  renderVoiceAll();
}

async function disconnectRoom() {
  const room = voice.room;
  cleanupVoice();
  if (room) await room.disconnect();
}

function cleanupVoice() {
  voice.room = null;
  voice.channelId = null;
  voice.serverId = null;
  voice.connecting = false;
  voice.speaking.clear();
  voice.watching.clear();
  voice.focus = null;
  lastServerMute = { muted: false, deafened: false };
  for (const v of voice.videoEls.values()) v.remove();
  voice.videoEls.clear();
  stopStats();
  $("#voice-audio").replaceChildren();
}

/** O servidor encerrou a sessão (canal apagado, banimento etc.). */
function onVoiceEnded(m) {
  disconnectRoom().then(renderVoiceAll);
  toast(m.reason);
}

// ---------- eventos da sala ----------
function participantName(identity) {
  const s = (state.voiceStates.get(voice.serverId) ?? []).find((x) => x.user.id === identity);
  return s?.user ?? { id: identity, name: voice.room?.getParticipantByIdentity(identity)?.name || "…", color: "#5865f2" };
}

/** Áudio e câmeras chegam sozinhos; transmissões de tela só quando a pessoa clica em "Assistir". */
function autoSubscribe(pub, p) {
  const screen = pub.source === SRC.ScreenShare || pub.source === SRC.ScreenShareAudio;
  pub.setSubscribed(screen ? voice.watching.has(p.identity) : true);
}

function wireRoom(room) {
  const E = LK.RoomEvent;
  room
    .on(E.TrackPublished, (pub, p) => { autoSubscribe(pub, p); renderVoiceAll(); })
    .on(E.TrackUnpublished, (pub, p) => {
      if (pub.source === SRC.ScreenShare) voice.watching.delete(p.identity);
      renderVoiceAll();
    })
    .on(E.TrackSubscribed, (track, pub, p) => {
      if (track.kind === "audio") {
        $("#voice-audio").append(track.attach());
        applyVolume(p);
      }
      renderVoiceAll();
    })
    .on(E.TrackUnsubscribed, (track) => {
      track.detach().forEach((e) => e.remove());
      voice.videoEls.delete(track.sid);
      renderVoiceAll();
    })
    .on(E.ParticipantConnected, () => renderVoiceAll())
    .on(E.ParticipantDisconnected, (p) => { voice.watching.delete(p.identity); renderVoiceAll(); })
    .on(E.TrackMuted, () => renderVoiceAll())
    .on(E.TrackUnmuted, () => renderVoiceAll())
    .on(E.LocalTrackPublished, () => { syncVoiceState(); renderVoiceAll(); })
    .on(E.LocalTrackUnpublished, () => { syncVoiceState(); renderVoiceAll(); })
    .on(E.ActiveSpeakersChanged, (speakers) => {
      voice.speaking = new Set(speakers.map((s) => s.identity));
      updateSpeaking();
    })
    .on(E.AudioPlaybackStatusChanged, () => {
      if (!room.canPlaybackAudio) toast("Clique em qualquer lugar da página para ativar o áudio da chamada");
    })
    .on(E.Disconnected, () => {
      if (voice.room !== room) return; // saída pedida por nós
      const channel = voice.channelId;
      cleanupVoice();
      // Com o canal: se fomos movidos nesse meio-tempo, o servidor não nos tira do novo.
      send({ type: "voice_leave", channel_id: channel });
      toast("Você foi desconectado da voz");
      renderVoiceAll();
    });
}

document.addEventListener("click", () => voice.room?.startAudio().catch(() => {}));

function updateSpeaking() {
  for (const e of document.querySelectorAll("[data-speaker]")) {
    e.classList.toggle("speaking", voice.speaking.has(e.dataset.speaker));
  }
}

// ---------- moderação recebida ----------
/** Meu estado definido por moderadores neste servidor (silenciado/ensurdecido). */
function myServerMute() {
  const s = (state.voiceStates.get(voice.serverId) ?? []).find((x) => x.user.id === state.me?.id);
  return { muted: !!s?.server_muted, deafened: !!s?.server_deafened };
}

let lastServerMute = { muted: false, deafened: false };
/** Chamado a cada atualização de quem está na voz: aplica silêncio/surdez da moderação. */
function onVoiceStatesChanged() {
  if (!voice.room) return;
  const now = myServerMute();
  if (now.muted !== lastServerMute.muted || now.deafened !== lastServerMute.deafened) {
    lastServerMute = now;
    applyMic();
    voice.room.remoteParticipants.forEach(applyVolume);
    renderVoiceControls();
  }
}

// ---------- microfone, ensurdecer e apertar para falar ----------
function micShouldBeOn() {
  const mod = myServerMute();
  if (mod.muted || mod.deafened) return false; // o LiveKit também bloqueia
  return !voice.muted && !voice.deafened && (vset.mode === "vad" || voice.pttDown);
}

async function applyMic(first) {
  if (!voice.room) return;
  const on = micShouldBeOn();
  try {
    await voice.room.localParticipant.setMicrophoneEnabled(on, micConstraints());
  } catch (e) {
    console.error(e);
    if (first && on) toast("Sem acesso ao microfone. Você entrou só para ouvir.");
    if (on) voice.muted = true;
  }
}

function applyVolume(p) {
  const v = voice.deafened || myServerMute().deafened ? 0 : (vset.volumes[p.identity] ?? 1);
  p.setVolume(v);
  p.setVolume(v, SRC.ScreenShareAudio);
}

async function toggleMute() {
  const mod = myServerMute();
  if (voice.room && (mod.muted || mod.deafened)) {
    return toast("Um moderador silenciou você neste servidor");
  }
  if (voice.deafened) {
    voice.deafened = false;
    voice.muted = false;
  } else {
    voice.muted = !voice.muted;
  }
  store("concord.muted", voice.muted);
  await applyMic();
  voice.room?.remoteParticipants.forEach(applyVolume);
  syncVoiceState();
  renderVoiceControls();
}

async function toggleDeafen() {
  voice.deafened = !voice.deafened;
  await applyMic();
  voice.room?.remoteParticipants.forEach(applyVolume);
  syncVoiceState();
  renderVoiceControls();
}

const isTypingTarget = (t) => t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement || t instanceof HTMLSelectElement;

document.addEventListener("keydown", (e) => {
  if (vset.mode !== "ptt" || e.code !== vset.pttKey || e.repeat || isTypingTarget(e.target)) return;
  voice.pttDown = true;
  applyMic();
  renderVoiceControls();
});
document.addEventListener("keyup", (e) => {
  if (vset.mode !== "ptt" || e.code !== vset.pttKey) return;
  voice.pttDown = false;
  applyMic();
  renderVoiceControls();
});
window.addEventListener("blur", () => {
  if (voice.pttDown) {
    voice.pttDown = false;
    applyMic();
    renderVoiceControls();
  }
});

/** Estado visível na barra lateral para todo o servidor. */
function syncVoiceState() {
  const lp = voice.room?.localParticipant;
  if (!lp) return;
  send({
    type: "voice_update",
    muted: voice.muted,
    deafened: voice.deafened,
    streaming: lp.isScreenShareEnabled,
    camera: lp.isCameraEnabled,
  });
}

// ---------- câmera ----------
async function toggleCamera() {
  const lp = voice.room?.localParticipant;
  if (!lp) return;
  const preset = voice.videoQuality === "720p" ? LK.VideoPresets.h720 : LK.VideoPresets.h1080;
  try {
    await lp.setCameraEnabled(!lp.isCameraEnabled, { resolution: preset.resolution }, { simulcast: true, videoCodec: vset.codec });
  } catch (e) {
    console.error(e);
    toast("Não foi possível usar a câmera");
  }
  syncVoiceState();
  renderVoiceAll();
}

// ---------- transmissão de tela ----------
async function toggleScreenShare() {
  const lp = voice.room?.localParticipant;
  if (!lp) return;
  if (lp.isScreenShareEnabled) {
    await lp.setScreenShareEnabled(false);
    syncVoiceState();
    renderVoiceAll();
    return;
  }
  if (!navigator.mediaDevices?.getDisplayMedia) return toast("Este navegador não permite transmitir a tela");
  const r = await dialog({
    title: "Transmitir tela",
    desc: "Atenção: sua tela pode mostrar dados pessoais de outras pessoas (conversas, e-mails, documentos). Feche o que não deve aparecer. Nada é gravado.",
    submit: "Escolher o que transmitir",
    fields: [
      { name: "preset", type: "select", label: "Qualidade", options: SCREEN_PRESETS.map((p) => [p[0], p[1]]), value: vset.preset },
      { name: "hint", type: "select", label: "Otimizar para", value: vset.hint, options: [
        ["motion", "Fluidez — jogos e vídeos"],
        ["detail", "Nitidez — texto, código e slides"],
      ] },
      { name: "audio", type: "checkbox", value: vset.shareAudio, label: "Transmitir também o áudio (da aba ou do sistema, quando o navegador permitir)" },
    ],
  });
  if (!r) return;
  Object.assign(vset, { preset: r.preset, hint: r.hint, shareAudio: r.audio });
  saveVset();

  const [, , width, height, fps, maxBitrate] = SCREEN_PRESETS.find((p) => p[0] === r.preset);
  // VP9 e AV1 usam camadas SVC; H.264 usa simulcast (uma camada 720p extra para conexões fracas).
  const svc = vset.codec === "vp9" || vset.codec === "av1";
  try {
    await lp.setScreenShareEnabled(
      true,
      {
        audio: r.audio,
        systemAudio: r.audio ? "include" : "exclude",
        resolution: { width, height, frameRate: fps },
        contentHint: r.hint,
        selfBrowserSurface: "exclude",
        surfaceSwitching: "include",
      },
      {
        videoCodec: vset.codec,
        // O padrão do LiveKit é 1080p a 15 fps; aqui vale o preset escolhido.
        screenShareEncoding: { maxBitrate, maxFramerate: fps, priority: "high" },
        simulcast: !svc,
        screenShareSimulcastLayers: svc ? undefined : [new LK.VideoPreset(1280, 720, 1_500_000, Math.min(fps, 30))],
        scalabilityMode: svc ? "L3T3_KEY" : undefined,
        degradationPreference: r.hint === "motion" ? "maintain-framerate" : "maintain-resolution",
      },
    );
  } catch (e) {
    if (e?.name !== "NotAllowedError") {
      console.error(e);
      toast("Não foi possível transmitir a tela");
    }
  }
  syncVoiceState();
  renderVoiceAll();
}

// ---------- estatísticas reais (resolução, fps e taxa de bits efetivos) ----------
function startStats() {
  stopStats();
  voice.statsTimer = setInterval(updateStats, 2000);
}

function stopStats() {
  clearInterval(voice.statsTimer);
  voice.statsTimer = null;
  voice.lastBytes.clear();
  $("#vp-stats").classList.add("hidden");
}

const LIMITS = { cpu: "processamento", bandwidth: "conexão", other: "navegador" };

/** Taxa de bits (Mbps) e quadros por segundo entre duas leituras de estatísticas. */
function rates(key, bytes, frames, ts) {
  const prev = voice.lastBytes.get(key);
  voice.lastBytes.set(key, { bytes, frames, ts });
  if (!prev || ts <= prev.ts) return { mbps: null, fps: null };
  const secs = (ts - prev.ts) / 1000;
  return {
    mbps: ((bytes - prev.bytes) * 8) / secs / 1e6,
    fps: frames != null && prev.frames != null ? (frames - prev.frames) / secs : null,
  };
}

async function updateStats() {
  const room = voice.room;
  if (!room) return;
  // Minha transmissão
  const screenTrack = room.localParticipant.getTrackPublication(SRC.ScreenShare)?.videoTrack;
  const box = $("#vp-stats");
  if (screenTrack) {
    try {
      const layers = await screenTrack.getSenderStats();
      const best = layers.sort((a, b) => (b.frameWidth ?? 0) - (a.frameWidth ?? 0))[0];
      if (best) {
        const total = layers.reduce((s, l) => s + (l.bytesSent ?? 0), 0);
        const { mbps } = rates("local", total, null, best.timestamp);
        let text = `Transmitindo ${best.frameWidth ?? "?"}×${best.frameHeight ?? "?"} · ${Math.round(best.framesPerSecond ?? 0)} fps`;
        if (mbps != null) text += ` · ${mbps.toFixed(1)} Mbps`;
        const lim = best.qualityLimitationReason;
        if (lim && lim !== "none") text += ` · limitado pelo(a) ${LIMITS[lim] ?? lim}`;
        box.textContent = text;
        box.classList.remove("hidden");
      }
    } catch {}
  } else {
    box.classList.add("hidden");
  }
  // Transmissões que estou assistindo
  for (const e of document.querySelectorAll("[data-stats-sid]")) {
    const p = room.getParticipantByIdentity(e.dataset.identity);
    const track = p?.getTrackPublication(SRC.ScreenShare)?.videoTrack;
    if (!track?.getReceiverStats) continue;
    try {
      const s = await track.getReceiverStats();
      if (!s) continue;
      // O SDK não informa fps na recepção: calcula pelos quadros decodificados.
      const { mbps, fps } = rates(e.dataset.statsSid, s.bytesReceived ?? 0, s.framesDecoded, s.timestamp);
      let text = `${s.frameWidth ?? "?"}×${s.frameHeight ?? "?"}`;
      if (fps != null) text += ` · ${Math.round(fps)} fps`;
      if (mbps != null) text += ` · ${mbps.toFixed(1)} Mbps`;
      e.textContent = text;
    } catch {}
  }
}

// ---------- configurações ----------
async function voiceSettings() {
  let inputs = [];
  let outputs = [];
  try {
    inputs = await LK.Room.getLocalDevices("audioinput");
    outputs = await LK.Room.getLocalDevices("audiooutput", false);
  } catch {}
  const opts = (list, fallback) => [["default", fallback], ...list.filter((d) => d.deviceId !== "default").map((d) => [d.deviceId, d.label || "Dispositivo"])];
  const r = await dialog({
    title: "Voz e vídeo",
    desc: "Estas preferências ficam só neste navegador.",
    submit: "Salvar",
    fields: [
      { name: "input", type: "select", label: "Microfone", options: opts(inputs, "Padrão do sistema"), value: vset.input },
      { name: "output", type: "select", label: "Saída de áudio", options: opts(outputs, "Padrão do sistema"), value: vset.output },
      { name: "mode", type: "select", label: "Modo de entrada", value: vset.mode, options: [
        ["vad", "Ativação por voz"],
        ["ptt", "Apertar para falar"],
      ] },
      { name: "pttKey", type: "key", label: "Tecla do apertar para falar (funciona com a aba do Concord em foco)", value: vset.pttKey },
      { name: "noise", type: "checkbox", value: vset.noise, label: "Supressão de ruído" },
      { name: "echo", type: "checkbox", value: vset.echo, label: "Cancelamento de eco" },
      { name: "agc", type: "checkbox", value: vset.agc, label: "Ajuste automático de volume do microfone" },
      { name: "codec", type: "select", label: "Codec de vídeo (câmera e tela)", options: CODECS, value: vset.codec },
    ],
  });
  if (!r) return;
  Object.assign(vset, r);
  saveVset();
  const room = voice.room;
  if (room) {
    try {
      await room.switchActiveDevice("audioinput", vset.input);
      if (vset.output !== "default") await room.switchActiveDevice("audiooutput", vset.output);
      const mic = room.localParticipant.getTrackPublication(SRC.Microphone)?.track;
      if (mic) await mic.restartTrack(micConstraints());
    } catch (e) {
      console.error(e);
      toast("Algumas configurações valem a partir da próxima chamada");
    }
    await applyMic();
  }
  renderVoiceControls();
}

async function setUserVolume(user) {
  const current = Math.round((vset.volumes[user.id] ?? 1) * 100);
  const r = await dialog({
    title: `Volume de ${user.name}`,
    desc: "Só muda o que você ouve, neste navegador.",
    submit: "Aplicar",
    fields: [{ name: "v", type: "select", label: "Volume", value: String(current), options: [0, 25, 50, 75, 100, 125, 150, 200].map((v) => [String(v), `${v}%`]) }],
  });
  if (!r) return;
  vset.volumes[user.id] = Number(r.v) / 100;
  saveVset();
  const p = voice.room?.getParticipantByIdentity(user.id);
  if (p) applyVolume(p);
}

async function editVoiceStatus(ch) {
  const r = await dialog({
    title: "Status do canal de voz",
    desc: "Aparece para todo o servidor e some quando o canal esvazia. Deixe vazio para remover.",
    submit: "Salvar",
    fields: [{ name: "status", label: "Status", value: ch.status ?? "", maxLength: 100, placeholder: "Ex.: jogando xadrez" }],
  });
  if (r) send({ type: "set_voice_status", channel_id: ch.id, status: r.status });
}

async function channelSettings(server, ch) {
  const fields = [{ name: "name", label: "Nome", value: ch.name, required: true, maxLength: 32 }];
  if (ch.kind === "voice") {
    fields.push(
      { name: "user_limit", type: "select", label: "Limite de pessoas", value: String(ch.user_limit),
        options: [0, 2, 3, 4, 5, 8, 10, 15, 20, 25, 50, 99].map((n) => [String(n), n === 0 ? "Sem limite" : String(n)]) },
      { name: "bitrate", type: "select", label: "Qualidade do áudio (taxa de bits)", value: String(ch.bitrate),
        options: [8, 16, 32, 48, 64, 96, 128, 192, 256, 384].map((n) => [String(n), `${n} kbps${n === 64 ? " (padrão)" : ""}`]) },
      { name: "video_quality", type: "select", label: "Qualidade da câmera", value: ch.video_quality,
        options: [["auto", "Automática (até 1080p)"], ["720p", "720p"]] },
    );
  }
  const r = await dialog({ title: `Configurações de ${ch.kind === "voice" ? "" : "#"}${ch.name}`, submit: "Salvar", fields });
  if (!r) return;
  const msg = { type: "update_channel", channel_id: ch.id, name: r.name };
  if (ch.kind === "voice") Object.assign(msg, { user_limit: Number(r.user_limit), bitrate: Number(r.bitrate), video_quality: r.video_quality });
  send(msg);
}

// ---------- moderação de voz (menu do moderador) ----------
/** Itens do menu de uma pessoa que está numa chamada deste servidor. */
function voiceModerationItems(server, user) {
  if (!server?.can_moderate || user.id === state.me?.id) return [];
  const s = (state.voiceStates.get(server.id) ?? []).find((x) => x.user.id === user.id);
  if (!s) return [];
  const act = (action) => () => send({ type: "voice_moderate", server_id: server.id, user_id: user.id, action });
  // Alternâncias com caixinha (docs/DESIGN.md › Menu de contexto).
  return [
    "-",
    { label: "Silenciar no servidor", danger: true, checked: s.server_muted, onClick: act(s.server_muted ? "unmute" : "mute") },
    { label: "Ensurdecer no servidor", danger: true, checked: s.server_deafened, onClick: act(s.server_deafened ? "undeafen" : "deafen") },
    { label: "Mover para outro canal…", icon: "move", onClick: () => moveUser(server, s) },
    { label: "Desconectar da voz", icon: "hangup", danger: true, onClick: act("disconnect") },
  ];
}

async function moveUser(server, s) {
  const targets = server.channels.filter((c) => c.kind === "voice" && c.id !== s.channel_id);
  if (targets.length === 0) return toast("Não há outro canal de voz neste servidor");
  const r = await dialog({
    title: `Mover ${s.user.name}`,
    submit: "Mover",
    fields: [{ name: "channel", type: "select", label: "Para o canal", options: targets.map((c) => [c.id, c.name]) }],
  });
  if (r) send({ type: "voice_move", user_id: s.user.id, channel_id: r.channel });
}

// ---------- desenho ----------
function renderVoiceAll() {
  renderVoiceControls();
  renderChannels();
  renderVoiceView();
}

function renderVoiceControls() {
  const room = voice.room;
  $("#voice-panel").classList.toggle("hidden", !room && !voice.connecting);
  const server = state.servers.find((s) => s.id === voice.serverId);
  const ch = server?.channels.find((c) => c.id === voice.channelId);
  $("#vp-status").textContent = voice.connecting ? "Conectando…" : voice.pttDown ? "Voz conectada · falando" : "Voz conectada";
  $("#vp-where").textContent = ch ? `${ch.name} / ${server.name}` : "";
  const lp = room?.localParticipant;
  $("#vp-camera").classList.toggle("active", !!lp?.isCameraEnabled);
  $("#vp-screen").classList.toggle("active", !!lp?.isScreenShareEnabled);
  $("#vp-screen span").textContent = lp?.isScreenShareEnabled ? "Parar" : "Tela";
  const mod = room ? myServerMute() : { muted: false, deafened: false };
  const mute = $("#btn-mute");
  mute.classList.toggle("off", voice.muted || voice.deafened || mod.muted || mod.deafened);
  mute.classList.toggle("by-mod", mod.muted || mod.deafened);
  mute.title = mod.muted || mod.deafened ? "Silenciado por um moderador" : voice.muted ? "Ativar microfone" : "Silenciar microfone";
  const deaf = $("#btn-deafen");
  deaf.classList.toggle("off", voice.deafened || mod.deafened);
  deaf.classList.toggle("by-mod", mod.deafened);
  deaf.title = mod.deafened ? "Áudio desativado por um moderador" : voice.deafened ? "Ativar áudio" : "Desativar áudio";
}

function videoFor(track) {
  let v = voice.videoEls.get(track.sid);
  if (!v) {
    v = track.attach();
    v.muted = true;
    v.playsInline = true;
    voice.videoEls.set(track.sid, v);
  }
  return v;
}

/** Blocos do palco: uma pessoa (câmera ou avatar) ou uma transmissão de tela. */
function collectTiles() {
  const room = voice.room;
  const tiles = [];
  const add = (p, local) => {
    const user = local ? { ...state.me, color: identity.color } : participantName(p.identity);
    const cam = p.getTrackPublication(SRC.Camera);
    const mic = p.getTrackPublication(SRC.Microphone);
    tiles.push({
      key: `u:${p.identity}`,
      kind: "user",
      identity: p.identity,
      user,
      local,
      track: cam && !cam.isMuted ? cam.videoTrack : null,
      micOff: local ? voice.muted || voice.deafened : !mic || mic.isMuted,
    });
    const scr = p.getTrackPublication(SRC.ScreenShare);
    if (scr) {
      tiles.push({
        key: `s:${p.identity}`,
        kind: "screen",
        identity: p.identity,
        user,
        local,
        sid: scr.trackSid,
        track: local || voice.watching.has(p.identity) ? scr.videoTrack : null,
      });
    }
  };
  add(room.localParticipant, true);
  for (const p of room.remoteParticipants.values()) add(p, false);
  return tiles;
}

function tileEl(t) {
  const div = el("div", `tile ${t.kind}${voice.focus === t.key ? " focused" : ""}`);
  div.dataset.key = t.key;
  if (t.kind === "user") div.dataset.speaker = t.identity;

  if (t.track) {
    div.append(videoFor(t.track));
  } else if (t.kind === "screen") {
    const ph = el("div", "tile-placeholder");
    ph.append(el("div", "live-badge", "AO VIVO"), el("p", null, `${t.user.name} está transmitindo`));
    ph.append(button("Assistir transmissão", "primary-btn", () => watch(t.identity, true)));
    div.append(ph);
  } else {
    const a = avatar(t.user, true);
    a.classList.add("tile-avatar");
    div.append(a);
  }

  const bar = el("div", "tile-bar");
  bar.append(el("span", "tile-name", t.kind === "screen" ? `Tela de ${t.user.name}` : t.user.name));
  if (t.kind === "user" && t.micOff) bar.append(el("span", "tile-flag", ICON_MIC_OFF));
  if (t.kind === "screen") bar.append(el("span", "live-badge small", "AO VIVO"));
  div.append(bar);

  if (t.kind === "screen" && t.track && !t.local) {
    const stats = el("div", "tile-stats");
    stats.dataset.statsSid = t.sid;
    stats.dataset.identity = t.identity;
    div.append(stats);
  }

  const tools = el("div", "tile-tools");
  if (t.track) {
    tools.append(button(voice.focus === t.key ? "Sair do destaque" : "Destacar", null, (e) => {
      e.stopPropagation();
      voice.focus = voice.focus === t.key ? null : t.key;
      renderVoiceView();
    }));
    tools.append(button("Tela cheia", null, (e) => { e.stopPropagation(); div.requestFullscreen?.(); }));
  }
  if (t.kind === "screen" && !t.local && voice.watching.has(t.identity)) {
    tools.append(button("Parar de assistir", null, (e) => { e.stopPropagation(); watch(t.identity, false); }));
  }
  if (!t.local && t.kind === "user") {
    tools.append(button("Volume", null, (e) => { e.stopPropagation(); setUserVolume(t.user); }));
  }
  div.append(tools);
  div.ondblclick = () => t.track && div.requestFullscreen?.();
  return div;
}

function watch(identity, on) {
  const p = voice.room?.getParticipantByIdentity(identity);
  if (!p) return;
  if (on) voice.watching.add(identity);
  else voice.watching.delete(identity);
  for (const src of [SRC.ScreenShare, SRC.ScreenShareAudio]) p.getTrackPublication(src)?.setSubscribed(on);
  if (on) voice.focus = `s:${identity}`;
  else if (voice.focus === `s:${identity}`) voice.focus = null;
  renderVoiceView();
}

function renderVoiceView() {
  const ch = currentChannel();
  if (!ch || ch.kind !== "voice") return;
  const server = currentServer();
  const view = $("#voice-view");
  const inThis = voice.room && voice.channelId === ch.id;
  view.replaceChildren();

  if (!inThis) {
    const people = (state.voiceStates.get(server.id) ?? []).filter((s) => s.channel_id === ch.id);
    const lobby = el("div", "vv-lobby");
    lobby.append(el("h2", null, ch.name));
    if (ch.status) lobby.append(el("p", "vv-status-text", ch.status));
    lobby.append(el("p", "muted", people.length ? `${people.length} pessoa(s) na chamada` : "Ninguém na chamada ainda"));
    const avatars = el("div", "vv-avatars");
    for (const s of people) avatars.append(avatar(s.user, true));
    lobby.append(avatars);
    if (ch.user_limit) lobby.append(el("p", "muted", `Limite: ${people.length}/${ch.user_limit} pessoas`));
    if (!state.voiceEnabled) {
      lobby.append(el("p", "muted", "Os canais de voz estão desativados nesta instância."));
    } else {
      lobby.append(button(voice.connecting ? "Conectando…" : voice.room ? "Mudar para este canal" : "Entrar na chamada", "primary-btn", () => voiceJoin(ch.id)));
    }
    view.append(lobby);
    return;
  }

  const head = el("div", "vv-head");
  head.append(el("span", "vv-status-text", ch.status ? ch.status : "Sem status"));
  head.append(button(ch.status ? "Editar status" : "Definir status", null, () => editVoiceStatus(ch)));
  view.append(head);

  const tiles = collectTiles();
  const focused = tiles.find((t) => t.key === voice.focus && t.track);
  const stage = el("div", focused ? "vv-stage has-focus" : "vv-stage");
  if (focused) {
    const main = el("div", "vv-focus");
    main.append(tileEl(focused));
    const strip = el("div", "vv-strip");
    for (const t of tiles) if (t !== focused) strip.append(tileEl(t));
    stage.append(main, strip);
  } else {
    const grid = el("div", "vv-grid");
    grid.style.setProperty("--cols", String(Math.min(4, Math.ceil(Math.sqrt(tiles.length)))));
    for (const t of tiles) grid.append(tileEl(t));
    stage.append(grid);
  }
  view.append(stage);
  updateSpeaking();
}

// ---------- ligações com a interface ----------
$("#btn-mute").onclick = toggleMute;
$("#btn-deafen").onclick = toggleDeafen;
$("#btn-voice-settings").onclick = () => openSettings("user", "account");
$("#vp-leave").onclick = voiceLeave;
$("#vp-camera").onclick = toggleCamera;
$("#vp-screen").onclick = toggleScreenShare;
window.addEventListener("beforeunload", () => voice.room?.disconnect());
renderVoiceControls();
