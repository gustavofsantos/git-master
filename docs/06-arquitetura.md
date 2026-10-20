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

## Workflows (D60, D63, D65; WF0 a WF2 implementadas)

- Interpretador em `gm-core`: modelo puro (TOML para modelo tipado), avaliador de expressões e validador, sem I/O. Testável sem janela.
- Catálogo de ações em `gm-core` (`workflow/catalog.rs`): nível de efeito, outcomes, fase e os campos tipados de `with` (`FieldSpec`) de cada ação. O validador checa o workflow inteiro, inclusive `with`, antes de rodar (D63). O JSON Schema da entrada e da saída, previsto no D63, ainda não existe; os campos tipados cumprem esse papel no editor e no validador.
- Módulos em `gm-core/src/workflow/`: `model`, `parse`, `expr`, `catalog`, `validate` e `machine` (núcleo puro); `dry_run` (passeio pela função de transição sem efeitos, D85); `edit` (edições estruturais com `toml_edit`, D86); `templates`; `loader`, `store` (SQLite), `exec` e `driver` (I/O). A sessão por repositório fica em `gm-core/src/workflow_session.rs`.
- Em `gm-ui`: `workflows_page` (modelo puro da página: arquivo aberto, abas, seleção, edições, dry run, caminho do run), `flow_graph` (layout puro do grafo), `flow_graph_view` (desenho), `flow_inspector` (inspetor da aba Flow), `shell_workflows` (a página em GPUI), `workflow_run` e `workflow_inputs`.
- Cada passo despacha um `Command` existente. Workflow não tem caminho de lógica próprio.
- Cada execução é uma operação (`OperationId`), persistida em `state.db`.
- O interpretador é uma função de transição pura: `next(definição, run, evento) -> (transição, efeitos)`, sem I/O. O engine executa os efeitos e registra o resultado (D80).
- Cada transição é gravada em uma transação SQLite antes do efeito ("entrando em X") e depois dele ("X → desfecho"). Após um crash, a retomada lê `current_state`.
- Um `apply` interrompido recebe o desfecho `interrupted`, que vai para `end.failed` se não for tratado. Nunca é reexecutado às cegas (D80).
- Eventos produzidos por uma execução carregam o id da execução e nunca disparam gatilhos (sem cascata).
- A aba TOML e a aba Flow editam um só texto. A aba Flow edita esse texto com `toml_edit`, para preservar os comentários e a formatação do usuário, e cada edição é um só passo de desfazer no editor TOML (D86). Salvar grava o texto de forma atômica (D76).
- `exec` executa `argv` diretamente, nunca por shell (D74).
- `HookPort` continua até a migração em WF3 (D75).

## Persistência

- Arquivos de workflow: fora do repositório, em `~/.config/gitmaster/workflows/` (macOS: `Library/Application Support/gitmaster/workflows/`) e nos diretórios de `gitmaster.workflowPath`. Nada é lido nem escrito no working tree (D61). O arquivo guarda só a definição (D79): nada de execução, como `enabled`, última execução ou aprovações.
- Por repositório, em `state.db` (no diretório comum do Git): runs, transições, saídas, esperas e gatilhos pendentes. Acompanha o repositório e funciona com worktrees.
- Por usuário, em `~/.local/share/gitmaster/gitmaster.db` (diretório de dados do app): workflow habilitado em cada repositório, aprovações automáticas por hash da definição e cache das definições validadas.
- SQLite via `rusqlite` com SQLite embutido (bundled): compilado no binário, sem dependência externa.
- Retenção: 200 execuções por workflow e por repositório, podadas ao iniciar uma execução. Saídas maiores que 64 KiB são guardadas como hash, tamanho e os primeiros 4 KiB.
- Pais de stack: git config do repositório.
- Operation log, comentários de review, aprovações: SQLite no diretório comum do Git (`git rev-parse --git-common-dir`), em `gitmaster/state.db`. Funciona com worktrees.
- Snapshots: refs privadas dentro de `.git`.
- O operation log é auditoria e base do undo (por snapshots de refs). **Não é event sourcing**: reconstruir estado por replay é armadilha, porque o Git muda por fora.
- O log de execuções de um workflow é o registro que o próprio engine guarda da execução, não uma reconstrução do estado do repositório. O Git continua sendo a fonte da verdade, e cada efeito revalida fingerprints (D66).

## Testes

- Propriedades (`proptest`): staged mais unstaged igual à mudança total; restack idempotente; plano invalidado quando uma ref muda.
- Determinismo: a mesma sequência de comandos sobre o mesmo repositório gera o mesmo operation log.
- Core testado com fakes das ports, sem GPUI (ciclos de teste rápidos).
- Propriedade de workflows: mesmo workflow + mesmo estado do repositório + mesmas entradas → mesmos comandos (D09, D66).
- Propriedade: a função de transição é determinística e toda execução termina (D78).
- Teste de crash: matar o engine entre "entrando em X" e o desfecho, reiniciar e verificar o tratamento de `interrupted` (D80).

## Estrutura de crates (D27)

Começar com quatro: `gm-schema`, `gm-core` (com ports e fakes), `gm-ui`, `gm-mcp`. Extrair `gm-git`, `gm-hooks` e `gm-store` quando o core pesar. A separação core/ui vale desde o primeiro dia: o GPUI é pesado de compilar e o core precisa compilar e testar sem ele.

## Riscos técnicos

- **Texto editável e virtualizado no GPUI**: necessário para o painel de resultado do resolvedor e para o diff. Avaliar bibliotecas de terceiros e licenças (Q7).
- **rebase in-memory do libgit2**: confirmar que cobre o caso (Q6).
- **Status em monorepo**: validar subprocess com fsmonitor.
- **Layout de lanes do grafo**: testar com repositório grande real.
