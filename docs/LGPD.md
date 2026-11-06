# Concord e a LGPD

Registro das operações de tratamento (LGPD art. 37) e das medidas adotadas desde o início do projeto
(privacy by design, art. 46, §2º). Atualize este documento sempre que o tratamento mudar.

> Este documento descreve medidas técnicas e não substitui orientação jurídica.

## 1. Inventário de dados

| Dado | Origem | Finalidade | Base legal | Onde fica | Retenção |
|---|---|---|---|---|---|
| Apelido e cor | Titular | Identificar quem fala no chat | Consentimento (art. 7º, I) | Servidor (`concord-data.json`) e navegador | Enquanto houver mensagens ou acesso recente |
| Id pseudônimo (SHA-256 do segredo) | Calculado no servidor | Vincular mensagens ao titular sem cadastro | Consentimento | Servidor | Igual ao acima |
| Segredo de 256 bits | Gerado no navegador | Provar a autoria sem senha | Consentimento | **Apenas no navegador** | Até o titular limpar o navegador ou apagar os dados |
| Mensagens e data/hora | Titular | Prestar o serviço de chat | Consentimento | Servidor | `CONCORD_RETENCAO_DIAS` (padrão 180) |
| Versão e data do aceite | Sistema | Comprovar o consentimento (art. 8º, §2º) | Cumprimento de obrigação legal (art. 7º, II) | Servidor | Enquanto o titular tiver dados |
| Último acesso | Sistema | Calcular a retenção de inativos | Consentimento | Servidor | Igual ao acima |
| Endereço IP | Conexão | Manter a conexão aberta | Necessário ao serviço | **Só em memória**, nunca gravado | Duração da conexão |

**O que não é coletado:** e-mail, telefone, nome real, senha, localização, cookies, rastreadores
e dados de navegação.

## 2. Direitos do titular (art. 18) e onde estão implementados

| Direito | Como o titular exerce | Implementação |
|---|---|---|
| Confirmação e acesso (I, II) | Menu 🛡 → "Baixar meus dados" | `ClientMsg::ExportMyData` → `AppState::export_user_data` |
| Correção (III) | ✎ ou menu de privacidade → "Corrigir meu apelido" | `ClientMsg::Hello` durante a conexão → `update_profile`, que também atualiza o nome nas mensagens antigas (qualidade dos dados, art. 6º, V) |
| Portabilidade (V) | O mesmo arquivo JSON do acesso | `export_user_data` |
| Eliminação (VI) e revogação (IX) | Menu 🛡 → "Apagar meus dados e sair" | `ClientMsg::DeleteMyData` → `delete_user_data` (gravado em disco na hora) |
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
| Corrupção do arquivo de dados | Gravação atômica (arquivo temporário seguido de renomeação) |
| Apagar mensagem de outra pessoa | O servidor confere a autoria pelo id derivado |
| Dados guardados além do necessário | Limpeza automática a cada hora (arts. 15 e 16) |

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
4. **Criptografia em repouso.** Proteja o disco ou a pasta de `concord-data.json` (BitLocker, LUKS)
   e restrinja o acesso ao arquivo.
5. **Backups.** Se houver backup, ele precisa respeitar o mesmo prazo de retenção, e a eliminação
   deve chegar até ele.
6. **Plano de resposta a incidentes (art. 48).** Defina quem avalia o incidente e como comunicar a
   ANPD e os titulares em prazo razoável.
7. **Moderação.** Hoje qualquer pessoa cria servidores e canais. Para uso público, defina regras de
   uso e um canal de denúncia de conteúdo.
8. **Relatório de impacto (RIPD, art. 38).** Recomendado se o uso crescer ou se houver dados sensíveis.
