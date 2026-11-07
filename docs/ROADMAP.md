# Roadmap do Concord

Toda funcionalidade nova passa pela checagem de LGPD antes de ser feita: dados mínimos, base legal,
exportação, eliminação, retenção, política (subir `POLICY_VERSION` se mudar o tratamento) e
[docs/LGPD.md](LGPD.md).

## Feito

- Chat em tempo real, servidores e canais, sem cadastro
- LGPD: consentimento versionado, exportação, correção, eliminação, retenção
- Moderação: dono, moderadores, servidores privados com convite, denúncias anônimas, banimentos
- Lista de membros por servidor (online/offline)

- Banco de dados SQLite (modo WAL, transações, UUIDv7, paginação por chave, migrações, backup a
  quente). Todo o SQL fica em `src/db.rs`, então trocar por PostgreSQL afeta só esse arquivo

- **Canais de voz (primeira versão):** LiveKit local gerenciado pelo Concord, áudio, mutar e
  ensurdecer, apertar para falar, volume individual, câmera, transmissão de tela até 4K60 com
  estatísticas reais, status e configurações do canal, política v3. Validado com teste de mídia real
  entre dois navegadores (`tests/media.mjs`)

- **Moderação de voz:** silenciar e ensurdecer pelo servidor, mover e desconectar, aplicados no
  LiveKit e testados contra um cliente que tenta burlar

- **Chat completo:** reações, edição, respostas, menções com aviso, imagens (sem metadados, links
  assinados), seletor de emoji, visualizador. Política v4

- **Sons e Soundpad:** efeitos sonoros da interface (mensagem, menção, entrar/sair da chamada,
  alguém entrou/saiu, mutar, transmitir, queda e volta da conexão), configuráveis. Soundpad por
  servidor com sons padrão sintetizados, favoritos, recentes, busca e volume próprio. Política v5

- **Celular:** gavetas com deslize, toque longo nas mensagens, folhas de baixo para emoji, menus e
  Soundpad, barra de controles da chamada, áreas seguras do iPhone

### Pendente na voz
- Medir a fluidez de 60 fps numa máquina com GPU (`HEADED=1 node tests/media.mjs`)
- App de desktop (Tauri e SDK nativo do LiveKit) para igualar o Discord de desktop em 1440p e 4K

## Planejamento original da voz

Pedido: funcionar o quanto antes, com os recursos do Discord e transmissão de tela com a mesma
qualidade e fluidez, em alta resolução e fps.

### Decisão técnica principal: SFU

- Áudio e vídeo vão por **WebRTC**, que já vem embutido nos navegadores, criptografado (DTLS-SRTP)
  e com baixa latência.
- Se cada pessoa mandasse o vídeo direto para todas as outras (malha), uma transmissão 1080p60 para
  10 pessoas exigiria cerca de 10 vezes o upload de quem transmite. Por isso o Discord usa um
  **SFU** (*Selective Forwarding Unit*): quem transmite envia **uma** vez ao servidor, e o servidor
  repassa para cada espectador.
- **Simulcast/SVC:** quem transmite manda camadas em qualidades diferentes (por exemplo 1080p60,
  720p30 e 360p). O SFU entrega a camada que cabe na conexão de cada espectador, e quem tem internet
  ruim não derruba a qualidade dos outros.

Opções de SFU (a escolher no início da etapa):

| Opção | Prós | Contras |
|---|---|---|
| **LiveKit** (serviço separado, código aberto) | Mais rápido de chegar à qualidade do Discord: simulcast, SVC, AV1/VP9/H.264, TURN embutido, maduro | Escrito em Go; é um serviço a mais para operar |
| **mediasoup** (biblioteca com API oficial em Rust) | Fica dentro do servidor Rust | Compila um núcleo em C++; mais difícil de montar no Windows |
| **SFU próprio com webrtc-rs** | 100% Rust | Muitos meses até igualar controle de congestionamento e qualidade |

Recomendação inicial: **LiveKit**, pelo pedido de "o quanto antes". O Concord continua decidindo
quem pode entrar em cada canal, emitindo um token de acesso curto e assinado por sessão.

### Transmissão de tela em qualidade de Discord
- Captura pelo `getDisplayMedia`: tela inteira, janela ou aba, com o **áudio** da aba ou do sistema
  quando o navegador permitir.
- Presets de **720p30, 1080p60, 1440p60 e 4K30/60**, como no Discord, com taxa de bits de até
  10 Mbps ou mais.
- `contentHint` "motion" para jogos e vídeos (prioriza fluidez) ou "detail" para texto e código
  (prioriza nitidez).
- Codecs com aceleração de hardware (H.264 e AV1, conforme a GPU) e simulcast para espectadores com
  conexões diferentes.
- Limite real: a qualidade máxima depende da GPU e do upload de quem transmite, como no Discord. A
  tela mostra a resolução e o fps efetivos e um aviso quando o envio estiver limitado.

### Recursos do canal de voz (paridade com o Discord)
- **Status do canal de voz:** texto curto definido por quem está no canal (por exemplo "jogando X").
- **Configurações do canal:** limite de pessoas, taxa de bits do áudio, qualidade de vídeo
  (automática ou 720p) e permissões de quem pode falar e transmitir.
- **Configurações pessoais:**
  - microfone e saída de áudio;
  - supressão de ruído e cancelamento de eco;
  - ativação por voz (com sensibilidade) ou *push-to-talk*;
  - volume individual de cada pessoa.
- **Indicadores:** quem está falando, mutado ou ensurdecido, e quem está transmitindo ("AO VIVO").
- **Moderação:** mutar ou ensurdecer pelo servidor, mover entre canais e desconectar.
- Câmera e vários espectadores por transmissão.

### LGPD da voz
- **Nada é gravado:** áudio e vídeo só passam pelo SFU, criptografados. Gravação, se um dia existir,
  exigirá aviso visível e consentimento de todos no canal.
- **IP:** o SFU e o servidor TURN processam o IP de quem participa. Isso entra na política, e os
  logs deles serão configurados para não guardar IP além do necessário.
- **Terceiros na tela:** antes de transmitir, aparece um aviso de que a tela pode mostrar dados
  pessoais de outras pessoas.
- **Status do canal:** é dado guardado, então entra na exportação, na eliminação e na retenção.
- **Política:** sobe de versão, porque microfone e câmera são um tratamento novo, o que pede novo
  aceite.

## Depois da voz

A ordem combinada é voz primeiro e, em seguida, a importação do Discord e os bots, descritos abaixo.

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
| Produção (HTTPS, Docker, backups) | Médio | Ver pendências em [LGPD.md](LGPD.md) |
