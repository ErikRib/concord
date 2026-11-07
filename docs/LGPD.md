# Concord e a LGPD

Registro das operações de tratamento (LGPD art. 37) e das medidas adotadas desde o início do projeto
(privacy by design, art. 46, §2º). Atualize este documento sempre que o tratamento mudar.

> Este documento descreve medidas técnicas e não substitui orientação jurídica.

## 1. Inventário de dados

| Dado | Origem | Finalidade | Base legal | Onde fica | Retenção |
|---|---|---|---|---|---|
| Apelido e cor | Titular | Identificar quem fala no chat | Consentimento (art. 7º, I) | Banco (`concord.db`) e navegador | Enquanto houver mensagens ou acesso recente |
| Id pseudônimo (SHA-256 do segredo) | Calculado no servidor | Vincular mensagens ao titular sem cadastro | Consentimento | Servidor | Igual ao acima |
| Segredo de 256 bits | Gerado no navegador | Provar a autoria sem senha | Consentimento | **Apenas no navegador** | Até o titular limpar o navegador ou apagar os dados |
| Mensagens e data/hora | Titular | Prestar o serviço de chat | Consentimento | Servidor | `CONCORD_RETENCAO_DIAS` (padrão 180) |
| Versão e data do aceite | Sistema | Comprovar o consentimento (art. 8º, §2º) | Cumprimento de obrigação legal (art. 7º, II) | Servidor | Enquanto o titular tiver dados |
| Último acesso | Sistema | Calcular a retenção de inativos | Consentimento | Servidor | Igual ao acima |
| Endereço IP | Conexão | Manter a conexão aberta | Necessário ao serviço | **Só em memória**, nunca gravado | Duração da conexão |
| Participação e papel em servidores | Titular (convite) / dono | Controlar acesso a servidores privados e poderes de moderação | Consentimento | Servidor | Enquanto o servidor existir ou até o titular sair ou apagar os dados |
| Denúncias (categoria, detalhes, quem denunciou) | Titular | Moderação de conteúdo | Legítimo interesse (art. 7º, IX) | Servidor; quem denunciou **nunca é mostrado** aos moderadores | Até ser resolvida ou por `CONCORD_RETENCAO_DIAS` |
| Banimentos (servidor, motivo, data) | Moderador | Afastar quem descumpre regras | Legítimo interesse | Servidor; visível aos moderadores e ao próprio banido | `CONCORD_RETENCAO_DIAS` (o banimento expira) |
| Registro de moderação (ação, autor, alvo, data; **sem conteúdo**) | Sistema | Responsabilização e revisão de decisões (art. 6º, X) | Legítimo interesse | Servidor | `CONCORD_RETENCAO_DIAS` |
| Áudio, câmera e tela | Titular, ao ligar cada um | Chamadas ao vivo | Consentimento | **Não é armazenado**: trafega criptografado (DTLS-SRTP) pelo LiveKit | Só durante a transmissão |
| Presença em canal de voz (canal, mudo, câmera, ao vivo) | Sistema | Mostrar quem está na chamada | Consentimento | **Só em memória** | Até sair da chamada |
| Status do canal de voz e quem o definiu | Titular | Informar o que acontece no canal | Consentimento | Banco; quem definiu **não é mostrado** | Apagado quando o canal esvazia |
| IP na chamada | Conexão de mídia | Entregar áudio e vídeo | Necessário ao serviço | LiveKit, em memória, com log em nível "warn", sem IP de participante | Duração da chamada |
| Preferências de voz (dispositivos, volumes, tecla) | Titular | Conforto | — | **Apenas no navegador** | Até limpar o navegador ou apagar os dados |
| Edição de mensagem (só a data; sem histórico) | Titular | Mostrar "(editado)" | Consentimento | Servidor | Junto com a mensagem |
| Respostas e menções (id da mensagem respondida; `<@id>` no texto) | Titular | Contexto da conversa | Consentimento | Servidor, dentro da mensagem | Junto com a mensagem |
| Reações (mensagem, emoji, quem, quando) | Titular | Interação | Consentimento | Servidor; visível a quem vê o servidor | Junto com a mensagem; apagadas com os dados do titular |
| Imagens (arquivo **sem metadados**, nome, tipo, tamanho, dimensões) | Titular | Compartilhar imagens no chat | Consentimento | Banco, entregue só por link assinado que expira (~24 h) | Junto com a mensagem; não usadas somem em 1 h |
| Vídeos MP4/MOV (sem localização, modelo e datas de gravação) e outros arquivos (como enviados, até 25 MB) | Titular | Compartilhar arquivos no chat | Consentimento | Banco; link assinado; não-mídia só como download | Igual às imagens |
| Sons do Soundpad (WAV ≤ 5 s **sem metadados**, nome, emoji, quem enviou) | Moderação do servidor | Soundpad nos canais de voz | Consentimento | Banco; link assinado; **quem enviou não é mostrado** | Enquanto fizer parte do servidor; apagados com o servidor ou com os dados de quem enviou |
| Som tocado (quem, qual, em qual canal) | Titular | Avisar a chamada | Consentimento | **Não é gravado**: só repassado ao servidor na hora | — |
| Preferências de sons, favoritos e recentes do Soundpad, emojis recentes | Titular | Conforto | — | **Apenas no navegador** | Até limpar o navegador ou apagar os dados |
| Silenciado/ensurdecido pela moderação | Moderador | Moderação de voz | Legítimo interesse | **Só em memória** (a ação fica no registro de moderação) | Até um moderador desfazer, o Concord reiniciar ou o titular apagar os dados |

### Teste de legítimo interesse (art. 10)

- **Finalidade legítima:** manter servidores livres de spam, assédio e exposição de dados de terceiros.
- **Necessidade:** só se guarda o mínimo. O registro de moderação não tem conteúdo de mensagens, e
  denúncias resolvidas são apagadas.
- **Expectativa do titular e salvaguardas:** a moderação é informada no aceite e na política. Quem
  denuncia fica anônimo para os moderadores, quem é banido recebe o motivo, tudo aparece na
  exportação e tudo expira com a retenção.

**O que não é coletado:** e-mail, telefone, nome real, senha, localização, cookies, rastreadores
e dados de navegação.

## 2. Direitos do titular (art. 18) e onde estão implementados

| Direito | Como o titular exerce | Implementação |
|---|---|---|
| Confirmação e acesso (I, II) | Menu de privacidade → "Baixar meus dados" (inclui servidores, denúncias feitas, banimentos recebidos e ações de moderação ligadas ao titular) | `ClientMsg::ExportMyData` → `AppState::export_user_data` |
| Correção (III) | ✎ ou menu de privacidade → "Corrigir meu apelido" | `ClientMsg::Hello` durante a conexão → `update_profile`, que também atualiza o nome nas mensagens antigas (qualidade dos dados, art. 6º, V) |
| Portabilidade (V) | O mesmo arquivo JSON do acesso | `export_user_data` |
| Eliminação (VI) e revogação (IX) | Menu de privacidade → "Apagar meus dados e sair" | `ClientMsg::DeleteMyData` → `delete_user_data`: apaga mensagens, denúncias, papéis e banimentos, anonimiza o registro de moderação e passa os servidores do titular para um moderador ou membro. É gravado em disco na hora |
| Oposição (§2º) ao legítimo interesse | Contato do encarregado | Atendimento manual pelo controlador |
| Informação (VII, VIII) | Página `/privacidade` e aviso na entrada | `static/privacidade.html` |

## 3. Consentimento (arts. 8º e 9º)

- A tela de entrada mostra um resumo do tratamento e um link para a política completa.
- As caixas de aceite e de maioridade **começam desmarcadas**, e o botão "Entrar" só é liberado com ambas marcadas.
- **Nada é gravado no servidor antes do aceite.** Sem a versão atual da política, o servidor responde
  `consent_required` e fecha a conexão.
- Ao mudar a política, aumente `POLICY_VERSION` e `POLICY_UPDATED_AT` em `src/state.rs`. Todos
  precisarão aceitar de novo.

## 4. Medidas de segurança (art. 46)

| Risco | Medida |
|---|---|
| Alguém se passar por outra pessoa | Id público derivado do SHA-256 de um segredo aleatório de 256 bits que só existe no navegador do titular |
| Outro site abrir conexão em nome do visitante (CSWSH) | Verificação do cabeçalho `Origin` contra o `Host` |
| Injeção de código (XSS) | Mensagens renderizadas só com `textContent`, links limitados a http/https, CSP `default-src 'self'` sem scripts inline |
| Clickjacking | `X-Frame-Options: DENY` e `frame-ancestors 'none'` |
| Vazamento de URL para terceiros | `Referrer-Policy: no-referrer` e links com `noopener noreferrer` |
| Spam e esgotamento de recursos | Limite de taxa por conexão (≈3 ações/s), mensagens de até 16 KiB, até 500 conexões, 200 servidores e 100 canais por servidor, tempo limite para identificação |
| Corrupção ou perda de dados | SQLite em modo WAL; cada alteração é uma transação ACID, com chaves estrangeiras garantindo a integridade (apagar um canal leva junto mensagens e denúncias). Se o banco falhar, o cache em memória é recarregado dele |
| Apagar mensagem de outra pessoa | O servidor confere a autoria pelo id derivado; só moderadores apagam mensagens alheias, e isso fica registrado |
| Escalada de privilégio | Toda permissão é verificada no servidor, em `src/commands.rs`. O cliente só esconde botões. Testado com tentativas de membros criarem canais, se promoverem e apagarem mensagens alheias |
| Vazamento de servidor privado | Cada conexão filtra os eventos pelo que a pessoa pode ver (`Audience` e `ServerView`). Membros, banidos e convite só chegam a quem modera. Admins da instância não veem servidores privados |
| Adivinhar convites | Código aleatório de 12 caracteres (~70 bits), limite de taxa e troca do código a qualquer momento |
| Retaliação contra quem denuncia | O id de quem denunciou nunca sai do servidor |
| Entrar em chamada sem permissão | Só o Concord emite tokens do LiveKit, válidos por 10 minutos, presos a uma sala e a uma identidade, e só para quem vê o servidor, não está banido e cabe no limite. Testado: o LiveKit recusa token adulterado |
| Segredo do LiveKit vazar | No uso local, é gerado a cada execução e passado ao LiveKit por variável de ambiente, nunca pela linha de comando nem em arquivo |
| Mídia interceptada | WebRTC com DTLS-SRTP obrigatório. Em produção, sinalização por `wss://` |
| Microfone, câmera ou tela usados por terceiros | `Permissions-Policy` libera microfone, câmera e captura de tela só para o próprio Concord. A CSP só libera conexão com o LiveKit configurado |
| Quem foi banido continuar na chamada | Banimento, saída de servidor privado e exclusão de canal ou servidor encerram a sessão de voz na hora, **também no LiveKit** (RemoveParticipant), sem depender do navegador |
| Burlar o silêncio imposto pela moderação | "Silenciar no servidor" retira a permissão de microfone no LiveKit e muta a trilha no ar; reentrar recebe um token sem microfone. Testado com navegador real: chamar o SDK direto falha com "insufficient permissions" |
| Chamada que segue depois de fechar a aba | Se o chat não reconectar em 15 s, o LiveKit remove a pessoa da sala |
| Exposição de terceiros na transmissão de tela | Aviso antes de transmitir; as transmissões só são baixadas por quem clica em "Assistir" |
| LiveKit órfão após uma queda | Job Object do Windows encerra o LiveKit junto com o Concord |
| Metadados escondidos em fotos (GPS, aparelho, data) | Removidos no navegador (nova codificação por canvas) e de novo no servidor (EXIF/XMP/IPTC/comentários em PNG, JPEG, GIF e WebP). Testado com um PNG com localização |
| Metadados em áudio (autor, software, ID3/LIST) | O navegador converte o som para WAV PCM; o servidor só aceita WAV PCM 16 bits e **reconstrói** o arquivo só com os blocos de formato e áudio |
| Arquivo malicioso aberto com a origem do Concord (HTML/SVG com script) | O tipo vem do conteúdo, não do navegador; tudo que não é imagem, vídeo ou áudio sai como `application/octet-stream` + `attachment`, com CSP `sandbox` própria. Nomes sem caracteres de inversão de texto (truque "foto‮gpj.exe"); aviso em programas e scripts |
| Localização em fotos HEIC/AVIF que o navegador não converte | Recusadas no servidor com orientação para enviar JPEG |
| Envio de arquivo por terceiros ou sem permissão | Autorização de uso único (2 min) pedida pelo WebSocket; o segredo de identidade nunca vai na URL. Autorização de imagem não serve para som e vice-versa |
| Links de imagens e sons copiados e espalhados | Links assinados (HMAC-SHA256, chave nova a cada execução), com validade de ~24 h e domínios separados (imagem ≠ som); `Cache-Control: private` |
| Spam de sons na chamada | Só quem está na voz toca, no próprio canal; intervalo mínimo de 1,5 s; silenciados pela moderação não tocam; até 48 sons por servidor |
| Dados guardados além do necessário | Limpeza automática a cada hora (arts. 15 e 16), em lotes que não travam o chat. A retenção usa a ordem cronológica dos ids UUIDv7 |

## 5. Pendências para colocar em produção

Estes itens dependem de quem vai operar a instância e não podem ser resolvidos só com código:

1. **Identificar o controlador e o encarregado (art. 41)** nas variáveis `CONCORD_CONTROLADOR` e
   `CONCORD_CONTATO`. Enquanto não forem definidas, o servidor emite um aviso ao iniciar.
2. **HTTPS obrigatório.** Use um proxy reverso (Caddy, nginx) com TLS. Sem isso, as mensagens e o
   segredo trafegam sem criptografia.
3. **Logs do proxy.** O Concord não grava IP, mas o proxy reverso pode gravar. Configure-o de acordo
   e informe na política. Se o operador for uma pessoa jurídica com fins econômicos, o Marco Civil
   da Internet (Lei 12.965/2014, art. 15) exige guardar registros de acesso por 6 meses. Nesse caso,
   ajuste a política e a retenção.
4. **Criptografia em repouso.** Proteja o disco ou a pasta de `concord.db` (BitLocker, LUKS) e
   restrinja o acesso aos arquivos `concord.db`, `concord.db-wal` e `concord.db-shm`.
5. **Backups.** `concord backup <arquivo>` gera uma cópia consistente. Cada cópia guarda dados
   pessoais: mantenha os backups por no máximo o prazo de retenção e saiba que "Apagar meus dados"
   não alcança cópias antigas. Por isso, troque as cópias com frequência e documente isso na política.
6. **Arquivo migrado.** Depois de conferir a migração, apague o `concord-data.json.migrado`. Ele tem
   dados pessoais fora do alcance da retenção e da eliminação.
7. **Plano de resposta a incidentes (art. 48).** Defina quem avalia o incidente e como comunicar a
   ANPD e os titulares em prazo razoável.
8. **Regras de uso e administradores.** A moderação já existe, mas o servidor público "Concord" só é
   moderado se `CONCORD_ADMINS` estiver definido. Publique regras de convivência e um canal para
   contestar decisões de moderação.
9. **Relatório de impacto (RIPD, art. 38).** Recomendado se o uso crescer ou se houver dados sensíveis.
   Com voz e câmera, a voz e a imagem do rosto são dados pessoais. Não há reconhecimento facial
   nem biometria, mas um RIPD passa a ser ainda mais recomendável.
10. **LiveKit em produção.** Use HTTPS (`wss://`), TURN para quem está atrás de firewall, logs sem
    IP de participante, e coloque o LiveKit no inventário de suboperadores se ele for hospedado por
    terceiros (por exemplo, LiveKit Cloud).
