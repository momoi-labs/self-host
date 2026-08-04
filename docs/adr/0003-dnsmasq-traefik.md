# DNS via dnsmasq + proxy Traefik

Para o MVP, clientes LAN precisam resolver `nome.<sufixo>` para o IP do Host; o roteamento HTTP por `Host` fica no reverse proxy. A pesquisa em `docs/research/lan-dns-service-discovery.md` mostrou que DNS “burro” basta e que Consul é overkill. Escolhemos **dnsmasq** como container de Infra (menor superfície que CoreDNS/Consul; menos risco que implementar DNS no binário agora) e **Traefik** com Docker provider. Consul e DNS embutido no binário ficam de fora do MVP.

**Status:** accepted
