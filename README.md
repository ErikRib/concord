# Concord

Um chat no estilo Discord feito em Rust, **sem cadastro de usuário**: quem abre o site só escolhe um
apelido. O projeto foi construído desde o início seguindo a LGPD (veja [docs/LGPD.md](docs/LGPD.md)).

## Como rodar

```
cargo run --release
```

Abra http://localhost:3000. Outras pessoas na mesma rede acessam por `http://SEU-IP:3000`.
Para uso fora da rede local, coloque o Concord atrás de um proxy reverso com HTTPS.

### Ambiente de compilação (Windows, sem administrador)

Esta máquina usa o Rust com a toolchain **GNU** (`stable-x86_64-pc-windows-gnu`) e o **MinGW-w64
(WinLibs)**, instalado via `winget install BrechtSanders.WinLibs.POSIX.UCRT --scope user`. O MinGW
fornece o `dlltool` completo que algumas dependências do Windows exigem. Com o Visual Studio Build
Tools instalado, a toolchain padrão (MSVC) também funciona.

## Configuração

| Variável | Padrão | Para que serve |
|---|---|---|
| `PORT` | `3000` | Porta HTTP |
| `CONCORD_DATA` | `concord-data.json` | Arquivo de dados (contém dados pessoais: proteja-o) |
| `CONCORD_RETENCAO_DIAS` | `180` | Prazo após o qual mensagens e registros inativos são apagados |
| `CONCORD_CONTROLADOR` | — | Nome do controlador, exibido na política de privacidade |
| `CONCORD_CONTATO` | — | Contato do encarregado (DPO) / canal do titular |

## Funcionalidades

- Servidores e canais de texto, mensagens em tempo real, lista de quem está online, aviso de "digitando…"
- Histórico salvo em disco (até 1000 mensagens por canal, respeitando o prazo de retenção)
- Apagar as próprias mensagens e trocar o apelido
- **LGPD:** tela de consentimento com política versionada; menu 🛡 com "Baixar meus dados" (JSON)
  e "Apagar meus dados" (eliminação imediata); retenção automática; página `/privacidade`

## Segurança

- Identidade sem senha: o navegador gera um segredo aleatório de 256 bits, e o servidor usa só o
  SHA-256 dele como id público. Saber o id de alguém não permite se passar por essa pessoa.
- Verificação de origem no WebSocket, CSP restritiva e outros cabeçalhos de segurança
- Limite de taxa, de tamanho de mensagem e de conexões
- Nenhum IP é gravado e o servidor não registra conteúdo de mensagens

Os detalhes e as pendências para colocar em produção estão em [docs/LGPD.md](docs/LGPD.md).

## Testes

```
cargo test
```

## Estrutura

```
src/main.rs        servidor HTTP, rotas, cabeçalhos de segurança, tarefas de salvamento e de retenção
src/ws.rs          conexões WebSocket: identidade, consentimento, limites e comandos
src/state.rs       estado, persistência e operações da LGPD (exportar, apagar, reter)
src/model.rs       tipos e protocolo cliente/servidor
static/            cliente web e política de privacidade (embutidos no executável)
docs/LGPD.md       registro do tratamento, direitos, medidas e pendências
```
