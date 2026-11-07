# Segurança

## Como relatar uma vulnerabilidade

**Não abra uma issue pública.** Use o relato privado do GitHub: na aba **Security** do repositório,
clique em **Report a vulnerability**. Descreva o problema, como reproduzir e o impacto que você
imagina.

Vulnerabilidades que exponham dados pessoais (mensagens, arquivos, ids, servidores privados) têm
prioridade, porque o Concord trata dados protegidos pela LGPD. Se o problema afetar uma instância em
produção, o operador avalia a comunicação à ANPD e aos titulares (LGPD, art. 48).

## O que está no escopo

- Ler ou alterar dados de outra pessoa ou de um servidor privado
- Passar por cima de permissões (moderação, papéis, banimentos)
- Executar script na origem do Concord (XSS), inclusive por arquivos enviados
- Usar o servidor para alcançar a rede interna (SSRF) pela busca de GIF colado
- Entrar em chamadas sem permissão ou burlar a moderação de voz
- Metadados pessoais (localização) que passem pela limpeza de imagens, vídeos ou sons

## Medidas existentes

Resumo em [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#segurança-em-camadas). A lista completa de
riscos e medidas está em [docs/LGPD.md](docs/LGPD.md).

## Fora do escopo

- Vírus em arquivos enviados por usuários (o Concord não analisa conteúdo e avisa em programas)
- Ataques de negação de serviço por volume
- Configuração da máquina de quem opera a instância (veja as pendências em `docs/LGPD.md`)
