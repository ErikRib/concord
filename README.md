# Concord

Um chat no estilo Discord feito em Rust, **sem cadastro de usuário**: quem abre o site só escolhe um
apelido. Tem servidores e canais, voz, câmera e transmissão de tela em até 4K60, Soundpad, envio
de arquivos, moderação e versão para celular. Foi construído desde o início seguindo a **LGPD**.

- **Backend:** Rust (axum + tokio), SQLite embutido, um único executável
- **Frontend:** HTML, CSS e JavaScript puros, embutidos no executável (sem etapa de build)
- **Voz e vídeo:** [LiveKit](https://livekit.io) (SFU de código aberto), que o Concord inicia e controla

## Funcionalidades

| Área | O que tem |
|---|---|
| **Chat** | Mensagens em tempo real, histórico paginado, "digitando…", reações, edição, respostas com prévia, menções com aviso, seletor de emoji |
| **Arquivos** | Qualquer tipo, até 25 MB e 10 por mensagem. Imagens com visualizador, vídeo e áudio com player, outros arquivos para baixar. GIF colado de um site mantém a animação |
| **Servidores** | Públicos ou privados com convite, canais de texto e de voz, categorias recolhíveis, lista de membros por papel |
| **Moderação** | Dono, moderadores, denúncias anônimas, banimentos com motivo, registro de moderação |
| **Voz** | Ativação por voz ou apertar para falar, supressão de ruído, volume por pessoa, status e configurações do canal, moderação de voz aplicada no servidor de mídia |
| **Tela e câmera** | Transmissão de 720p30 a 4K60 com áudio, codec automático com GPU, estatísticas reais, volume próprio para quem assiste |
| **Sons** | Efeitos sonoros configuráveis e Soundpad por servidor, com sons padrão sintetizados |
| **Celular** | Gavetas com deslize, toque longo, painéis que sobem de baixo, barra de controles da chamada |
| **LGPD** | Consentimento versionado, exportação (JSON), correção, eliminação imediata, retenção automática, metadados removidos de fotos, vídeos e sons |

## Como rodar

Requisitos: [Rust](https://rustup.rs) estável. Para voz, o executável do LiveKit (veja [docs/VOICE.md](docs/VOICE.md)).

```
cargo run --release
```

Abra <http://localhost:3000>. Outras pessoas na mesma rede acessam por `http://SEU-IP:3000`, mas
microfone, câmera e tela só funcionam em `localhost` ou com HTTPS.

**Colocar na internet:** [deploy/README.md](deploy/README.md). Um script configura tudo (HTTPS, voz,
firewall, backup) numa VPS. A instância atual usa a AWS Lightsail em São Paulo.

## Configuração

Tudo por variáveis de ambiente. Nenhuma é obrigatória para rodar localmente.

| Variável | Padrão | Para que serve |
|---|---|---|
| `PORT` | `3000` | Porta HTTP |
| `CONCORD_HOST` | `0.0.0.0` | Endereço em que o Concord escuta (em servidor, `127.0.0.1` atrás do proxy) |
| `CONCORD_DB` | `concord.db` | Banco SQLite (tem dados pessoais: proteja o arquivo) |
| `CONCORD_DATA` | `concord-data.json` | Formato antigo, importado uma única vez se o banco estiver vazio |
| `CONCORD_RETENCAO_DIAS` | `180` | Prazo para apagar mensagens e registros |
| `CONCORD_CONTROLADOR` | — | Nome do controlador, mostrado na política de privacidade |
| `CONCORD_CONTATO` | — | Contato do encarregado (DPO) / canal do titular |
| `CONCORD_ADMINS` | — | Ids pseudônimos dos administradores da instância, separados por vírgula |
| `CONCORD_ORIGENS` | — | Origens extras aceitas no WebSocket (quando há proxy na frente) |
| `CONCORD_LIVEKIT_*` | — | Voz: veja [docs/VOICE.md](docs/VOICE.md#configuração) |

### Como virar administrador

Não há conta, então o administrador é identificado pelo **id pseudônimo**:

1. Abra o Concord no navegador que você vai usar para administrar.
2. Em Configurações (engrenagem) → Minha conta, copie o id pseudônimo.
3. Inicie o servidor com `CONCORD_ADMINS=<esse id>`.

Administradores moderam o servidor público "Concord", que não tem dono, e os servidores públicos.
Eles **não** veem servidores privados dos quais não participam. Se os dados daquele navegador forem
apagados, o id muda e é preciso repetir o processo.

## Comandos

```
cargo run --release                        # servidor
cargo run --release -- backup copia.db     # cópia consistente do banco com o servidor rodando
cargo run --release -- bench 1000000       # teste de carga num banco temporário
cargo test                                 # testes de unidade
node tests/e2e.mjs                         # ponta a ponta (veja docs/DEVELOPMENT.md)
```

## Documentação

| Documento | Conteúdo |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Módulos, fluxo de uma mensagem, protocolo, banco de dados, permissões |
| [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) | Ambiente, testes, convenções e checklist para funcionalidades novas |
| [docs/VOICE.md](docs/VOICE.md) | Voz, câmera e tela: LiveKit local e em produção, qualidade, moderação |
| [docs/DESIGN.md](docs/DESIGN.md) | Design system: tokens, componentes, celular |
| [docs/LGPD.md](docs/LGPD.md) | Registro do tratamento, direitos do titular, medidas de segurança, pendências |
| [docs/ROADMAP.md](docs/ROADMAP.md) | O que já foi feito e o que vem a seguir |
| [deploy/README.md](deploy/README.md) | Colocar o Concord no ar |
| [SECURITY.md](SECURITY.md) | Como relatar uma vulnerabilidade |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Como contribuir |

## Estrutura

```
src/
  main.rs       servidor HTTP, rotas, cabeçalhos de segurança, retenção, subcomandos
  ws.rs         conexões WebSocket: identidade, consentimento, limite de taxa, filtro de eventos
  commands.rs   comandos dos clientes, com TODAS as verificações de permissão
  state.rs      estado, cache em memória, transações e operações da LGPD
  db.rs         todo o SQL: esquema, migrações e consultas
  model.rs      tipos, papéis e protocolo cliente/servidor
  voice.rs      LiveKit: inicia o servidor local, assina tokens, API de administração
  uploads.rs    envio e entrega de arquivos e sons (links assinados)
  media.rs      identificação de formato e limpeza de metadados
  fetch.rs      busca do GIF original ao colar (com proteção contra SSRF)
  legacy.rs     importação única do formato JSON antigo
  bench.rs      teste de carga
static/         cliente web (embutido no executável): index.html, style.css, app.js,
                chat.js, voice.js, sounds.js, privacidade.html, vendor/ (SDK do LiveKit)
tests/          e2e.mjs (protocolo) e media.mjs (navegador real + LiveKit)
deploy/         setup.sh e guias de implantação
docs/           documentação
```

## Licença

Ainda não definida. Até que um arquivo `LICENSE` seja adicionado, todos os direitos são reservados
ao autor. O SDK do LiveKit em `static/vendor/` é Apache-2.0.
