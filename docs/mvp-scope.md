# Escopo do MVP (em construção)

Documento de decisões da sessão de grilling. **Não commitado até acordo explícito.**  
Última atualização: 2026-08-03.

## Objetivo do MVP

No Host da LAN, o Operador baixa o binário da Plataforma, inicia (Bootstrap), e consegue fazer Deploy de Aplicações (pull de imagem **ou** build local). Consumidores acessam por HTTP via Hostname de Aplicação resolvido por DNS local da própria stack — **sem depender de um serviço externo/SaaS** no caminho feliz.

> Esclarecimento: “sem internet / sem serviço externo” = não depender de SaaS de terceiros para DNS/controle. Não significa necessariamente air-gap total (ex.: `docker pull` de registry público pode existir quando houver rede).

## Decisões fechadas

| # | Tema | Decisão |
|---|---|---|
| 1 | Ambiente primário | Host na **LAN** (não VPS-first) |
| 2 | Papéis | Operador único; **Consumidores** na LAN usam as apps |
| 3 | Controle | **Daemon + API gRPC + CLI** (UI de gestão fora do MVP) |
| 4 | Runtime de Aplicação | **Container Docker** (não processo solto no host) |
| 5 | Plataforma | **Um binário** que sobe containers de Infra e de Aplicação conforme necessário |
| 6 | Forma de Deploy no MVP | **Pull de imagem** **ou** **build local** (Dockerfile/contexto). **Sem GitHub/Git no MVP** |
| 7 | DNS local | **Obrigatório** para declarar MVP pronto |
| 8 | TLS | Só **HTTP** no MVP (HTTPS/CA local depois) |
| 9 | Hostname | Default `nome.<Sufixo DNS>`; **override** explícito permitido |
| 10 | DNS/descoberta | **dnsmasq** (container de Infra) — DNS “burro” `*.<sufixo>` → IP do Host. Consul rejeitado para o MVP; DNS no binário considerado e não escolhido |
| 11 | Estado da Plataforma | **PostgreSQL 18** (container de Infra subido no Bootstrap). SQLite foi considerado e rejeitado em favor de PG por familiaridade / caminho futuro |
| 12 | Proxy HTTP | **Traefik** (Docker provider; roteamento por `Host`) |
| 13 | Logs | **`self-host logs <app>`** — stream via gRPC a partir do Docker; sem retenção própria no MVP |
| 14 | CLI | Estilo Disco com subcomandos espaçados (`apps add`, não `apps:add`). Binário: **`self-host`**. Recursos: **`apps`** (não `projects`). Quickstart: `init` → `apps add` (`--image` \| `--path`) → `list` / `logs` / `remove`. Sem GitHub/`git push`/`init user@host` no MVP |
| 15 | Sufixo no `init` | **`self-host init --dns <sufixo>`**. Default: **`home.lan`** (evita `.local` / mDNS). Vários sufixos = follow-up pós-MVP |
| 16 | DNS do Consumidor | Após `init`, a CLI **imprime instruções** (IP do Host + apontar resolver). Sem integração automática com roteador/DHCP |
| 17 | Env vars | **`self-host apps env set|get|unset`** (gestão após create). `--env-file` fora do MVP |
| 18 | Linguagem | **Rust** (binário CLI + daemon; gRPC via tonic; Docker via API) |
| 19 | Portas / rede | LAN: **:80** Traefik + **:53** dnsmasq; apps sem publish direto. **gRPC do daemon também na LAN** (CLI em outro device). Publish arbitrário de portas de app fora do MVP |
| 20 | Auth gRPC | **API key** gerada no `init`; CLI persiste config local. Sem mTLS no MVP |

## Fora do MVP (explícito)

- UI web de gestão
- GitHub / git clone / webhooks / GHCR como fonte obrigatória (candidato a v0.2)
- Kubernetes / controller / Operator
- Let’s Encrypt / HTTPS
- Multi-operador / contas
- Multi-host / cluster
- Paridade com Dokploy/Coolify/Kubero
- Vários Sufixos DNS na mesma Plataforma (MVP = um, escolhido no `init`)
- `init user@host` via SSH (CLI no laptop → Host remoto)
- GitHub / git push to deploy

## Aberto — descoberta / DNS

**Fechado.** Ver decisões #10 e #12 e ADR `0003`.

Pesquisa de apoio: [docs/research/lan-dns-service-discovery.md](./research/lan-dns-service-discovery.md).

## Critério de pronto (rascunho)

1. Bootstrap: `self-host init [--dns]` sobe Infra (PG 18 + dnsmasq + Traefik), gera API key, imprime instruções de DNS
2. CLI (no Host ou outro device na LAN) autentica com a API key via gRPC
3. `apps add` por imagem **e** por build local
4. Consumidor na LAN resolve o Hostname e recebe HTTP da Aplicação
5. `self-host logs <app>` e `apps env set|get|unset` funcionam
6. Sem conta em serviço externo/SaaS no caminho feliz

## Próximos temas a grelhar

- Declarar entendimento compartilhado do MVP e preparar commit
