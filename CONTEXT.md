# Self-host PaaS (LAN)

Contexto de uma plataforma self-hosted para publicar aplicações numa rede local, operada por uma pessoa, consumida por outras na LAN — sem depender de um serviço externo (SaaS) para o caminho feliz.

## Language

**Plataforma**:
O sistema de controle instalado no host: o binário/daemon que faz bootstrap da infra e gerencia o ciclo de vida das aplicações.
_Avoid_: PaaS (como sinônimo vago), cluster, Kubernetes

**Aplicação**:
Um workload publicado para consumidores na LAN, executado como container Docker (a partir de imagem pronta ou build local).
_Avoid_: binário (reservado à Plataforma), serviço (sobrecarregado), site

**Bootstrap**:
O ato de iniciar a Plataforma no host a partir do binário (ex.: download + Enter), subindo a infra necessária para operar.
_Avoid_: install script como conceito de domínio, setup

**Operador**:
A pessoa que publica e gerencia Aplicações via CLI (e a API da Plataforma). No MVP há um único Operador.
_Avoid_: admin, user, developer (como papel do produto)

**Consumidor**:
Quem acessa Aplicações já publicadas na LAN (HTTP). Não opera a Plataforma.
_Avoid_: end user, cliente, visitor

**Host**:
A máquina na LAN onde a Plataforma roda e onde os containers das Aplicações (e da infra) sobem.
_Avoid_: node, server, VPS (VPS não é o ambiente primário do MVP)

**Deploy**:
A ação do Operador de tornar uma Aplicação disponível na LAN a partir de uma imagem Docker existente ou de um build local (Dockerfile/contexto).
_Avoid_: release, publish, ship (como sinônimos oficiais)

**Hostname de Aplicação**:
O nome DNS pelo qual Consumidores alcançam uma Aplicação na LAN, em geral `nome.<sufixo>` com override explícito possível.
_Avoid_: URL (a URL inclui esquema/path), domínio público

**Sufixo DNS**:
O sufixo configurável da zona local da Plataforma sob o qual os Hostnames de Aplicação são derivados. No MVP existe um único Sufixo DNS, passado em `self-host init --dns` (default **`home.lan`**, de propósito sem `.local` por conflito com mDNS). Suporte a vários sufixos fica fora do MVP.
_Avoid_: domínio, TLD, zone (como jargão DNS cru no glossário), home.local (como default)

**Infra da Plataforma**:
Componentes que a Plataforma sobe para si (ex.: proxy de tráfego HTTP, descoberta/DNS local, store de estado) — não são Aplicações do Operador.
_Avoid_: dependências (ambíguo com deps de app), sidecars
