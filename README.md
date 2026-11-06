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
| `CONCORD_DB` | `concord.db` | Banco de dados SQLite (contém dados pessoais: proteja-o) |
| `CONCORD_DATA` | `concord-data.json` | Arquivo do formato antigo, importado uma única vez se o banco estiver vazio |
| `CONCORD_RETENCAO_DIAS` | `180` | Prazo após o qual mensagens e registros inativos são apagados |
| `CONCORD_CONTROLADOR` | — | Nome do controlador, exibido na política de privacidade |
| `CONCORD_CONTATO` | — | Contato do encarregado (DPO) / canal do titular |
| `CONCORD_ADMINS` | — | Ids pseudônimos dos administradores da instância, separados por vírgula |

### Como virar administrador

Não há conta, então o administrador é identificado pelo id pseudônimo:

1. Abra o Concord no navegador que você vai usar para administrar.
2. Clique no escudo → "Baixar meus dados" e copie o valor de `id_pseudonimo`.
3. Inicie o servidor com `CONCORD_ADMINS=<esse id>`.

Administradores moderam o servidor público "Concord", que não tem dono, e qualquer servidor público.
Eles **não** veem servidores privados dos quais não participam. Se os dados daquele navegador forem
apagados, o id muda e é preciso repetir o processo.

## Banco de dados

O Concord usa **SQLite**, embutido no executável, sem nada para instalar.

- **Modo WAL:** leituras rodam em paralelo com a escrita, e uma queda de energia não corrompe o banco.
- **Transações:** toda alteração é atômica. Um banimento, por exemplo, grava tudo ou nada.
- **Ids UUIDv7:** já vêm em ordem cronológica, então abrir um canal e rolar para o passado custa o
  mesmo com mil ou com milhões de mensagens.
- **Migrações versionadas:** o esquema evolui sozinho ao atualizar o Concord.
- **Migração do formato antigo:** na primeira execução, o `concord-data.json` é importado e
  renomeado para `concord-data.json.migrado`. Confira e depois **apague esse arquivo**: ele tem
  dados pessoais que a retenção e o "Apagar meus dados" não alcançam.
- **Fronteira única:** todo o SQL fica em `src/db.rs`. Trocar por PostgreSQL significa reescrever
  só esse arquivo.

Comandos úteis:

```
cargo run --release -- backup copia.db    # cópia consistente com o servidor rodando
cargo run --release -- bench 1000000      # teste de carga num banco temporário
```

## Funcionalidades

- Servidores e canais de texto, mensagens em tempo real, lista de membros online/offline, aviso de "digitando…"
- Histórico completo no banco, carregado em páginas ao rolar para cima (limitado só pelo prazo de retenção)
- Apagar as próprias mensagens e trocar o apelido
- **Servidores públicos ou privados** (padrão), com link de convite `/convite/CÓDIGO` que pode ser gerado de novo
- **Papéis:** dono, moderador, membro e visitante. Clique no nome de alguém para promover ou banir
- **Moderação:** apagar mensagens e canais; banir com motivo obrigatório (a pessoa é avisada) e revogar
- **Denúncias** com categorias. Os moderadores não sabem quem denunciou
- **LGPD:** tela de consentimento com política versionada; menu de privacidade com "Baixar meus dados"
  (JSON) e "Apagar meus dados" (eliminação imediata); retenção automática; página `/privacidade`

## Segurança

- Identidade sem senha: o navegador gera um segredo aleatório de 256 bits, e o servidor usa só o
  SHA-256 dele como id público. Saber o id de alguém não permite se passar por essa pessoa.
- Verificação de origem no WebSocket, CSP restritiva e outros cabeçalhos de segurança
- Limite de taxa, de tamanho de mensagem e de conexões
- Permissões verificadas no servidor; cada conexão só recebe eventos dos servidores que pode ver
- Nenhum IP é gravado e o servidor não registra conteúdo de mensagens

Os detalhes e as pendências para colocar em produção estão em [docs/LGPD.md](docs/LGPD.md).
O que vem a seguir, incluindo a importação do Discord e os bots, está em [docs/ROADMAP.md](docs/ROADMAP.md).

## Testes

Testes unitários:

```
cargo test
```

Teste de ponta a ponta (Node 22+), com um servidor de teste na porta 3999 e um banco **novo e
vazio**. No PowerShell:

```powershell
$env:PORT="3999"; $env:CONCORD_DB="$env:TEMP\concord-e2e.db"; Remove-Item "$env:CONCORD_DB*" -ErrorAction SilentlyContinue
$env:CONCORD_DATA="$env:TEMP\nao-existe.json"
$env:CONCORD_CONTROLADOR="Teste <Ltda>"; $env:CONCORD_CONTATO="dpo@exemplo.com"; $env:CONCORD_RETENCAO_DIAS="30"
$env:CONCORD_ADMINS = node tests/e2e.mjs --admin-id
cargo run
```

Depois, em outro terminal, rode `node tests/e2e.mjs`.

## Estrutura

```
src/main.rs        servidor HTTP, rotas, cabeçalhos de segurança, retenção, subcomandos backup/bench
src/ws.rs          conexões WebSocket: identidade, consentimento, limites e filtro de eventos
src/commands.rs    comandos dos clientes, com todas as verificações de permissão
src/state.rs       estado, cache, transações e operações da LGPD (exportar, apagar, reter)
src/db.rs          todo o SQL: esquema, migrações e consultas
src/legacy.rs      importação única do formato JSON antigo
src/bench.rs       teste de carga
src/model.rs       tipos, papéis e protocolo cliente/servidor
static/            cliente web e política de privacidade (embutidos no executável)
docs/LGPD.md       registro do tratamento, direitos, medidas e pendências
```
