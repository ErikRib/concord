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
  ["auto", "Automático — escolhe um codec com aceleração na sua GPU (recomendado)"],
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
    codec: "auto",
    preset: "1080p60",
    hint: "motion",
    shareAudio: true,
    volumes: {}, // id pseudônimo -> volume da voz (0 a 2)
    streamVolumes: {}, // id pseudônimo -> volume do áudio da transmissão (0 a 2)
  },
  load("concord.voice", {}),
);
const saveVset = () => store("concord.voice", vset);
// Versões antigas gravavam "h264" como padrão; passa a ser "automático" uma única vez.
if (!vset.codecV2) {
  if (vset.codec === "h264") vset.codec = "auto";
  vset.codecV2 = true;
  saveVset();
}

/**
 * Codec efetivo. "auto": o primeiro com codificação por hardware nesta máquina (consulta o
 * navegador). Ordem: H.264 (todo mundo decodifica por hardware), AV1, VP9. Sem hardware: H.264.
 */
const codecCache = new Map();
async function pickCodec(width, height, fps, bitrate) {
  if (vset.codec !== "auto") return vset.codec;
  const key = `${width}x${height}@${fps}`;
  if (codecCache.has(key)) return codecCache.get(key);
  let chosen = "h264";
  if (navigator.mediaCapabilities?.encodingInfo) {
    for (const [codec, mime] of [["h264", "video/H264"], ["av1", "video/AV1"], ["vp9", "video/VP9"]]) {
      try {
        const info = await navigator.mediaCapabilities.encodingInfo({
          type: "webrtc",
          video: { contentType: mime, width, height, bitrate, framerate: fps },
        });
        if (info.supported && info.powerEfficient) { chosen = codec; break; }
      } catch {}
    }
  }
  codecCache.set(key, chosen);
  return chosen;
}

const voice = {
  room: null,
  channelId: null,
  serverId: null,
  connecting: false,
  reconnecting: false,
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
    // Sem adaptiveStream: com ele, o SDK escolhe a camada pelo tamanho do vídeo na tela e ignora
    // qualquer escolha manual; uma transmissão num bloco menor que a tela chegava em 720p30.
    // A qualidade é definida em applyVideoQualities(); o LiveKit ainda reduz sozinho se a
    // conexão de quem assiste não aguentar.
    adaptiveStream: false,
    dynacast: true, // quem transmite deixa de enviar camadas que ninguém está vendo
    webAudioMix: true, // permite volume individual acima de 100%
    audioCaptureDefaults: micConstraints(),
    audioOutput: vset.output === "default" ? undefined : { deviceId: vset.output },
    publishDefaults: {
      audioPreset: { maxBitrate: m.bitrate * 1000 },
      dtx: true,
      red: true,
      videoCodec: vset.codec === "auto" ? "h264" : vset.codec,
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
  playSfx("voice_join");
  ensureSounds(m.server_id);
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
  if (channel) playSfx("voice_leave");
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
  voice.reconnecting = false;
  voice.speaking.clear();
  voice.watching.clear();
  voice.focus = null;
  lastServerMute = { muted: false, deafened: false };
  for (const v of voice.videoEls.values()) v.remove();
  voice.videoEls.clear();
  stopStats();
  closeSoundpad();
  $("#voice-audio").replaceChildren();
}

/** O servidor encerrou a sessão (canal apagado, banimento etc.). */
function onVoiceEnded(m) {
  if (voice.room) playSfx("voice_leave");
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
    .on(E.ParticipantConnected, () => { playSfx("user_join"); renderVoiceAll(); })
    .on(E.ParticipantDisconnected, (p) => { voice.watching.delete(p.identity); playSfx("user_leave"); renderVoiceAll(); })
    // Queda de rede: o LiveKit tenta reconectar sozinho.
    .on(E.Reconnecting, () => { voice.reconnecting = true; playSfx("disconnect"); renderVoiceControls(); })
    .on(E.Reconnected, () => { voice.reconnecting = false; playSfx("reconnect"); renderVoiceControls(); })
    .on(E.TrackMuted, () => renderVoiceAll())
    .on(E.TrackUnmuted, () => renderVoiceAll())
    .on(E.LocalTrackPublished, (pub) => {
      if (pub.source === SRC.ScreenShare) playSfx("stream", true);
      syncVoiceState();
      renderVoiceAll();
    })
    .on(E.LocalTrackUnpublished, (pub) => {
      if (pub.source === SRC.ScreenShare) playSfx("stream", false);
      syncVoiceState();
      renderVoiceAll();
    })
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
      playSfx("voice_leave");
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
  const deaf = voice.deafened || myServerMute().deafened;
  p.setVolume(deaf ? 0 : (vset.volumes[p.identity] ?? 1));
  p.setVolume(deaf ? 0 : (vset.streamVolumes?.[p.identity] ?? 1), SRC.ScreenShareAudio);
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
  playSfx("mute", voice.muted);
  await applyMic();
  voice.room?.remoteParticipants.forEach(applyVolume);
  syncVoiceState();
  renderVoiceControls();
}

async function toggleDeafen() {
  voice.deafened = !voice.deafened;
  playSfx("deafen", voice.deafened);
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
    const codec = await pickCodec(preset.resolution.width, preset.resolution.height, 30, preset.encoding.maxBitrate);
    await lp.setCameraEnabled(!lp.isCameraEnabled, { resolution: preset.resolution }, { simulcast: true, videoCodec: codec });
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
  if (lp.isScreenShareEnabled) return stopScreenShare();
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

  // Captura feita aqui (e não pelo SDK) para usar todas as opções do navegador: áudio sem o
  // processamento de voz, áudio da janela (Chrome recente) e sem o eco da própria chamada.
  let stream;
  try {
    stream = await navigator.mediaDevices.getDisplayMedia({
      video: { width: { ideal: width }, height: { ideal: height }, frameRate: { ideal: fps, max: fps } },
      audio: r.audio
        ? {
            echoCancellation: false,
            noiseSuppression: false,
            autoGainControl: false,
            channelCount: 2,
            sampleRate: 48000,
            suppressLocalAudioPlayback: false,
            restrictOwnAudio: true, // não manda de volta o som da própria chamada
          }
        : false,
      systemAudio: r.audio ? "include" : "exclude",
      windowAudio: r.audio ? "window" : "exclude",
      selfBrowserSurface: "exclude",
      surfaceSwitching: "include",
      monitorTypeSurfaces: "include",
    });
  } catch (e) {
    if (e?.name !== "NotAllowedError") {
      console.error(e);
      toast("Não foi possível capturar a tela");
    }
    return;
  }
  const [videoTrack] = stream.getVideoTracks();
  const [audioTrack] = stream.getAudioTracks();
  videoTrack.contentHint = r.hint;
  // "Parar de compartilhar" na barra do navegador: encerra vídeo e áudio juntos.
  videoTrack.addEventListener("ended", () => stopScreenShare());
  if (audioTrack) audioTrack.contentHint = "music";
  const cap = videoTrack.getSettings();
  const capW = cap.width ?? width;
  const capH = cap.height ?? height;

  const codec = await pickCodec(capW, capH, fps, maxBitrate);
  // VP9 e AV1 usam camadas SVC; H.264 usa simulcast com uma camada 720p extra para conexões
  // fracas, na mesma taxa de quadros (fluidez primeiro).
  const svc = codec === "vp9" || codec === "av1";
  const lowRate = fps > 30 ? 2_500_000 : 1_500_000;
  try {
    await lp.publishTrack(videoTrack, {
      source: SRC.ScreenShare,
      videoCodec: codec,
      screenShareEncoding: { maxBitrate, maxFramerate: fps, priority: "high" },
      simulcast: !svc && Math.max(capW, capH) > 1280,
      screenShareSimulcastLayers: svc ? undefined : [new LK.VideoPreset(1280, 720, lowRate, fps)],
      scalabilityMode: svc ? "L3T3_KEY" : undefined,
      degradationPreference: r.hint === "motion" ? "maintain-framerate" : "maintain-resolution",
    });
    if (audioTrack) {
      await lp.publishTrack(audioTrack, {
        source: SRC.ScreenShareAudio,
        // Música e jogos: estéreo, 128 kbps e sem DTX (o DTX corta trechos baixos; é feito para voz).
        audioPreset: { maxBitrate: 128_000 },
        dtx: false,
        red: false,
        forceStereo: true,
      });
    }
  } catch (e) {
    console.error(e);
    stream.getTracks().forEach((t) => t.stop());
    await stopScreenShare();
    toast("Não foi possível transmitir a tela");
    return;
  }
  if (r.audio && !audioTrack) toast(noShareAudioHint(cap.displaySurface));
  syncVoiceState();
  renderVoiceAll();
}

async function stopScreenShare() {
  const lp = voice.room?.localParticipant;
  for (const src of [SRC.ScreenShare, SRC.ScreenShareAudio]) {
    const pub = lp?.getTrackPublication(src);
    if (pub?.track) await lp.unpublishTrack(pub.track, true).catch(() => {});
  }
  syncVoiceState();
  renderVoiceAll();
}

/** O navegador não entregou áudio: explica o que marcar na janela de escolha. */
function noShareAudioHint(surface) {
  if (surface === "browser") return "Transmitindo sem áudio. Para incluir o som, escolha a aba de novo e deixe marcado “Compartilhar áudio da guia”.";
  if (surface === "monitor") return "Transmitindo sem áudio. Para incluir o som, escolha a tela de novo e marque “Também compartilhar o áudio do sistema”.";
  return "Transmitindo sem áudio: este navegador não captura o som de uma janela. Para ter som, transmita a Tela inteira (marcando o áudio do sistema) ou uma aba.";
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
        const enc = await encoderInfo(screenTrack);
        if (enc) text += ` · ${enc}`;
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

/** "H264 · GPU" ou "VP9 · CPU": ajuda a entender quedas de fps no envio. */
async function encoderInfo(track) {
  try {
    const report = await track.sender?.getStats();
    let best = null;
    report?.forEach((r) => {
      if (r.type === "outbound-rtp" && r.kind === "video" && (!best || (r.frameWidth ?? 0) > (best.frameWidth ?? 0))) best = r;
    });
    if (!best) return null;
    const codec = report.get(best.codecId)?.mimeType?.split("/")[1];
    const hw = best.powerEfficientEncoder;
    return [codec, hw === true ? "GPU" : hw === false ? "CPU" : null].filter(Boolean).join(" · ") || null;
  } catch {
    return null;
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

/** Volumes individuais: só mudam o que você ouve, neste navegador. */
let volumeSaveTimer = null;
function saveVolumesSoon() {
  clearTimeout(volumeSaveTimer);
  volumeSaveTimer = setTimeout(saveVset, 400);
}

function setUserVolume(id, v) {
  vset.volumes[id] = v;
  saveVolumesSoon();
  const p = voice.room?.getParticipantByIdentity(id);
  if (p) applyVolume(p);
}

function setStreamVolume(id, v) {
  vset.streamVolumes ??= {};
  vset.streamVolumes[id] = v;
  saveVolumesSoon();
  const p = voice.room?.getParticipantByIdentity(id);
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
  $("#vp-status").textContent = voice.connecting ? "Conectando…" : voice.reconnecting ? "Reconectando…" : voice.pttDown ? "Voz conectada · falando" : "Voz conectada";
  $("#vp-where").textContent = ch ? `${ch.name} / ${server.name}` : "";
  const lp = room?.localParticipant;
  $("#vp-camera").classList.toggle("active", !!lp?.isCameraEnabled);
  $("#vp-screen").classList.toggle("active", !!lp?.isScreenShareEnabled);
  $("#vp-screen span").textContent = lp?.isScreenShareEnabled ? "Parar" : "Tela";
  $("#voice-panel").classList.toggle("reconnecting", voice.reconnecting);
  $("#vp-soundpad").classList.toggle("active", !!sp.pad);
  updateCallBar();
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
    // Volume do áudio da transmissão, na própria barra (funciona também em tela cheia).
    const vol = volumeSlider(vset.streamVolumes?.[t.identity] ?? 1, (v) => setStreamVolume(t.identity, v), { compact: true });
    vol.title = "Volume da transmissão (só para você)";
    tools.prepend(vol);
  }
  if (!t.local && t.kind === "user") {
    const vol = volumeSlider(vset.volumes[t.identity] ?? 1, (v) => setUserVolume(t.identity, v), { compact: true });
    vol.title = `Volume de ${t.user.name} (só para você)`;
    tools.prepend(vol);
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
  view.append(callBar());
  applyVideoQualities(tiles, focused);
  updateSpeaking();
  renderVoiceControls();
}

/**
 * Transmissões de tela: sempre a qualidade original (como no Discord). Câmeras: alta em destaque,
 * média na grade e baixa na faixa lateral ou com muita gente.
 */
function applyVideoQualities(tiles, focused) {
  const Q = LK.VideoQuality;
  for (const t of tiles) {
    if (t.local) continue;
    const p = voice.room?.getParticipantByIdentity(t.identity);
    const pub = p?.getTrackPublication(t.kind === "screen" ? SRC.ScreenShare : SRC.Camera);
    if (!pub?.isSubscribed) continue;
    let q;
    if (t.kind === "screen") q = Q.HIGH;
    else if (focused) q = t === focused ? Q.HIGH : Q.LOW;
    else q = tiles.length > 6 ? Q.LOW : Q.MEDIUM;
    pub.setVideoQuality(q);
  }
}

// ---------- barra de controles da chamada (essencial no celular) ----------
Object.assign(ICONS, {
  mic: "M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3zM19 10v2a7 7 0 0 1-14 0v-2M12 19v4",
  micOff: "M1 1l22 22M9 9v3a3 3 0 0 0 5.1 2.1M15 9.3V4a3 3 0 0 0-5.9-.6M17 16.9A7 7 0 0 1 5 12v-2m14 0v2c0 .8-.1 1.5-.4 2.2M12 19v4",
  headset: "M3 18v-6a9 9 0 0 1 18 0v6M21 19a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3zM3 19a2 2 0 0 0 2 2h1a2 2 0 0 0 2-2v-3a2 2 0 0 0-2-2H3z",
  headsetOff: "M1 1l22 22M3 18v-6a9 9 0 0 1 14.5-7.1M21 12v6M3 19a2 2 0 0 0 2 2h1a2 2 0 0 0 2-2v-3a2 2 0 0 0-2-2H3z",
  camera: "M23 7l-7 5 7 5zM3 5h11a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z",
  screen: "M4 3h16a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zM8 21h8M12 17v4",
});
const canShareScreen = !!navigator.mediaDevices?.getDisplayMedia;

function callBar() {
  const bar = el("div", "vv-controls");
  const btn = (act, title, onClick, cls = "") => {
    const b = el("button", `cb-btn ${cls}`.trim());
    b.type = "button";
    b.dataset.act = act;
    b.title = title;
    b.setAttribute("aria-label", title);
    b.onclick = (e) => { e.stopPropagation(); onClick(e); };
    return b;
  };
  bar.append(
    btn("mic", "Microfone", toggleMute),
    btn("deaf", "Áudio", toggleDeafen),
    btn("camera", "Câmera", toggleCamera),
  );
  if (canShareScreen) bar.append(btn("screen", "Transmitir tela", toggleScreenShare));
  bar.append(
    btn("sounds", "Soundpad", (e) => openSoundpad(e.currentTarget)),
    btn("leave", "Desconectar", voiceLeave, "danger"),
  );
  return bar;
}

/** Atualiza ícones e estados da barra (chamado junto com o painel de voz). */
function updateCallBar() {
  const bar = document.querySelector(".vv-controls");
  if (!bar) return;
  const mod = voice.room ? myServerMute() : { muted: false, deafened: false };
  const lp = voice.room?.localParticipant;
  const micOff = voice.muted || voice.deafened || mod.muted || mod.deafened;
  const deafOff = voice.deafened || mod.deafened;
  const set = (act, iconName, on, off) => {
    const b = bar.querySelector(`[data-act="${act}"]`);
    if (!b) return;
    b.replaceChildren(icon(iconName, "cb-icon"));
    b.classList.toggle("on", !!on);
    b.classList.toggle("off", !!off);
  };
  set("mic", micOff ? "micOff" : "mic", false, micOff);
  set("deaf", deafOff ? "headsetOff" : "headset", false, deafOff);
  set("camera", "camera", lp?.isCameraEnabled, false);
  set("screen", "screen", lp?.isScreenShareEnabled, false);
  set("sounds", "music", !!sp.pad, false);
  set("leave", "hangup", false, false);
}

// ---------- ligações com a interface ----------
$("#btn-mute").onclick = toggleMute;
$("#btn-deafen").onclick = toggleDeafen;
$("#btn-voice-settings").onclick = () => openSettings("user", "account");
$("#vp-leave").onclick = voiceLeave;
$("#vp-camera").onclick = toggleCamera;
$("#vp-screen").onclick = toggleScreenShare;
if (!canShareScreen) $("#vp-screen").classList.add("hidden"); // navegadores de celular não transmitem tela
window.addEventListener("beforeunload", () => voice.room?.disconnect());
renderVoiceControls();
