# Design system do Concord

Guia visual e de interação de **toda** tela nova do Concord. Os tokens moram em
`static/style.css` (`:root`); nenhuma cor, raio ou sombra deve ser escrita "solta" no CSS.
Componentes novos reutilizam as classes abaixo antes de criar outras.

Inspiração: aplicativos de comunidade com tema escuro em camadas. Usamos **padrões** de layout e
interação, nunca marcas, logotipos, ilustrações ou fontes proprietárias de terceiros.

## 1. Princípios

1. **Escuro em camadas.** Quanto mais "à frente", mais claro: base → barra lateral → chat →
   elevado (cartões, menus, painel do usuário).
2. **Conteúdo primeiro.** Ações secundárias aparecem no *hover* (ferramentas do canal, ações da
   mensagem) ou em menus de contexto, sem poluir a tela.
3. **Perigo é vermelho e explícito.** Ações destrutivas ficam no fim dos menus, separadas por uma
   divisória, em `--danger`. Ações irreversíveis pedem confirmação digitada.
4. **Estado visível.** Alternâncias usam caixinha à direita do item (marcada/desmarcada). Status
   de voz e moderação têm ícone próprio, e o que foi imposto pela moderação fica com fundo vermelho.
5. **Privacidade à vista (LGPD).** Telas que tratam dados pessoais explicam o tratamento no próprio
   lugar (texto `--text-muted` abaixo do título) e linkam a política.
6. **Acessível.** Contraste AA no texto principal, foco visível (anel `--accent`), `aria-label` em
   botões só com ícone, nada depende só de cor.

## 2. Tokens

### Cores (camadas)
| Token | Uso |
|---|---|
| `--bg-base` | Fundo da janela, barra de servidores, tela de configurações |
| `--bg-sidebar` | Lista de canais e lista de membros |
| `--bg-chat` | Área principal (mensagens, palco de voz) |
| `--bg-elevated` | Cartões, painel do usuário, compositor de mensagem |
| `--bg-floating` | Menus, popovers, seletores (sempre com `--border` e `--shadow-pop`) |
| `--bg-input` | Campos de texto |
| `--bg-hover` / `--bg-selected` | Linha sob o cursor / item selecionado |
| `--border` / `--border-strong` | Contornos sutis / contornos de campos e divisórias fortes |

### Texto e cores de função
| Token | Uso |
|---|---|
| `--text-strong` | Títulos, nome selecionado |
| `--text` | Corpo |
| `--text-muted` | Rótulos, descrições, horários |
| `--text-faint` | Placeholders, itens desativados |
| `--accent` (+ `-hover`, `-soft`) | Ação principal, foco, seleção, caixinha marcada |
| `--danger` (+ `-soft`) | Ações destrutivas, erro, moderação imposta |
| `--success` | Online, "voz conectada", ao falar |
| `--warning` | Selo de dono, avisos |
| `--link` | Links no conteúdo |

### Forma, espaço e tipo
| Token | Valor |
|---|---|
| `--radius-xs / -sm / (padrão) / -lg / -xl / -pill` | 4 / 6 / 8 / 12 / 16 / 999 px |
| `--space-1 … --space-6` | 4 / 8 / 12 / 16 / 20 / 24 px |
| `--fs-xs / -sm / -md / -lg / -xl` | 12 / 14 / 16 / 20 / 24 px |
| `--shadow-pop` | Menus e popovers |
| `--shadow-card` | Cartões elevados |
| `--t-fast` | Transições de hover (120 ms) |

Fonte: pilha do sistema (`--font`). Rótulos de seção: 12 px, 700, MAIÚSCULAS, `--text-muted`.

## 3. Componentes

### Botões
- `.btn.primary`: ação principal (`--accent`). Uma por tela/diálogo.
- `.btn.secondary`: ação neutra (`--bg-selected`).
- `.btn.danger`: destrutiva sólida; `.btn.danger-outline`: destrutiva menos enfática (ex.: "Remover").
- `.btn.ghost`: links de ação ("Cancelar").
- `.btn.success` / `.primary-btn`: entrar em chamada.
- `.icon-btn`: só ícone, 32 px, sempre com `title` e `aria-label`.

### Campos
`.field` (input/select/textarea): `--bg-input`, borda `--border-strong`, raio padrão, foco com
anel de 2 px `--accent`. Rótulo acima com `.field-label`; ajuda abaixo em `--text-muted`.

### Menu de contexto (`.menu`)
Popover `--bg-floating`, raio padrão, borda, sombra, largura mínima 220 px. Item (`.menu-item`):
ícone opcional de 20 px à esquerda, texto, e à direita um **atalho/chevron ou caixinha**
(`.menu-check`, marcada = `--accent` com ✓). Grupos separados por `.menu-sep`. Itens perigosos
(`.danger`) no fim, em vermelho, com hover `--danger-soft`. Fecha com clique fora ou Esc.
Usado em: opções do servidor, pessoa (nome/avatar), "+" de criar/entrar.

### Lista de canais
- Categoria (`.category`): rótulo de seção recolhível com chevron; "+" à direita para quem modera.
- Canal (`.ch-row`): ícone (# texto, alto-falante voz), nome, ferramentas no hover
  (engrenagem, ×). Selecionado: `--bg-selected` e `--text-strong`. Não lido: texto forte.
- Canal de voz: pílula `.ch-count` "pessoas | limite"; pessoas conectadas indentadas abaixo
  com avatar 24 px, selos e ícones de estado; status do canal em `--text-muted`.

### Painel do usuário (`#user-panel`)
Cartão flutuante `--bg-elevated` no rodapé da barra lateral, raio padrão, margem 8 px: avatar
com ponto de status, nome + subtítulo, e ícones microfone / áudio / configurações.

### Mensagens
- Agrupamento por autor em 5 min; horário aparece no hover.
- `.date-divider`: linha com a data no centro ("6 de novembro de 2026").
- Ações da mensagem: barra flutuante no hover, canto superior direito.
- Futuro: reações em pílula (`--bg-elevated`, borda; a minha com `--accent-soft` e borda accent).

### Lista de membros
Seções por papel (`DONO — 1`, `MODERAÇÃO — n`, `ONLINE — n`, `OFFLINE — n`), avatar 32 px com
ponto de status, nome na cor da pessoa, selo de papel em pílula.

### Selos (`.badge`)
Pílula de 10-11 px, 700, maiúsculas: `owner` (amarelo), `mod` (accent), `live` (vermelho),
`new` (accent, para novidades).

### Diálogo (`.overlay` + `.card`)
Centralizado, `--bg-elevated`, raio `-lg`, título 20 px, descrição muted, campos, ações
alinhadas à direita (ghost "Cancelar" + primary).

### Tela de configurações (`.settings`)
Tela cheia sobre o app, fundo `--bg-base`:
- **Navegação** à esquerda: grupos com rótulo de seção, itens 16 px; selecionado
  `--bg-selected`; itens perigosos em vermelho no fim.
- **Conteúdo** no centro (até 700 px): título 20 px, descrição, blocos separados por divisória.
- **Prévia** à direita quando faz sentido (cartão do servidor/perfil).
- **Fechar**: botão circular com "ESC" embaixo; a tecla Esc também fecha.
Usada em: configurações do servidor (perfil, convites, denúncias, banidos) e do usuário
(minha conta, voz e vídeo, privacidade e dados).

### Toast
Rodapé central, raio padrão; erro = `--danger`, sucesso = `--success`.

## 4. Como aplicar em funcionalidades novas

1. Use tokens e classes existentes; crie componente novo só se não houver equivalente, e
   documente-o aqui.
2. Opções de um objeto (mensagem, pessoa, canal) → menu de contexto, não botões soltos.
3. Configuração com várias seções → tela `.settings`; pergunta rápida → diálogo.
4. Toda tela que coleta ou mostra dado pessoal explica o tratamento e respeita a LGPD
   (ver `docs/LGPD.md`).
5. Teste em 1280 px, 1000 px (sem lista de membros) e celular (640 px).
