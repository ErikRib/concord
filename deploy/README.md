# Colocar o Concord num servidor (grátis: Oracle Cloud + DuckDNS)

Resultado: o Concord no ar 24 horas, em `https://SEU-NOME.duckdns.org`, com voz e transmissão de
tela, numa máquina em **São Paulo**, sem custo.

O script `setup.sh` instala e configura:
- **Concord** e **LiveKit** como serviços, que reiniciam sozinhos;
- **Caddy**, com HTTPS automático e **sem log de acesso**, ou seja, sem registro de IP;
- firewall da máquina;
- **backup diário** com 7 dias de cópias;
- atualizações automáticas de segurança.

As chaves secretas são geradas no próprio servidor e nunca saem dele.

> Não testei o `setup.sh` numa máquina real da Oracle, só a sintaxe. Se algum passo falhar, a
> mensagem de erro no terminal mostra onde.

## 1. Conta na Oracle Cloud

1. Crie a conta em <https://www.oracle.com/cloud/free/>.
2. Em **Home Region**, escolha **Brazil East (Sao Paulo)**. Essa escolha **não muda depois**, e é
   ela que mantém os dados no Brasil.
3. A Oracle pede cartão de crédito só para verificar a identidade. Recursos "Always Free" não são
   cobrados.

**Para não ter gastos:**
- Não clique em "Upgrade to Pay As You Go" sem entender o que muda.
- Em *Billing → Budgets*, crie um orçamento de US$ 1 com alerta por e-mail. Se algo começar a
  custar, você fica sabendo na hora.

## 2. Criar a máquina

*Compute → Instances → Create instance*:

| Campo | Valor |
|---|---|
| Image | **Canonical Ubuntu 24.04** |
| Shape | **Ampere → VM.Standard.A1.Flex**, com **2 OCPU e 12 GB** (o grátis vai até 4 OCPU e 24 GB no total) |
| SSH keys | **Generate a key pair** e baixe a chave privada (`.key`) |

Anote o **IP público** da máquina.

Se aparecer **"Out of capacity"**, tente outro *Availability Domain*, tente mais tarde ou use o
shape **VM.Standard.E2.1.Micro** (AMD, 1 GB, também grátis; o script cria memória swap para
compilar).

## 3. Liberar as portas no painel

*Networking → Virtual Cloud Networks →* sua VCN *→ Security Lists → Default → Add Ingress Rules*,
com origem `0.0.0.0/0`:

| Protocolo | Portas | Para quê |
|---|---|---|
| TCP | 80, 443 | Site e HTTPS |
| TCP | 7881 | Voz (quando o UDP está bloqueado na rede de quem usa) |
| UDP | 3478 | TURN (voz atrás de firewall) |
| UDP | 50000-60000 | Áudio e vídeo |

## 4. Domínios grátis (DuckDNS)

Em <https://www.duckdns.org> (login com GitHub ou Google), crie **dois** subdomínios apontando para
o IP público da máquina:
- `SEU-NOME` → o site (ex.: `meuconcord.duckdns.org`)
- `voz-SEU-NOME` → a voz (ex.: `voz-meuconcord.duckdns.org`)

## 5. Enviar o código e instalar

No **PowerShell do seu computador**, na pasta do Concord. Troque `chave.key` pelo arquivo baixado
e `IP` pelo IP da máquina:

```powershell
# O ssh do Windows exige que só você leia a chave
icacls chave.key /inheritance:r /grant:r "$($env:USERNAME):R"

git archive --format=tar.gz -o concord.tar.gz HEAD
scp -i chave.key concord.tar.gz ubuntu@IP:~
ssh -i chave.key ubuntu@IP
```

Já **dentro do servidor**:

```bash
rm -rf concord && mkdir concord && tar -xzf concord.tar.gz -C concord && cd concord
sudo bash deploy/setup.sh meuconcord.duckdns.org voz-meuconcord.duckdns.org seu@email.com
```

A primeira execução leva alguns minutos, porque compila o Concord. No fim, aparece o endereço do
site.

## 6. Configurar (LGPD e administração)

```bash
sudo nano /etc/concord/concord.env
```

Preencha os campos abaixo e depois rode `sudo systemctl restart concord`:
- `CONCORD_CONTROLADOR`: quem opera a instância (você).
- `CONCORD_CONTATO`: o e-mail de atendimento ao titular.
- `CONCORD_ADMINS`: o seu id. Abra o site, clique no escudo, depois em "Baixar meus dados", e copie
  o `id_pseudonimo`.

## Atualizar para uma versão nova

Repita o passo 5. O script mantém os dados, as chaves e a configuração.

## Operação do dia a dia

| Tarefa | Comando no servidor |
|---|---|
| Ver os logs | `sudo journalctl -u concord -u livekit -f` |
| Reiniciar | `sudo systemctl restart concord` |
| Ver os backups | `ls /var/backups/concord` |
| Baixar um backup (no seu PC) | `scp -i chave.key ubuntu@IP:/var/backups/concord/concord-AAAA-MM-DD.db .` |

**Máquina ociosa:** a Oracle pode desligar instâncias gratuitas que passam muitos dias quase sem
uso. Baixe um backup de vez em quando. Se a máquina for desligada, basta criar outra, rodar o setup
e colocar o backup como `/var/lib/concord/concord.db`.

## LGPD neste servidor

- **Onde ficam os dados:** em São Paulo. A Oracle atua como fornecedora de infraestrutura
  (operadora). Mencione isso na política de privacidade.
- **Logs:** o Caddy não registra acessos e o LiveKit só registra avisos. Nenhum IP fica gravado.
- **Backups:** guardam 7 dias, dentro do prazo de retenção. "Apagar meus dados" não alcança cópias
  já feitas, que somem sozinhas em até 7 dias.
- **Arquivos sensíveis:** `/etc/concord/*.env` (chaves) e `/var/lib/concord` (banco) só podem ser
  lidos pelos serviços.
