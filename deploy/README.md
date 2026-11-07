# Implantação

O `deploy/setup.sh` coloca o Concord no ar numa máquina Ubuntu 24.04 e instala:

- **Concord** e **LiveKit** como serviços systemd (com isolamento), que reiniciam sozinhos;
- **Caddy**, com HTTPS automático e **sem log de acesso** (sem registro de IP). A sinalização da voz
  passa pelo mesmo domínio do site (`/rtc`), para não ser barrada por bloqueadores de anúncio;
- **TURN** num segundo domínio, para quem está atrás de firewall;
- firewall da máquina, **backup diário** com 7 dias de cópias e atualizações automáticas de segurança.

As chaves são geradas no próprio servidor e o LiveKit baixado tem o SHA-256 conferido. Rodar o
script de novo atualiza o Concord e mantém dados, chaves e configuração.

## Guias

| Guia | Custo | Observação |
|---|---|---|
| [LIGHTSAIL.md](LIGHTSAIL.md) | ~US$ 7/mês | **Recomendado e usado na instância atual.** AWS em São Paulo, IP fixo, passo a passo completo |
| [ORACLE.md](ORACLE.md) | Grátis | Alternativa. No Brasil, o cadastro pode exigir conta corporativa |

Os dois usam o [DuckDNS](https://www.duckdns.org) para os domínios gratuitos.

## Atualizar a instância

No seu computador, na pasta do Concord:

```powershell
git archive --format=tar.gz -o concord.tar.gz HEAD
scp -i SUA-CHAVE.pem concord.tar.gz ubuntu@IP-DO-SERVIDOR:~
ssh -i SUA-CHAVE.pem ubuntu@IP-DO-SERVIDOR
```

No servidor:

```bash
rm -rf concord && mkdir concord && tar -xzf concord.tar.gz -C concord && cd concord
sudo bash deploy/setup.sh SITE.duckdns.org voz-SITE.duckdns.org seu@email.com
```

Depois de atualizar, recarregue a página com Ctrl+F5. Se a política de privacidade mudou de versão,
todos verão o pedido de aceite de novo.

## Depois de colocar no ar

Defina `CONCORD_CONTROLADOR`, `CONCORD_CONTATO` e `CONCORD_ADMINS` (o guia mostra onde) e revise as
[pendências para produção](../docs/LGPD.md#5-pendências-para-colocar-em-produção): controlador,
logs do proxy, criptografia do disco, backups e plano de incidentes.

**Nunca** coloque a chave `.pem` do servidor no Git. O `.gitignore` já ignora `*.pem` e `*.key`.
