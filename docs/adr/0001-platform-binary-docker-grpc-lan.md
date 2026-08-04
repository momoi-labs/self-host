# Plataforma = binário + Docker na LAN; controle via gRPC

Ambiente primário é um Host na LAN, sem depender de SaaS externo no caminho feliz. Decidimos que a Plataforma é um único binário (daemon) que sobe a Infra e as Aplicações como containers Docker; o Operador controla via CLI falando gRPC com o daemon. UI, Kubernetes e Git-as-source ficam fora do MVP — o caminho de Deploy do MVP é pull de imagem ou build local.

**Status:** accepted
