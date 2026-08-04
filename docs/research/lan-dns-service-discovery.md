# DNS e service discovery em LAN para PaaS self-hosted (MVP)

**Data:** 2026-08-03  
**Pergunta de pesquisa:** Para um PaaS self-hosted em um único host na LAN, com apps em Docker atrás de reverse proxy por `Host`, qual abordagem de DNS / service discovery atende o MVP (`name.<suffix>` → IP do host) sem depender de SaaS externo — e o que deixar para uma fase 2?

## Como ler

- **Fatos** vêm de docs oficiais, man pages, RFCs ou repositórios first-party; cada afirmação factual tem citação.
- **Opinião / recomendação** está isolada na seção [Recomendação](#recomendação-opinião).
- **Incertezas** estão marcadas explicitamente.
- Esboços de container/compose só usam flags/comandos documentados pelas fontes citadas; placeholders (`HOST_LAN_IP`, `paas.lan`) são do nosso cenário, não inventados pelo produto.

### Restrições do projeto (critérios de avaliação)

| Restrição | Implicação |
| --- | --- |
| Um único Host na LAN | Sem cluster multi-nó no MVP |
| Control-plane sobe containers de infra sob demanda | DNS pode ser um container gerenciado pela plataforma |
| Apps = containers Docker | Routing pode usar labels Docker (Traefik) |
| Consumidores na LAN usam HTTP | DNS deve resolver a partir de laptops/phones na LAN, não só entre containers |
| DNS local **obrigatório** no MVP | `name.<suffix>` → IP do host; proxy roteia por `Host` |
| Sem SaaS no happy path | Consul self-hosted OK; DNS na nuvem não |
| TLS fora do MVP | Só HTTP |
| Kubernetes fora de escopo | Ignorado |

---

## Respostas diretas às perguntas do brief

### “Dumb DNS” basta, ou precisa de service discovery completo?

**Para o MVP descrito — `*.paas.lan` (exemplo) → IP do host + reverse proxy por header `Host` — DNS “burro” (wildcard / domínio inteiro → um A record) é suficiente.**

Motivo factual:

1. O DNS só precisa entregar o endereço do host onde o proxy escuta.
2. O Traefik (provider Docker) monta rotas a partir de labels do container, tipicamente `Host(\`example.com\`)` — a descoberta de *qual container* atende o hostname é do proxy, não do DNS. Fonte: [Traefik Docker provider](https://doc.traefik.io/traefik/v3.5/reference/install-configuration/providers/docker/).
3. Service discovery completo (catálogo, health, SRV, multi-instância) resolve *outro* problema: achar o endereço/porta de instâncias saudáveis. No modelo “tudo no mesmo host atrás de um proxy HTTP”, isso é redundante no MVP.

### Dá para começar “dumb” e adicionar Consul depois sem reescrever o modelo?

**Sim, se o modelo de plataforma permanecer: nome da app → hostname HTTP → proxy no host.**

- Fase 1: DNS wildcard → IP do host; Traefik Docker provider com labels `Host(\`name.paas.lan\`)`.
- Fase 2 (opcional): registrar serviços no Consul (`PUT /v1/agent/service/register`) e/ou usar Traefik `consulCatalog` provider. Fontes: [Consul register services](https://developer.hashicorp.com/consul/docs/register/service/vm), [Traefik Consul Catalog](https://doc.traefik.io/traefik/reference/install-configuration/providers/hashicorp/consul-catalog/).

O contrato externo para clientes LAN (`http://name.paas.lan`) pode permanecer. O que muda é *de onde* o Traefik tira a config dinâmica (Docker labels → tags Consul), não o papel do DNS unicast na LAN.

**Incerteza:** se na fase 2 o DNS passar a apontar para IPs de containers individuais (em vez do host), o modelo de rede/publicação de portas muda — isso *seria* uma mudança de arquitetura, não só de implementação.

---

## Comparação das opções

### Tabela 1 — Visão geral

| Opção | Papel principal | Wildcard `*.suffix` → 1 IP | Service catalog / health | Adequado a clientes LAN | Peso operacional (MVP solo) |
| --- | --- | --- | --- | --- | --- |
| **Consul** | Catalog + DNS de serviços | Não é o caso de uso principal; DNS resolve nomes no domínio Consul (default `consul.`) a partir do catálogo | Sim (registro + checks) | Sim, se clientes usarem o resolver Consul (porta default 8600) ou houver forwarder na 53 | Alto |
| **CoreDNS** | Servidor DNS pluginável | Sim (`file` com `*` RFC 1034, ou `template`) | Não (sem plugins extras de catalog) | Sim, na porta 53 (ou forwarder) | Médio-baixo |
| **dnsmasq** | DNS/DHCP leve | Sim (`--address=/domain/ip`) | Não | Sim | Baixo |
| **DNS no binário** | Resolver autoritativo embutido | Depende da implementação (zona com wildcard / lógica custom) | Não, salvo se a plataforma implementar | Sim | Variável (menos containers; mais código) |
| **mDNS / Avahi** | Descoberta zero-config em `.local` | Não cobre o padrão “zona unicast com wildcard controlada pela plataforma” | DNS-SD (outro modelo) | Parcial (suporte por SO/cliente) | Baixo software / alto risco de fit |
| **Docker embedded DNS** | Nomes entre containers na mesma user-defined network | N/A para LAN | Só entre containers na rede | **Não** resolve nomes para phones/laptops na LAN | — |

Citações detalhadas nas seções abaixo e em [Fontes](#fontes).

### Tabela 2 — Fit às restrições do MVP

| Critério | Consul | CoreDNS | dnsmasq | DNS embutido | mDNS | Docker DNS |
| --- | --- | --- | --- | --- | --- | --- |
| Sem SaaS | ✅ self-hosted | ✅ | ✅ | ✅ | ✅ | ✅ |
| Um host | ✅ (server bootstrap-expect=1 documentado) | ✅ | ✅ | ✅ | ✅ | ✅ |
| `name.suffix` → IP host | ⚠️ possível via registro artificial / domínio custom; desenho natural é `*.service.consul` → endereço da instância | ✅ | ✅ | ✅ | ❌ padrão inadequado | ❌ |
| Traefik + Docker | ✅ provider Catalog **ou** Docker (independente) | ✅ (só DNS; Traefik Docker à parte) | ✅ idem | ✅ idem | ⚠️ | Docker provider usa API Docker, não o embedded DNS |
| Peso solo MVP | ❌ alto | ✅ | ✅✅ | ✅ se escopo mínimo | ❌ fit fraco | ❌ não resolve LAN |

---

## 1. HashiCorp Consul

### O que é (fatos)

- Consul DNS é a interface principal para consultar nós/serviços registrados quando service mesh está desabilitado e a rede não é Kubernetes. Fonte: [Consul DNS overview](https://developer.hashicorp.com/consul/docs/discover/dns).
- Por default, DNS escuta em `127.0.0.1:8600` e usa o domínio `consul`. Consul **não** usa a porta 53 por default porque exige privilégio elevado. Parâmetros relevantes: `client_addr`, `ports.dns`, `domain`, `alt_domain`, `recursors`. Fonte: [Configure Consul DNS behavior](https://developer.hashicorp.com/consul/docs/discover/dns/configure); defaults de porta: [Consul ports reference](https://developer.hashicorp.com/consul/docs/reference/architecture/ports) (DNS TCP/UDP **8600**).
- `client_addr` default `127.0.0.1`; `ports.dns` default **8600**. Fonte: [Agent configuration — general](https://developer.hashicorp.com/consul/docs/reference/agent/configuration-file/general).
- Domínio DNS configurável via `domain` (default `consul.`). Fonte: [DNS parameters](https://developer.hashicorp.com/consul/docs/reference/agent/configuration-file/dns).

### Rodar como container (um nó)

Documentação oficial de deploy Docker (server único):

```bash
docker run --name=consul-server -d \
  -p 8500:8500 -p 8600:8600/udp \
  hashicorp/consul \
  consul agent -server -ui -node=server-1 -bootstrap-expect=1 \
  -client=0.0.0.0 -data-dir=/consul/data
```

Fonte: [Deploy Consul server agent on Docker](https://developer.hashicorp.com/consul/docs/deploy/server/docker).

Compose multi-nó na mesma página mapeia também `8600:8600/tcp` e `8600:8600/udp` no server exposto. Flags do exemplo oficial multi-nó incluem `-bootstrap-expect=3` e `-retry-join=...` — **não** usar esses valores inventados fora do que a doc mostra.

**Implicação LAN:** clientes precisariam apontar DNS para `host:8600` (incomum em DHCP/SO) **ou** um forwarder na 53 (dnsmasq/CoreDNS/`iptables`) encaminhando o domínio Consul. A própria HashiCorp documenta forward de queries do domínio Consul a partir de um DNS existente. Fonte: [Configure Consul DNS behavior](https://developer.hashicorp.com/consul/docs/discover/dns/configure) (“Forward DNS for Consul Service Discovery”).

### Como apps se registram

Métodos documentados em VM/agent:

- Definição no diretório de config do agent + start/reload
- CLI `consul services register`
- HTTP `PUT /v1/agent/service/register`

Fonte: [Register services and health checks](https://developer.hashicorp.com/consul/docs/register/service/vm); API: [Service — Agent HTTP API](https://developer.hashicorp.com/consul/api-docs/agent/service).

A plataforma (daemon) precisaria registrar/deregistrar a cada deploy — peso operacional real no MVP.

### Fit com Traefik / Docker

- **Traefik Docker provider:** lê labels nos containers; não precisa de Consul. Fonte: [Traefik & Docker](https://doc.traefik.io/traefik/v3.5/reference/install-configuration/providers/docker/).
- **Traefik Consul Catalog provider:** `providers.consulCatalog`, endpoint default `127.0.0.1:8500`, tags no estilo Traefik nos serviços Consul. Fonte: [Traefik & Consul Catalog](https://doc.traefik.io/traefik/reference/install-configuration/providers/hashicorp/consul-catalog/).

No MVP com um host + Docker, o provider Docker já cobre routing por `Host` sem catálogo externo.

### Licença (BUSL etc.)

- Em agosto de 2023 a HashiCorp anunciou mudança de MPL 2.0 para **Business Source License (BSL / BUSL) 1.1** em releases futuros dos produtos; APIs/SDKs/libraries em geral permanecem MPL 2.0. Fonte: anúncio [HashiCorp adopts Business Source License](https://www.hashicorp.com/en/blog/hashicorp-adopts-business-source-license) (resumo também em resultados oficiais / [Licensing FAQ](https://www.hashicorp.com/license-faq)).
- Existe edição **Consul Community Edition (CE)** vs Enterprise com features adicionais; discovery básico está na CE. Fonte: [Consul editions](https://developer.hashicorp.com/consul/docs/fundamentals/editions).

**Incerteza / aviso:** interpretação jurídica do BSL para um produto comercial que *embute* Consul como parte de uma oferta competitiva deve ser feita por aconselhamento legal; a FAQ da HashiCorp é a orientação oficial vinculante que eles publicam. Este documento **não** conclui compliance.

### Peso operacional para operador solo (MVP)

| Aspecto | Custo |
| --- | --- |
| Processo extra (server agent) | Sim |
| Porta DNS não-53 | Forwarder ou config exótica nos clientes |
| Ciclo register/deregister + health | Código e falhas a tratar |
| UI/API 8500 | Superfície a-secure |
| Licença BSL | Diligence |

**Veredito factual de fit:** Consul resolve bem *service discovery distribuído*; o MVP só precisa de *resolução unicast de um sufixo para o IP do host*.

---

## 2. CoreDNS

### Como container

- Imagem oficial: `coredns/coredns` no Docker Hub; releases também como imagens. Fonte: [CoreDNS installation manual](https://coredns.io/manual/installation/), [Docker Hub coredns/coredns](https://hub.docker.com/r/coredns/coredns/).
- Manual oficial usa frequentemente porta **1053** quando não é root (`-dns.port=1053`). Fonte: [Installation](https://coredns.io/manual/installation/).
- Para LAN, o usual é publicar **53/udp e 53/tcp** no host (requer privilégio/`CAP_NET_BIND_SERVICE` no processo que faz bind — detalhe de runtime; o manual enfatiza a limitação da porta 53).

Esboço alinhado ao padrão “montar Corefile” (flags `-conf` documentadas no manual; mapeamento de porta é Docker genérico):

```bash
docker run --rm -d --name coredns \
  -p 53:53/udp -p 53:53/tcp \
  -v "$PWD/Corefile:/Corefile:ro" \
  coredns/coredns -conf /Corefile
```

(O manual mostra `./coredns -dns.port=1053 -conf Corefile`; a imagem Docker Hub é o binário CoreDNS. Ajuste de porta interna/host conforme necessidade.)

### Wildcard / rewrite para um único A

Três mecanismos oficiais relevantes:

1. **Plugin `file`** — zona estilo RFC 1035; wildcards DNS clássicos usam owner `*.<domain>` (síntese de RRs). Fontes: [CoreDNS file](https://coredns.io/plugins/file/), [RFC 1034 §4.3.3 Wildcards](https://www.ietf.org/rfc/rfc1034.html).
2. **Plugin `template`** — resposta dinâmica por regex; exemplo oficial sintetiza A a partir do nome da query. Fonte: [CoreDNS template](https://coredns.io/plugins/template/).
3. **Plugin `rewrite`** — reescreve a *question* (e opcionalmente a answer); útil para mapear nomes a outro nome interno, não necessariamente o caminho mais curto para “tudo → mesmo IP”. Fonte: [CoreDNS rewrite](https://coredns.io/plugins/rewrite/).

Exemplo ilustrativo com `template` (IP e zona são do nosso cenário; sintaxe do plugin é oficial):

```text
paas.lan:53 {
    template IN A {
        match .*\.paas\.lan\.$
        answer "{{ .Name }} 60 IN A HOST_LAN_IP"
        fallthrough
    }
    # opcional: forward . <upstream> para o resto
}
```

Plugins mínimos para o caso “dumb”: `template` **ou** `file` (+ tipicamente `errors`/`log` em produção — convenção operacional, não requisito do MVP).

### Fit MVP

Alto: um container, zona fixa, zero catálogo, porta 53 clássica para DHCP da LAN.

---

## 3. dnsmasq

### Wildcard / domínio → IP

Man page oficial:

> `--address=/<domain>/[<domain>...]/[<ipaddr>]` — Specify an IP address to return for any host in the given domains. A (or AAAA) queries in the domains are never forwarded and always replied to with the specified IP address…

Também: `/#/` casa qualquer domínio; `/etc/hosts` e leases DHCP sobrescrevem nomes individuais.

Fonte: [dnsmasq man page](https://dnsmasq.org/docs/dnsmasq-man.html).

Exemplo alinhado ao man:

```text
address=/paas.lan/HOST_LAN_IP
```

Isso faz `app.paas.lan`, `foo.paas.lan`, etc. retornarem `HOST_LAN_IP` (comportamento descrito para “any host in the given domains”).

### Fit MVP

Muito alto para o requisito mínimo. Menos extensível que CoreDNS (plugins), mas o MVP não precisa disso.

**Nota:** não há imagem Docker “oficial dnsmasq.org” comparável à do CoreDNS; empacotar Alpine/`dnsmasq` é prática comum mas **não** é documento first-party do projeto dnsmasq — marcar como implementação do operador.

---

## 4. DNS embutido no binário da plataforma

### Abordagem (Rust / geral)

Em Rust, o ecossistema first-party atual é **Hickory DNS** (ex-Trust-DNS):

- Crates: `hickory-server` (biblioteca para construir servers), `hickory-dns` (binário). Fonte: [GitHub hickory-dns](https://github.com/hickory-dns/hickory-dns), [crates.io hickory-dns](https://crates.io/crates/hickory-dns).
- Manual: authoritative nameserver com zone files + `config.toml`; exemplo de run: `hickory-dns --port 2345 --config=./config.toml --zone-dir=.`. Fonte: [Authoritative Name server](https://hickory-dns.org/book/hickory/authoritative_nameserver.html).

Design geral (independente de linguagem):

- Thread/task UDP+TCP 53 (ou 53 só com capability)
- Zona in-memory ou arquivo com `*.paas.lan A HOST_IP`
- Control-plane atualiza o IP se a interface LAN mudar

### Pros / cons vs sidecar container

| | Embutido no daemon | Container (CoreDNS/dnsmasq) |
| --- | --- | --- |
| **Prós** | Menos peça móvel; IP pode reagir a eventos da plataforma sem reload externo | Isolamento; upgrade independente; configs bem documentadas; crash do DNS ≠ crash do control-plane (ou vice-versa) |
| **Contras** | Privilegio de bind 53 no binário principal; superfície de segurança maior; reimplementar edge cases DNS | Mais um container; sincronizar `HOST_LAN_IP` via volume/env/reload |

**Incerteza:** maturidade de APIs in-process do `hickory-server` para “wildcard dinâmico sem zone file” — verificar docs.rs na versão pinada; o manual público enfatiza zone files + config.

---

## 5. mDNS / Avahi (relevância limitada)

### O que é

Avahi implementa mDNS/DNS-SD (Zeroconf / Bonjour-like) para descoberta na rede local; nss-mdns permite lookup de `*.local`. Fonte: [avahi.org](https://avahi.org/).

### Limitações relevantes ao MVP

- **RFC 6762** define Multicast DNS; a seção “Wildcard Queries” trata de `qtype`/`qclass` ANY (responder com *todos* os RRs que casam), **não** de owner names `*.suffix` unicast como em RFC 1034. Fonte: [RFC 6762 §6.5](https://www.rfc-editor.org/rfc/rfc6762.html).
- Wildcards unicast clássicos (`*.example.com` sintetizando A) são de **DNS autoritativo unicast** ([RFC 1034 §4.3.3](https://www.ietf.org/rfc/rfc1034.html)), não o modelo “cada host anuncia seu próprio nome” do mDNS.
- Avahi expõe `AVAHI_ERR_IS_PATTERN` (−7) na API de erros — indício de tratamento especial/rejeição de nomes-padrão na API. Fonte: [Avahi error.h](http://avahi.org/doxygen/html/error_8h.html).
- Domínio típico mDNS é `.local` (ecossistema Bonjour/Avahi), não um sufixo arbitrário `paas.lan` configurável via DHCP da mesma forma que um nameserver unicast.

**Conclusão factual de fit:** mDNS não substitui um nameserver unicast que responde `*.paas.lan` → IP do host para clientes configurados via DHCP. Pode no máximo complementar anúncios pontuais (`hostname.local`), com suporte desigual entre dispositivos.

---

## 6. Docker embedded DNS (o que resolve e o que não)

Fatos das docs Docker:

- Containers em **user-defined networks** usam o embedded DNS em `127.0.0.11`, que resolve nomes/aliases **entre containers na mesma rede** e encaminha lookups externos aos DNS do host. Fonte: [Docker networking overview — DNS services](https://docs.docker.com/engine/network/).
- Em **user-defined bridge**, containers resolvem uns aos outros por nome; no default bridge, não (salvo legado `--link`). Fonte: [Bridge network driver](https://docs.docker.com/engine/network/drivers/bridge/).

**O que NÃO resolve para o MVP:** notebooks e phones na LAN **não** são membros da Docker network do host; eles não consultam `127.0.0.11` do daemon. Precisam de DNS unicast alcançável no IP LAN do host (ou de um forwarder no roteador).

---

## Arquitetura mínima do MVP (fatos + desenho)

```text
[Cliente LAN] --DNS--> [dnsmasq|CoreDNS|:53] --A--> HOST_LAN_IP
[Cliente LAN] --HTTP Host: app.paas.lan--> [Traefik|:80] --Docker provider--> [container app]
```

- DNS: domínio/wildcard → `HOST_LAN_IP` (dnsmasq `address=` ou CoreDNS `file`/`template`).
- Proxy: labels Traefik `traefik.http.routers.*.rule=Host(\`app.paas.lan\`)` ([doc Docker provider](https://doc.traefik.io/traefik/v3.5/reference/install-configuration/providers/docker/)).
- Operação LAN: DHCP option 6 (ou config manual) apontando para o IP do host — detalhe de rede doméstica/lab; não é feature do PaaS em si.

### Esboço compose (apenas ideias respaldadas)

**Consul (oficial):** ver comando `docker run` / compose em [Deploy Consul server on Docker](https://developer.hashicorp.com/consul/docs/deploy/server/docker).

**CoreDNS:** imagem `coredns/coredns`, `-conf /Corefile`, publish 53 — ver [installation](https://coredns.io/manual/installation/) + Hub.

**dnsmasq:** opção `address=/paas.lan/HOST_LAN_IP` da [man page](https://dnsmasq.org/docs/dnsmasq-man.html); packaging em container fica a cargo do operador.

**Traefik:** `providers.docker: {}` e label `Host(\`...\`)` — [doc](https://doc.traefik.io/traefik/v3.5/reference/install-configuration/providers/docker/).

Não inventar flags não documentadas.

---

## Fase 2 — Consul-style discovery (sem reescrever o modelo de hostnames)

| Passo | Mudança | Impacto no modelo |
| --- | --- | --- |
| Manter DNS wildcard → host | Nenhuma | Clientes LAN iguais |
| Traefik continua Docker provider | Nenhuma | Apps iguais |
| Opcional: subir Consul CE, registrar serviços, migrar Traefik para `consulCatalog` | Provider de config | Hostname HTTP pode permanecer |
| Opcional: health checks Consul | Observabilidade / drain | Interno |

Só seria rewrite grande se o DNS passasse a apontar para IPs de containers ou se se adotasse mesh/upstreams como path principal ([Consul DNS vs upstreams](https://developer.hashicorp.com/consul/docs/discover/dns)).

---

## Recomendação (opinião)

**MVP:** implementar **DNS “burro”** com **dnsmasq** (menor superfície) **ou CoreDNS** (se já se prevê plugins/`template`/forward seletivo). Preferência prática: **dnsmasq** se o único requisito for `address=/paas.lan/HOST_IP`; **CoreDNS** se a plataforma quiser Corefile versionado e extensões futuras sem trocar de servidor.

Acoplar **Traefik Docker provider** para routing por `Host`. Não introduzir Consul no MVP.

**Fase 2:** considerar Consul (CE, self-hosted) apenas se surgir necessidade real de catálogo multi-serviço, health-driven routing, ou múltiplos nós — não por “service discovery” genérico. DNS LAN wildcard pode permanecer.

**Consul no MVP:** **overkill** — custo (agent, registro, porta 8600/forwarder, BSL diligence) sem benefício para o critério de done (`*.suffix` → host + HTTP Host routing).

**DNS embutido no binário:** adiar até o MVP provar o fluxo com sidecar; só vale se o custo de um container DNS for inaceitável.

**mDNS:** não como solução primária do MVP.

---

## Fontes

1. HashiCorp — [Consul DNS overview](https://developer.hashicorp.com/consul/docs/discover/dns)  
2. HashiCorp — [Configure Consul DNS behavior](https://developer.hashicorp.com/consul/docs/discover/dns/configure)  
3. HashiCorp — [Deploy Consul server agent on Docker](https://developer.hashicorp.com/consul/docs/deploy/server/docker)  
4. HashiCorp — [Consul ports reference](https://developer.hashicorp.com/consul/docs/reference/architecture/ports)  
5. HashiCorp — [Agent configuration (general / ports / client_addr)](https://developer.hashicorp.com/consul/docs/reference/agent/configuration-file/general)  
6. HashiCorp — [DNS parameters (`domain`, `recursors`)](https://developer.hashicorp.com/consul/docs/reference/agent/configuration-file/dns)  
7. HashiCorp — [Register services and health checks](https://developer.hashicorp.com/consul/docs/register/service/vm)  
8. HashiCorp — [Service — Agent HTTP API](https://developer.hashicorp.com/consul/api-docs/agent/service)  
9. HashiCorp — [Consul editions](https://developer.hashicorp.com/consul/docs/fundamentals/editions)  
10. HashiCorp — [Adopts Business Source License](https://www.hashicorp.com/en/blog/hashicorp-adopts-business-source-license), [Licensing FAQ](https://www.hashicorp.com/license-faq)  
11. CoreDNS — [Installation](https://coredns.io/manual/installation/)  
12. CoreDNS — [file plugin](https://coredns.io/plugins/file/)  
13. CoreDNS — [template plugin](https://coredns.io/plugins/template/)  
14. CoreDNS — [rewrite plugin](https://coredns.io/plugins/rewrite/)  
15. Docker Hub — [coredns/coredns](https://hub.docker.com/r/coredns/coredns/)  
16. dnsmasq — [Man page](https://dnsmasq.org/docs/dnsmasq-man.html)  
17. Traefik — [Docker provider](https://doc.traefik.io/traefik/v3.5/reference/install-configuration/providers/docker/)  
18. Traefik — [Consul Catalog provider](https://doc.traefik.io/traefik/reference/install-configuration/providers/hashicorp/consul-catalog/)  
19. Docker Docs — [Networking overview (embedded DNS)](https://docs.docker.com/engine/network/)  
20. Docker Docs — [Bridge driver / user-defined networks](https://docs.docker.com/engine/network/drivers/bridge/)  
21. IETF — [RFC 1034 Domain Concepts (§4.3.3 Wildcards)](https://www.ietf.org/rfc/rfc1034.html)  
22. IETF — [RFC 6762 Multicast DNS](https://www.rfc-editor.org/rfc/rfc6762.html)  
23. Avahi — [Project site](https://avahi.org/), [error.h (`AVAHI_ERR_IS_PATTERN`)](http://avahi.org/doxygen/html/error_8h.html)  
24. Hickory DNS — [GitHub](https://github.com/hickory-dns/hickory-dns), [Authoritative nameserver manual](https://hickory-dns.org/book/hickory/authoritative_nameserver.html), [crates.io](https://crates.io/crates/hickory-dns)

---

## Incertezas remanescentes

- Comportamento exato de `address=/paas.lan/IP` vs necessidade de `address=/.paas.lan/IP` em versões específicas do dnsmasq: validar com `dig` na versão pinada (man descreve “any host in the given domains”; alguns tutoriais usam `/.domain/` — confirmar na man da build usada).
- Interpretação legal BSL para redistribuição embutida em produto comercial.
- Suporte mDNS/`*.local` em clientes móveis modernos — variar por SO; não pesquisado exaustivamente aqui.
- APIs in-process do `hickory-server` para zonas dinâmicas sem reload de arquivo — verificar na versão escolhida.
