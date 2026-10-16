# Decisões e questões abertas

Última atualização: 2026-10-16

## Como ler o status

- **Decidido**: o autor afirmou explicitamente.
- **Proposto**: sugerido durante os exercícios de design e não contestado. Precisa de confirmação antes de virar compromisso.
- **Aberto**: sem resposta ainda (ver a seção de questões).

## Log de decisões

| ID | Decisão | Status | Doc |
|---|---|---|---|
| D01 | Implementação em Rust com GPUI, nativo; sem Tauri nem front-end JS | Decidido | 06 |
| D02 | Produto local, de uso por engenheiro; sem modo headless, CI ou servidor | Decidido | 00 |
| D03 | Cada branch tem um único pai e vários filhos (árvore, não DAG) | Decidido | 02 |
| D04 | Hooks seguem a ideia dos hooks do Claude Code, com schema próprio de entrada e de saída | Decidido | 03 |
| D05 | Hooks executam de forma assíncrona; a UI nunca bloqueia por causa de um script lento | Decidido | 03 |
| D06 | Hook recebe um id de correlação na entrada e devolve o mesmo id na saída; o motor reconcilia | Decidido | 03 |
| D07 | Hook responde pelo stdout (sem modelo detached/callback) | Decidido | 03 |
| D08 | Motor único com dois front-ends (GPUI e MCP) | Decidido | 06 |
| D09 | Evitar previsões não determinísticas | Decidido | 00 |
| D10 | Sidebar com visão dupla: por stack e por nome | Decidido | 05 |
| D11 | Loop de review local (comentários, agente via MCP, devolutiva, antes/depois) | Proposto | 04 |
| D12 | Servidor MCP dentro do app; shim `gitmaster mcp` por socket Unix com token | Proposto | 04 |
| D13 | Metadata de stack no git config: `stackParent` e `stackParentBase` | Proposto | 02 |
| D14 | Restack com plano exato por commit, snapshot de refs, undo atômico e estado persistido | Proposto | 02 |
| D15 | Push com lease como passo explícito após o restack | Proposto | 02 |
| D16 | Hooks em três classes (gate, transform, observe); gates e observers em paralelo, transforms em sequência | Proposto | 03 |
| D17 | Hooks de repo exigem aprovação (hash do comando); padrão lê só config global/usuário; `includeIf` por projeto | Proposto | 03 |
| D18 | Hooks nativos do Git continuam rodando, antes dos do GitMaster | Proposto | 03 |
| D19 | `pre-commit` roda contra a index tree (checkout temporário) | Proposto | 03 |
| D20 | MCP em tiers Read, Propose, Aprovação, Apply; apply só aceita `plan_id` | Proposto | 04 |
| D21 | IDs de hunk determinísticos (hash do conteúdo) | Proposto | 04 |
| D22 | Resolvedor de conflitos em três painéis, lados rotulados por papel (não ours/theirs) | Proposto | 05 |
| D23 | Working Copy: "staged" e "lido" como eixos ortogonais; lista única com checkbox tri-state | Decidido (2026-10-16) | 05 |
| D24 | Stage por linha sobre modelo de diff exato, escrevendo o blob do index direto | Decidido (2026-10-16) | 05 |
| D25 | Histórico: defasagem de stack como aresta tracejada; dono do commit pela metadata do stack | Proposto | 05 |
| D26 | Backend Git híbrido: libgit2 para objetos/index/merge/rebase em memória; subprocess `git` para status em escala | Proposto | 06 |
| D27 | Começar com quatro crates (schema, core, ui, mcp) e extrair o resto sob demanda | Proposto | 06 |

## Questões abertas

- **Q1. Contrato de hooks para shell:** (A) JSON puro; (B) pipe de texto; (C) JSON canônico com `mode = text` opcional por hook (codec no host). Recomendação anterior: C.
- **Q2. Restack com working copy sujo:** bloquear com banner ou autostash determinístico registrado no operation log. Começar pelo bloqueio.
- **Q3. Review local:** confirmar a inclusão no produto e onde guardar comentários e snapshots (SQLite em `.git/` ou diretório de dados do app).
- **Q4. Nomes das chaves de config:** `gitmaster.*` ou espelhar o formato de hooks por config do Git recente (`hook.<nome>.command`, a verificar).
- **Q5. Hook `split-commits`:** proposta de agrupar hunks em vários commits, confirmada pelo humano. Ideia para depois da base.
- **Q6. libgit2 versus gitoxide:** checar maturidade de status, merge e rebase em memória antes de fechar D26. **Spike C (2026-10-16, `spikes/c-rebase`):** o rebase em memória do libgit2 bate com `git rebase --onto` em 12 cenários (stack limpa, conflito com os três lados, rename, merge commit, binário, modo, 3000 arquivos, pai amendado) sem tocar index nem working tree; 200 commits em 135 ms (contra 2377 ms de `git merge-tree` por commit). Commit que fica vazio exige tratar `Applied` como skip. `merge-tree --merge-base` exige git 2.40+. Não testados: rerere, hooks de rebase, assinatura, submódulos. Gitoxide não foi testado. **Spike D (`spikes/d-status`):** status por subprocess é 2,5x mais rápido que o libgit2 a 500 mil arquivos e `--no-optional-locks` evita quebrar commits concorrentes; confirma o status por subprocess de D26. fsmonitor sem veredito (daemon indisponível nesta máquina).
- **Q7. Texto editável no GPUI:** o painel de resultado do resolvedor e a visualização de diff exigem texto editável e virtualizado. Avaliar bibliotecas de terceiros (ex.: gpui-component) e licenças (verificar a licença dos crates do Zed além do `gpui`). **Spike B (2026-10-16, `spikes/b-text`):** portar o text area do rvw (do mesmo autor, MIT; 55 testes passam, janela abre; mouse, IME, clipboard e wrap só lidos do código, falta teste manual). O editor do Zed é GPL-3.0-or-later; `gpui-component` é Apache-2.0 mas depende de outra versão do gpui. O texto inteiro é shaped a cada frame: serve ao composer, não ao painel de conflitos. Decisão do autor (2026-10-16): o GitMaster será todo open source e o código do rvw pode ser usado.
- **Q8. Escopo de Tower não discutido:** pull requests, worktrees, submódulos, LFS.
- **Q9. Layout alternativo do resolvedor** (unificado/empilhado) para telas pequenas.
- **Q10. Licença e modelo de produto:** como combinar open source com "produto premium" (open core, binário pago com fonte aberta, etc.).
