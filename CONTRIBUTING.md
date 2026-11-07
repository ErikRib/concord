# Contribuindo com o Concord

1. Leia [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) para entender o código e
   [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) para preparar o ambiente.
2. Abra uma issue descrevendo a mudança antes de começar algo grande.
3. Crie um branch a partir do `main`.
4. Siga o [checklist de funcionalidade nova](docs/DEVELOPMENT.md#checklist-de-funcionalidade-nova),
   principalmente a parte de LGPD: todo dado novo precisa entrar na exportação, na eliminação e no
   inventário de `docs/LGPD.md`.
5. Antes do pull request:
   ```
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   node tests/e2e.mjs 3999   # com o servidor de teste rodando
   ```
6. No pull request, explique o que mudou e por quê, e anexe capturas de tela se mexer na interface
   (desktop e celular).

Interface, mensagens e comentários são em português. A interface segue o design system de
[docs/DESIGN.md](docs/DESIGN.md).

Vulnerabilidades: veja [SECURITY.md](SECURITY.md).
