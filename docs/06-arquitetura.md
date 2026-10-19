# Arquitetura

Última atualização: 2026-10-19. Status de cada decisão em `01`.

## Visão geral (D08 decidido)

Um motor único, dois front-ends (GPUI e MCP). Dependências apontam para dentro (arquitetura hexagonal, ports and adapters).

```
gm-ui (GPUI)          gm-mcp (rmcp + shim por socket)
      \                    /
            gm-core
      /         |         \
gm-git      gm-hooks    gm-store
(git2 +     (processos  (SQLite
 subprocess) e cadeia)   em .git/)

gm-schema: tipos compartilhados (serde + JSON Schema) por UI, MCP e hooks
```

O core define traits (`GitPort`, `HookPort`, `StorePort`) e os adaptadores as implementam. UI e MCP nunca importam `gm-git`. O core testa com fakes, sem repositório real.

## Regras

1. **Comandos entram, eventos saem.** `dispatch(Command { actor, ... })` devolve um `OperationId`; os front-ends assinam um stream de `Event`.
2. **Máquina de estados única da operação:** `Requested → Gating → AwaitingApproval → Executing → Done | Failed | Vetoed | Cancelled`. A UI e o MCP são adaptadores desse contrato. Ator humano pula a aprovação; agente não.
3. **Um escritor por repositório (atores).** Uma mailbox serializa as mutações; leituras vêm de snapshots imutáveis. O `Repository` do libgit2 não é `Sync`. A ordem do operation log é a ordem real de execução.
4. **Runtimes separados.** O GPUI tem executores próprios. Rodar o motor num runtime tokio dedicado, em threads à parte, e conversar por canais. A UI nunca espera o motor.
5. **Backend Git híbrido (D26):** libgit2 para objetos, index e merge/rebase em memória; subprocess `git` para status em repositórios massivos (o libgit2 não usa fsmonitor nem untracked cache) e para commit-graph em escala. Verificar o estado do gitoxide antes de fechar (Q6).
6. **Git é a fonte da verdade.** O estado do motor é derivado e descartável. Fora do Git ficam só metadados: pais de stack (git config), comentários e snapshots (store). Toda leitura revalida fingerprints; um watcher (`notify`, com debounce) emite `RepoChanged`.
7. **Um binário `gitmaster`** com subcomandos: abre o app por padrão; `gitmaster mcp` é o shim.
8. **Extensão por processo**, não por biblioteca dinâmica (ABI do Rust não é estável; segurança).

## Workflows (proposto, D52, D55, D57)

- Interpretador em `gm-core`: modelo puro (TOML para modelo tipado), avaliador de expressões e validador, sem I/O. Testável sem janela.
- Catálogo de ações em `gm-schema`, com JSON Schema da entrada e da saída de cada ação. O validador checa o workflow inteiro antes de rodar (D55).
- Cada passo despacha um `Command` existente. Workflow não tem caminho de lógica próprio.
- Cada execução é uma operação (`OperationId`), persistida em `state.db`.
- Eventos produzidos por uma execução carregam o id da execução e nunca disparam gatilhos (sem cascata).

## Persistência

- Arquivos de workflow: fora do repositório, em `~/.config/gitmaster/workflows/` (macOS: `Library/Application Support/gitmaster/workflows/`) e nos diretórios de `gitmaster.workflowPath`. Nada é lido nem escrito no working tree (D53).
- Estado habilitado por repositório e aprovações automáticas: diretório de dados do app, não no arquivo do workflow.
- Execuções de workflow: `state.db`.
- Pais de stack: git config do repositório.
- Operation log, comentários de review, aprovações: SQLite no diretório comum do Git (`git rev-parse --git-common-dir`), em `gitmaster/state.db`. Funciona com worktrees.
- Snapshots: refs privadas dentro de `.git`.
- O operation log é auditoria e base do undo (por snapshots de refs). **Não é event sourcing**: reconstruir estado por replay é armadilha, porque o Git muda por fora.

## Testes

- Propriedades (`proptest`): staged mais unstaged igual à mudança total; restack idempotente; plano invalidado quando uma ref muda.
- Determinismo: a mesma sequência de comandos sobre o mesmo repositório gera o mesmo operation log.
- Core testado com fakes das ports, sem GPUI (ciclos de teste rápidos).
- Propriedade de workflows: mesmo workflow + mesmo estado do repositório + mesmas entradas → mesmos comandos (D09, D58).

## Estrutura de crates (D27)

Começar com quatro: `gm-schema`, `gm-core` (com ports e fakes), `gm-ui`, `gm-mcp`. Extrair `gm-git`, `gm-hooks` e `gm-store` quando o core pesar. A separação core/ui vale desde o primeiro dia: o GPUI é pesado de compilar e o core precisa compilar e testar sem ele.

## Riscos técnicos

- **Texto editável e virtualizado no GPUI**: necessário para o painel de resultado do resolvedor e para o diff. Avaliar bibliotecas de terceiros e licenças (Q7).
- **rebase in-memory do libgit2**: confirmar que cobre o caso (Q6).
- **Status em monorepo**: validar subprocess com fsmonitor.
- **Layout de lanes do grafo**: testar com repositório grande real.
