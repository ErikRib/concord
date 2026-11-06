# Colocar o Concord no AWS Lightsail (São Paulo, ~US$ 7/mês)

Resultado: o Concord no ar 24 horas, em `https://SEU-NOME.duckdns.org`, com voz e transmissão de
tela, numa máquina em **São Paulo**, com custo fixo.

O mesmo `deploy/setup.sh` instala e configura tudo: Concord, LiveKit, HTTPS sem log de IP,
firewall da máquina, backup diário e atualizações de segurança.

## 1. Alerta de gastos (faça primeiro)

1. Abra <https://console.aws.amazon.com/billing/home#/budgets>.
2. Clique em **Criar orçamento → Usar um modelo → Orçamento mensal de custos**.
3. Defina o valor em **US$ 10** e informe o seu e-mail.

Se algo começar a custar mais que o esperado, você recebe um aviso.

## 2. Criar a máquina

1. Abra <https://lightsail.aws.amazon.com/> e clique em **Criar instância**.
2. Confira a região: **São Paulo (sa-east-1)**.
3. Escolha **Linux/Unix → Somente sistema operacional (OS Only) → Ubuntu 24.04 LTS**.
4. Plano: **Dual-stack** (com IPv4), **US$ 7** (1 GB de RAM, 2 vCPUs). Se o de US$ 5 (512 MB) for
   o único com período grátis, ele também funciona, mas mais apertado.
5. Nome: `concord`. Clique em **Criar instância**.

## 3. IP fixo (grátis enquanto estiver ligado à máquina)

Na instância, abra **Rede (Networking) → Anexar IP estático (Attach static IP) → Criar**.
Anote o IP.

> Um IP estático **sem** máquina ligada a ele é cobrado. Se apagar a instância, apague o IP também.

## 4. Liberar as portas

Ainda em **Rede → Firewall IPv4**, deixe estas regras (o SSH, porta 22, já vem):

| Aplicação | Protocolo | Portas |
|---|---|---|
| HTTP | TCP | 80 |
| HTTPS | TCP | 443 |
| Personalizada | TCP | 7881 |
| Personalizada | UDP | 3478 |
| Personalizada | UDP | 50000-60000 |

Faça o mesmo em **Firewall IPv6**, se a opção existir.

## 5. Domínios grátis (DuckDNS)

Em <https://www.duckdns.org> (login com GitHub ou Google), crie **dois** subdomínios e aponte os
dois para o **IP estático**:
- `SEU-NOME` → o site (ex.: `meuconcord.duckdns.org`)
- `voz-SEU-NOME` → a voz (ex.: `voz-meuconcord.duckdns.org`)

## 6. Chave SSH

Abra <https://lightsail.aws.amazon.com/ls/webapp/account/keys>, vá em **Chaves SSH** e clique em
**Baixar** a chave padrão da região **São Paulo**. O arquivo se chama algo como
`LightsailDefaultKey-sa-east-1.pem`. Salve na pasta do Concord.

> A chave dá acesso total ao servidor. Não a envie a ninguém. O `.gitignore` já impede que ela
> entre no Git.

## 7. Enviar o código e instalar

No **PowerShell do seu computador**, na pasta do Concord. Troque `IP` pelo IP estático:

```powershell
# O ssh do Windows exige que só você leia a chave
icacls LightsailDefaultKey-sa-east-1.pem /inheritance:r /grant:r "$($env:USERNAME):R"

git archive --format=tar.gz -o concord.tar.gz HEAD
scp -i LightsailDefaultKey-sa-east-1.pem concord.tar.gz ubuntu@IP:~
ssh -i LightsailDefaultKey-sa-east-1.pem ubuntu@IP
```

Já **dentro do servidor**:

```bash
rm -rf concord && mkdir concord && tar -xzf concord.tar.gz -C concord && cd concord
sudo bash deploy/setup.sh meuconcord.duckdns.org voz-meuconcord.duckdns.org seu@email.com
```

A primeira vez leva de 15 a 30 minutos, porque o Concord é compilado numa máquina pequena. No fim,
aparece o endereço do site.

## 8. Configurar (LGPD e administração)

```bash
sudo nano /etc/concord/concord.env
```

Preencha os campos abaixo e depois rode `sudo systemctl restart concord`:
- `CONCORD_CONTROLADOR`: quem opera a instância (você).
- `CONCORD_CONTATO`: o e-mail de atendimento ao titular.
- `CONCORD_ADMINS`: o seu id. Abra o site, clique no escudo, depois em "Baixar meus dados", e copie
  o `id_pseudonimo`.

## Atualizar para uma versão nova

Repita o passo 7. O script mantém os dados, as chaves e a configuração.

## Operação do dia a dia

| Tarefa | Comando no servidor |
|---|---|
| Ver os logs | `sudo journalctl -u concord -u livekit -f` |
| Reiniciar | `sudo systemctl restart concord` |
| Ver os backups | `ls /var/backups/concord` |
| Baixar um backup (no seu PC) | `scp -i LightsailDefaultKey-sa-east-1.pem ubuntu@IP:/var/backups/concord/concord-AAAA-MM-DD.db .` |

## Custos

- **Instância:** preço fixo do plano.
- **Tráfego:** o plano inclui uma franquia mensal. Em São Paulo ela é metade da franquia das outras
  regiões. O excedente custa cerca de US$ 0,15/GB, e só a transmissão de tela pesa: cerca de
  3,6 GB por espectador por hora em 1080p60. O consumo aparece em **Métricas → Transferência de
  dados**.
- **IP estático:** grátis enquanto estiver ligado a uma instância.
- **Snapshots** (cópias da máquina inteira): são cobrados à parte. O backup diário do Concord já
  fica dentro da máquina, sem custo.

## LGPD neste servidor

- **Onde ficam os dados:** em São Paulo. A AWS atua como fornecedora de infraestrutura (operadora).
  Mencione isso na política de privacidade.
- **Logs:** nenhum IP é gravado. O Caddy fica sem log de acesso e o LiveKit só registra avisos.
- **Backups:** guardam 7 dias, dentro do prazo de retenção.
