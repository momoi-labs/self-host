# Exposição na LAN: HTTP, DNS, gRPC + API key

No Host, a Plataforma publica na LAN a porta **80** (Traefik) e **53** (dnsmasq). Aplicações não fazem port-publish direto — tráfego entra pelo Hostname. O daemon gRPC também escuta na LAN para a CLI poder rodar em outro device. Autenticação: **API key** gerada no `init`, persistida na config local da CLI (estilo Disco). mTLS e publish arbitrário de portas de app ficam fora do MVP.

**Status:** accepted
