# Estado da Plataforma em PostgreSQL 18

O daemon precisa persistir Aplicações, hostnames e config além do processo. Consideramos arquivos planos, SQLite e “sem persistência”. Escolhemos **PostgreSQL 18** como container de Infra no Bootstrap: mais operacionalmente pesado que SQLite num MVP de um Host, mas alinha com familiaridade do Operador e evita migração cedo se o modelo de dados crescer. A Plataforma sobe o PG; o Operador não administra um SaaS externo de banco.

**Status:** accepted
