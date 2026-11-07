# Arquitetura

Visão para quem vai mexer no código. Para rodar e testar, veja [DEVELOPMENT.md](DEVELOPMENT.md).

## Visão geral

```
navegador ──HTTPS──> Caddy ──> Concord (axum)            ──> SQLite (concord.db)
    │                   │          │  WebSocket /ws (JSON)
    │                   │          │  HTTP /anexos, /sons
    │                   └─/rtc──> LiveKit (sinalização)  <── Concord: tokens JWT + API admin
    └──────UDP/TCP (mídia WebRTC)──> LiveKit
```

- **Um executável.** O cliente web (`static/`) entra no binário com `include_str!`. Não há build de
  frontend.
- **Um processo, muitas tarefas.** Cada conexão WebSocket é uma tarefa tokio. O estado compartilhado
  fica em `AppState` (`src/state.rs`), dentro de um `Arc`.
- **SQLite é a fonte da verdade.** Um cache em memória (`Cache`: servidores, canais, papéis, perfis)
  atende as consultas frequentes. Mensagens, arquivos e reações são lidas do banco.
- **Voz fora do Concord.** Áudio e vídeo vão direto entre o navegador e o LiveKit. O Concord só
  decide quem entra (token) e aplica moderação pela API de administração do LiveKit.

## Identidade sem conta

1. Na primeira visita, o navegador gera um segredo aleatório de 256 bits e guarda no `localStorage`.
2. Na conexão, manda `hello { secret, name, color, consent_version }`.
3. O servidor calcula `id = SHA-256("concord-user-id:" + segredo)` (16 primeiros bytes, formato UUID).
   O segredo nunca é gravado. Saber o id de alguém não permite se passar por essa pessoa.
4. Se `consent_version` for menor que `POLICY_VERSION` (`src/state.rs`), o servidor responde
   `consent_required` e não grava nada. Subir `POLICY_VERSION` pede novo aceite a todos.

## Fluxo de uma mensagem

```
cliente: send_message ─> ws.rs: limite de taxa, JSON -> ClientMsg
                     ─> commands.rs: handle() verifica permissão
                     ─> state.mutate(): transação SQLite + atualização do cache
                     ─> state.emit(Event::Send(Audience::Server(id), ServerMsg::Message))
                     ─> broadcast para todas as conexões
                     ─> ws.rs: deliver() filtra pela Audience de cada conexão
                     ─> clientes que podem ver o servidor recebem "message"
```

Pontos importantes:

- **Toda permissão é verificada em `src/commands.rs`.** O cliente só esconde botões.
- **`AppState::mutate`** abre uma transação `IMMEDIATE`, roda a função com o cache e a transação, e
  só confirma se tudo der certo. Se o banco falhar, o cache é recarregado do banco.
- **Eventos são filtrados por conexão.** `Audience::All`, `Server(id)` (quem vê o servidor) e
  `User(id)`. `Event::ServerChanged` gera um `ServerView` diferente para cada pessoa: quem modera vê
  convite e banidos, o membro comum não.
- **Erros:** `Fail::User` vira uma mensagem para a pessoa; `Fail::Internal` é registrado no log e a
  pessoa vê uma mensagem genérica.

## Protocolo

JSON pelo WebSocket `/ws`, com o campo `type` em snake_case. As definições estão em `src/model.rs`.

**Cliente → servidor** (`ClientMsg`):

| Grupo | Mensagens |
|---|---|
| Conexão e perfil | `hello` (também troca apelido e cor) |
| Servidores | `create_server`, `update_server`, `delete_server`, `regenerate_invite`, `join_server`, `leave_server` |
| Canais | `create_channel`, `update_channel`, `delete_channel` |
| Mensagens | `send_message` (com `reply_to` e `attachments`), `edit_message`, `react`, `delete_message`, `fetch_history`, `typing` |
| Arquivos | `upload_begin` (pede autorização), `upload_from_url` (GIF colado) |
| Moderação | `set_moderator`, `ban_user`, `unban_user`, `report_message`, `fetch_reports`, `resolve_report` |
| Voz | `voice_join`, `voice_leave`, `voice_update`, `set_voice_status`, `voice_moderate`, `voice_move` |
| Soundpad | `fetch_sounds`, `sound_upload_begin`, `play_sound`, `delete_sound` |
| LGPD | `export_my_data`, `delete_my_data` |

**Servidor → cliente** (`ServerMsg`): `ready`, `consent_required`, `server_updated`,
`server_removed`, `focus`, `message`, `message_edited`, `reaction_update`, `message_deleted`,
`messages_purged`, `history`, `presence`, `typing`, `reports`, `upload_token`, `url_upload`,
`sounds`, `sounds_changed`, `sound_played`, `voice_token`, `voice_states`, `voice_ended`,
`my_data`, `data_deleted`, `user_purged`, `profile_updated`, `info`, `error`.

### Envio de arquivos

O segredo de identidade nunca sai do WebSocket. Por isso o envio usa uma autorização de uso único:

1. `upload_begin { channel_id, size }` → servidor confere permissão e limites → `upload_token`.
2. `POST /anexos/{upload_id}?token=…&name=…` com o arquivo (até 25 MB, 2 minutos de validade).
3. O servidor identifica o formato **pelo conteúdo**, limpa metadados (`src/media.rs`) e guarda o
   arquivo como pendente.
4. `send_message { attachments: [id] }` liga o arquivo à mensagem. Pendentes somem em 1 hora.
5. Arquivos são entregues em `GET /anexos/{id}?exp=…&sig=…`, com link assinado por HMAC que vale
   cerca de 24 h. O que não é imagem, vídeo ou áudio sai sempre como download, com CSP `sandbox`.

Os sons do Soundpad seguem o mesmo esquema em `/sons/{id}`, com domínio de assinatura separado.

## Banco de dados

SQLite em modo WAL, com chaves estrangeiras ligadas. Todo o SQL fica em `src/db.rs`.

| Tabela | Conteúdo |
|---|---|
| `users` | id pseudônimo, apelido, cor, versão e data do aceite, último acesso |
| `servers`, `channels` | servidores (dono, público/privado, convite) e canais (texto/voz, configurações de voz) |
| `server_members`, `server_moderators`, `bans` | papéis e banimentos |
| `messages` | id UUIDv7, canal, autor, conteúdo, edição, resposta |
| `reactions` | mensagem, emoji, pessoa (`WITHOUT ROWID`) |
| `attachments` | arquivos (conteúdo no próprio banco, coluna `data` por último) |
| `sounds` | sons do Soundpad por servidor |
| `reports`, `mod_log` | denúncias e registro de moderação (sem conteúdo de mensagem) |

- **Migrações** são uma lista em `MIGRATIONS` (`src/db.rs`), controlada por `PRAGMA user_version`.
  Para mudar o esquema, **acrescente** uma migração no fim. Nunca altere uma que já foi publicada.
- **UUIDv7** já vem em ordem cronológica: paginar o histórico e apagar por idade usam o índice.
- **Apagar em cascata:** apagar servidor, canal ou mensagem leva junto reações, arquivos e denúncias.
- **Retenção:** uma tarefa roda a cada hora (`AppState::purge_expired`), em lotes.

## Voz

- `src/voice.rs` inicia o `livekit-server` local (ou usa um externo), assina tokens JWT HS256 e
  chama a API de administração (Twirp) para remover, silenciar e mover participantes.
- Quem está em qual canal fica **só em memória** (`voice_sessions`). Nada de áudio é gravado.
- Detalhes, configuração e qualidade: [VOICE.md](VOICE.md).

## Cliente web

Scripts simples (sem módulos), carregados nesta ordem e compartilhando o escopo global:

| Arquivo | Responsabilidade |
|---|---|
| `app.js` | Estado, conexão, `handle()` das mensagens do servidor, renderização, menus, configurações, gavetas do celular |
| `chat.js` | Compositor, menções, emoji, arquivos, reações, respostas, edição, visualizador |
| `sounds.js` | Efeitos sonoros e Soundpad (Web Audio, sem arquivos de áudio) |
| `voice.js` | Chamada (LiveKit), câmera, transmissão de tela, volumes, barra da chamada |

Como os scripts compartilham o escopo global, **confira se o nome de uma função nova já não existe
em outro arquivo** antes de criá-la.

## Segurança em camadas

| Camada | Onde |
|---|---|
| Origem do WebSocket, limite de taxa (10 em rajada, 3/s), tamanho de mensagem | `src/ws.rs` |
| Permissões e hierarquia de papéis | `src/commands.rs` |
| CSP, `X-Frame-Options`, `Referrer-Policy`, `Permissions-Policy` | `src/main.rs` |
| Formato pelo conteúdo, metadados, nomes de arquivo | `src/media.rs`, `src/uploads.rs` |
| Links assinados e autorizações de uso único | `src/state.rs` |
| SSRF na busca de GIF | `src/fetch.rs` |
| Tokens e moderação de voz aplicados no LiveKit | `src/voice.rs` |

A lista completa de riscos e medidas está em [LGPD.md](LGPD.md#4-medidas-de-segurança-art-46).
