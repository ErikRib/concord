# Roadmap do Concord

Toda funcionalidade nova passa pela checagem de LGPD antes de ser feita: dados mínimos, base legal,
exportação, eliminação, retenção, política (subir `POLICY_VERSION` se mudar o tratamento) e
[docs/LGPD.md](LGPD.md).

## Feito

- Chat em tempo real, servidores e canais, sem cadastro
- LGPD: consentimento versionado, exportação, correção, eliminação, retenção
- Moderação: dono, moderadores, servidores privados com convite, denúncias anônimas, banimentos
- Lista de membros por servidor (online/offline)

## Próximo: banco de dados SQLite

Hoje tudo fica num único arquivo JSON, regravado inteiro a cada mudança. Trocar por SQLite é
**pré-requisito** para os dois itens pedidos abaixo:

- bots precisam de tokens guardados com segurança e de consultas eficientes;
- a importação do Discord pode trazer milhares de canais e mensagens de uma vez.

Benefícios extras: histórico paginado (rolar para mensagens antigas), eliminação e retenção com
uma consulta, e menos risco de perder dados.

## Pedido: importar servidores do Discord

Como o Discord não oferece um botão de "exportar servidor", há três caminhos, do mais simples ao
mais completo:

| Caminho | O que traz | Como |
|---|---|---|
| **1. Modelo de servidor** | Nome, canais e categorias (sem mensagens) | O dono cria um modelo no Discord (Configurações do servidor → Modelo) e cola o link `discord.new/CÓDIGO`. O Concord lê a API pública de modelos |
| **2. Arquivo do DiscordChatExporter** | Canais e mensagens | O dono exporta em JSON com a ferramenta de código aberto DiscordChatExporter e envia o arquivo |
| **3. Bot de importação** | Canais e mensagens | O dono adiciona ao servidor do Discord um bot dele próprio, e o Concord usa o token desse bot para ler o histórico |

**Termos do Discord:** usar o token de uma conta de pessoa ("selfbot") é proibido. Só os caminhos
acima são aceitáveis.

**LGPD (o ponto mais delicado):** as mensagens importadas são dados pessoais de **outras pessoas**,
que nunca aceitaram a política do Concord.

- O padrão será importar **só a estrutura** (caminho 1).
- Ao importar mensagens, os autores ficarão **pseudonimizados** ("Membro 1", "Membro 2"…), sem nomes
  nem ids do Discord.
- Será possível **reivindicar a autoria**: quem entrar no Concord pode vincular as próprias mensagens
  importadas, por um processo de verificação a definir.
- Quem importa declara ter autorização para isso e assume o papel de controlador desses dados.
- Mensagens importadas seguem a retenção normal.
- O que importar entra no registro de moderação e na política.

## Pedido: bots para servidores

- **Conta de bot** criada por uma pessoa e adicionada a um servidor pelo dono, com permissões
  escolhidas (ler mensagens, enviar mensagens, moderar).
- **Token secreto** mostrado uma única vez. O servidor guarda só o hash, e é possível revogar e
  gerar outro.
- **API:** HTTP para ações (enviar mensagem, apagar etc.) e WebSocket para receber eventos, com os
  mesmos filtros de permissão de uma pessoa.
- **Selo "BOT"** nas mensagens e na lista de membros.
- **Limite de taxa** próprio e registro de moderação das ações do bot.

**LGPD:**
- Quem cria o bot é responsável pelo que ele faz com os dados (art. 37 a 39). Isso entra nos termos
  de criação.
- Bots só recebem eventos dos servidores em que foram adicionados.
- Ler o conteúdo das mensagens é uma permissão separada, que o dono do servidor precisa conceder
  (minimização).
- Os membros veem quais bots estão no servidor e o que cada um pode fazer (transparência).
- A exportação de quem criou o bot inclui seus bots. Quando essa pessoa apaga os dados, os bots
  dela são removidos.

## Depois

| Funcionalidade | Esforço | Cuidado de LGPD |
|---|---|---|
| Reações com emoji | Baixo | Entram na exportação e na eliminação |
| Editar mensagem | Baixo | Guardar só a versão atual |
| Responder e mencionar (@apelido) | Médio | — |
| Imagens e arquivos | Médio | Remover metadados (EXIF/GPS), limitar tipo e tamanho, incluir na eliminação |
| Mensagens diretas | Médio | Avaliar criptografia ponta a ponta |
| Canais de voz (WebRTC) | Alto | Não gravar áudio; política nova |
| Produção (HTTPS, Docker, backups) | Médio | Ver pendências em [LGPD.md](LGPD.md) |
