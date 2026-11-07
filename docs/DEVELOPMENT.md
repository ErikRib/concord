# Desenvolvimento

Como preparar o ambiente, testar e manter o padrão do projeto. A visão do código está em
[ARCHITECTURE.md](ARCHITECTURE.md).

## Ambiente

- **Rust** estável (`rustup`). O projeto compila em Linux, macOS e Windows.
- **Node 22+** para os testes de ponta a ponta.
- **Google Chrome** para o teste de mídia (`tests/media.mjs`).
- **LiveKit** opcional, para voz local (veja [VOICE.md](VOICE.md)).

### Windows sem administrador

Sem o Visual Studio Build Tools, use a toolchain **GNU** e o MinGW do WinLibs:

```powershell
rustup default stable-x86_64-pc-windows-gnu
winget install BrechtSanders.WinLibs.POSIX.UCRT --scope user
```

O MinGW fornece o `gcc` (SQLite e TLS são compilados junto) e o `dlltool` completo. Coloque a
pasta `bin` do WinLibs no `PATH` do usuário. Com o Build Tools instalado, a toolchain padrão (MSVC)
também funciona.

**Dica:** se um Concord estiver rodando, o Windows trava `target\debug\concord.exe`. Compile em
outra pasta com `cargo build --target-dir <outra-pasta>`.

## Testes

Antes de cada commit:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

O CI do GitHub (`.github/workflows/ci.yml`) roda esses três e o teste de ponta a ponta.

### Ponta a ponta (`tests/e2e.mjs`)

Testa o protocolo com vários clientes: permissões, moderação, LGPD, paginação, chat, arquivos,
Soundpad e voz (os testes de voz são pulados se não houver LiveKit). Precisa de um servidor de
teste com **banco novo e vazio** na porta 3999. No PowerShell:

```powershell
$env:PORT="3999"; $env:CONCORD_DB="$env:TEMP\concord-e2e.db"; Remove-Item "$env:CONCORD_DB*" -ErrorAction SilentlyContinue
$env:CONCORD_DATA="$env:TEMP\nao-existe.json"
$env:CONCORD_CONTROLADOR="Teste <Ltda>"; $env:CONCORD_CONTATO="dpo@exemplo.com"
$env:CONCORD_ADMINS = node tests/e2e.mjs --admin-id
$env:CONCORD_LIVEKIT_PORT="7890"; $env:CONCORD_LIVEKIT_UDP_START="50300"   # não colide com outro Concord
cargo run
```

Em outro terminal: `node tests/e2e.mjs 3999`. No bash, troque `$env:X="..."` por `export X=...`.

O servidor limita cada conexão a 10 ações em rajada e 3 por segundo. Testes novos que mandam muitas
mensagens seguidas precisam de pausas (`await sleep(3400)` repõe a rajada).

### Mídia real (`tests/media.mjs`)

Dois Chromes numa chamada pelo LiveKit: áudio, transmissão de tela (resolução, fps, áudio da
transmissão, qualidade pedida por quem assiste) e moderação de voz contra um cliente que tenta
burlar. Precisa de LiveKit e da "Ana" do teste como administradora:

```powershell
npm install --prefix tests
$env:CONCORD_ADMINS = node tests/media.mjs --admin-id    # antes de iniciar o servidor de teste
node tests/media.mjs 3999
$env:HEADED="1"; node tests/media.mjs 3999               # com janelas, para medir fps real com a GPU
```

Sem janelas, a aba transmitida fica congelada e o fps medido não reflete uma máquina real.

### Interface

Mudanças visuais são conferidas com capturas de tela (Puppeteer) em **1440 px** e **390 px**
(celular). Os scripts de captura não fazem parte do repositório. Basta um script que abra duas
pessoas, use `send(...)` e funções globais do cliente para montar a cena, e salve
`page.screenshot()`.

## Convenções

- **Português** na interface, nas mensagens de erro, nos comentários e nos commits.
- **Todo o SQL em `src/db.rs`.** O resto do código chama funções desse módulo.
- **Toda permissão em `src/commands.rs`**, nunca só no cliente.
- **Toda alteração por `AppState::mutate`**, para manter banco e cache juntos.
- **Esquema só por migração nova** no fim de `MIGRATIONS`.
- **Interface pelo design system** ([DESIGN.md](DESIGN.md)): só os tokens de `style.css`, componentes
  existentes primeiro, funcionando no celular. Todo volume é uma barra (`volumeSlider`).
- **Sons:** ações novas que merecem retorno ganham um efeito em `static/sounds.js` (`SFX` e
  `SFX_EVENTS`, para poder ser desligado). Sem arquivos de áudio com direitos autorais.
- **Mensagens de commit** curtas no título e com o porquê no corpo.

## Checklist de funcionalidade nova

1. **Dados:** qual dado novo é tratado? Use o mínimo necessário.
2. **Permissão:** quem pode fazer? Verifique em `commands.rs` e teste a recusa no e2e.
3. **LGPD** ([LGPD.md](LGPD.md)):
   - entra na **exportação** (`AppState::export_user_data`)?
   - é apagado em **"Apagar meus dados"** (`db::delete_user_rows`)?
   - tem **retenção** (`purge_expired`)?
   - acrescente a linha no inventário e as medidas em `docs/LGPD.md`;
   - se mudar o tratamento, atualize `static/privacidade.html` e **suba `POLICY_VERSION`** e
     `POLICY_UPDATED_AT` em `src/state.rs`. Atualize o `consent_version` dos testes.
4. **Segurança:** limites de tamanho e de quantidade, entrada validada, nada de HTML montado com
   texto do usuário (use `textContent`).
5. **Interface:** design system, celular (toque longo, gavetas, alvos de 40 px), efeito sonoro se
   fizer sentido.
6. **Testes:** unidade para lógica pura, e2e para o protocolo, captura de tela para a interface.
7. **Documentação:** README (funcionalidades), [ROADMAP.md](ROADMAP.md) (feito), DESIGN.md
   (componente novo).

## Publicar uma versão

```
git archive --format=tar.gz -o concord.tar.gz HEAD
```

Depois siga "Atualizar" em [deploy/LIGHTSAIL.md](../deploy/LIGHTSAIL.md). O `setup.sh` compila no
servidor e mantém dados, chaves e configuração.
