# Voz, câmera e transmissão de tela

A mídia passa pelo **LiveKit**, um servidor de mídia (SFU) de código aberto. Quem transmite envia
uma vez, e o LiveKit repassa para cada pessoa na camada de qualidade que a conexão dela aguenta,
como no Discord. O Concord decide quem pode entrar e emite um token de 10 minutos. **Nada é
gravado.**

## Uso local

Se `tools/livekit/livekit-server.exe` (ou o caminho em `CONCORD_LIVEKIT_BIN`) existir, o Concord inicia o
LiveKit sozinho, com uma chave secreta nova a cada execução, passada por variável de ambiente, e o
encerra junto. No Windows isso usa um Job Object, então funciona mesmo se o Concord travar.

Para instalar, baixe `livekit_<versão>_windows_amd64.zip` (o projeto usa a 1.13.11) em
<https://github.com/livekit/livekit/releases>, confira o SHA-256 com o `checksums.txt` da release e
extraia em `tools/livekit/`. A pasta `tools/` não vai para o Git.

Na primeira chamada, o Windows pode pedir permissão de rede para o `livekit-server.exe`. Permita
em **redes privadas**. O LiveKit usa TCP 7880 (sinalização) e 7881, e UDP 50000–50100 (mídia).

Se a porta do LiveKit já estiver em uso (por exemplo, outro Concord aberto), a voz é desativada com
um aviso, em vez de mandar tokens para o LiveKit errado.

## Produção

Rode o LiveKit à parte, com HTTPS e TURN. O `deploy/setup.sh` já faz isso: o LiveKit fica atrás do
Caddy no mesmo domínio do site (`/rtc`), o que evita bloqueio por extensões de anúncio, e o TURN usa
um segundo domínio.

## Configuração

| Variável | Padrão | Para que serve |
|---|---|---|
| `CONCORD_LIVEKIT_URL` | — | URL `wss://` do LiveKit externo (produção) |
| `CONCORD_LIVEKIT_KEY`, `CONCORD_LIVEKIT_SECRET` | — | Chave e segredo do LiveKit externo |
| `CONCORD_LIVEKIT_API_URL` | — | `http://` da API do LiveKit pela rede interna, para aplicar a moderação |
| `CONCORD_LIVEKIT_BIN` | `tools/livekit/livekit-server.exe` | Executável do LiveKit local (em Linux/macOS, defina o caminho) |
| `CONCORD_LIVEKIT_PORT` | `7880` | Porta do LiveKit local (a mídia usa TCP porta+1) |
| `CONCORD_LIVEKIT_UDP_START` | `50000` | Início da faixa UDP de mídia (100 portas) |
| `CONCORD_LIVEKIT_PUBLIC_URL` | — | URL `wss://` pela qual os navegadores alcançam o LiveKit local (ex.: Tailscale) |

## Recursos

- Entrar com um clique no canal de voz, com indicador de quem está falando
- Mutar e ensurdecer; ativação por voz ou apertar para falar, com tecla configurável
- Supressão de ruído, cancelamento de eco e ganho automático; escolha de microfone e saída
- Volume individual de cada pessoa e de cada transmissão (barra de 0 a 200%)
- Câmera com camadas de qualidade
- Status do canal (some quando o canal esvazia) e configurações: limite de pessoas, taxa de bits do
  áudio (8–384 kbps), qualidade da câmera
- Soundpad: sons tocados para todos da chamada

### Transmissão de tela

- Presets de 720p30 a **1080p60** (recomendado), 1440p60, 4K30 e 4K60
- Otimização para fluidez (jogos) ou nitidez (texto)
- Áudio da aba, da tela inteira ou (Chrome recente) da janela, em estéreo a 128 kbps, sem filtros de
  voz. No Windows, para som da tela inteira, marque "Também compartilhar o áudio do sistema". Se o
  navegador não entregar áudio, o Concord explica o que marcar
- Codec automático: escolhe H.264, AV1 ou VP9 com aceleração na GPU (`navigator.mediaCapabilities`)
- Quem assiste recebe a **qualidade original**: a qualidade é pedida explicitamente, sem o
  `adaptiveStream` do SDK, que escolhia a camada pelo tamanho do vídeo na tela
- Estatísticas reais nos dois lados: resolução, fps, taxa de bits, codificador (GPU/CPU) e motivo
  de limitação (CPU ou banda)

**Sobre a qualidade:** o LiveKit não é o gargalo. O limite real é o codificador do navegador (a GPU)
e o upload de quem transmite. Para igualar o aplicativo de desktop do Discord em 1440p e 4K, o
próximo passo seria um app de desktop (por exemplo, Tauri) com o SDK nativo do LiveKit.

## Moderação de voz

Clique no nome da pessoa:

- **Silenciar no servidor:** o LiveKit retira a permissão de microfone. Nem um cliente modificado
  consegue falar (testado em `tests/media.mjs`).
- **Ensurdecer no servidor:** silencia e corta o áudio no navegador da pessoa.
- **Mover para outro canal** e **Desconectar da voz** (removido no próprio LiveKit).

Banimento, saída de servidor privado e exclusão de canal ou servidor também tiram a pessoa da sala
no LiveKit. Toda ação entra no registro de moderação.

## Testar em vários aparelhos sem servidor (Tailscale)

O navegador só libera microfone, câmera e tela em **HTTPS** ou em `localhost`. O Tailscale cria uma
rede privada entre os seus aparelhos, com HTTPS e UDP:

1. Instale o Tailscale nos aparelhos, com a mesma conta. No painel, ative **MagicDNS** e
   **HTTPS Certificates**.
2. No computador que roda o Concord:
   ```powershell
   tailscale serve --bg --https=443 http://localhost:3000
   tailscale serve --bg --https=8443 http://localhost:7880
   $env:CONCORD_LIVEKIT_PUBLIC_URL = "wss://SUA-MAQUINA.SEU-TAILNET.ts.net:8443"
   $env:CONCORD_ORIGENS = "https://SUA-MAQUINA.SEU-TAILNET.ts.net"
   cargo run --release
   ```
3. Nos outros aparelhos, abra `https://SUA-MAQUINA.SEU-TAILNET.ts.net`.

O Cloudflare Tunnel serve para o chat, mas não repassa o UDP do WebRTC.
