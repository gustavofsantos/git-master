# MCP e review local

Última atualização: 2026-10-19. Status de cada decisão em `01`.

## Transporte (D02, D12)

- Produto local: **sem modo headless, CI ou servidor.**
- O servidor MCP vive dentro do app em execução. Para humano e agente enxergarem o mesmo estado, e para a aprovação acontecer na UI.
- `gitmaster mcp` é só um **shim stdio** que repassa para o app por socket Unix com token, e abre o app se estiver fechado.
- Preferir socket com token a HTTP em localhost (portas TCP são alcançáveis por qualquer processo local e por páginas web via DNS rebinding).

## Ferramentas por nível de efeito (D20)

```
Read  →  Propose  →  Aprovação (humano na UI)  →  Apply
```

| Nível | Ferramentas iniciais |
|---|---|
| Read | `repo_status`, `stack_tree`, `diff_stat`, `conflict_list`, `conflict_get`, `operation_log`, `workflow_list` |
| Propose | `restack_plan`, `reparent_plan`, `conflict_propose` |
| Apply | `restack_apply`, `conflict_resolve`, `commit_create`, `push`, `operation_undo`, `workflow_run` |

Os passos de apply de um workflow disparado por `workflow_run` sempre esperam o humano (D64(c), D71). Ver `10`.

- `apply` só aceita `plan_id`, nunca argumentos livres: o que o humano aprova é exatamente o que executa.
- Anotações do MCP (`readOnlyHint`, `destructiveHint`) em cada tool.
- Política no config, no estilo dos hooks: `[gitmaster "mcp"]` com `auto = read, propose` e `approve = apply`. Sem UI aberta, apply fica `pending_approval`.
- O agente passa pelos mesmos hooks e gates que o humano. O operation log registra `actor: agent:<cliente>`.
- Schemas compartilhados com os hooks (`gm-schema`): `RestackPlan` é o mesmo tipo no `pre-restack` e no `restack_plan`.

## Desenho das saídas

- Saídas pequenas por padrão: `diff_stat` primeiro, depois `diff_get(path, hunk)`.
- **IDs de hunk determinísticos** (hash do conteúdo, D21). Se o hunk mudou, o id muda e a chamada falha, em vez de aplicar no lugar errado.
- Menos de cerca de 12 tools, desenhadas por tarefa, não por comando do Git. Não duplicar o que o CLI do `git` já faz bem (`log`, `show`).
- **Prompt injection**: mensagens de commit, nomes de branch e conteúdo de arquivo vêm do repo e não são confiáveis. Devolvê-los em campos marcados como não confiáveis. Nenhum valor vindo do repo vira argumento de tool de apply sem passar por um plano.

## Exemplo: restack feito por um agente

```
stack_tree()                      -> ledger-base -> [api, ui -> [e2e]], 3 desatualizadas
restack_plan(root="ledger-base")  -> { plan_id: "01J9Z...", steps: 3, refs: {...SHAs...} }
restack_apply(plan_id)            -> { status: "pending_approval", operation_id }
                                     ... humano aprova na UI ...
                                  -> { status: "conflict", branch: "ledger-ui", hunks: ["h_7f3a", "h_c21b"] }
conflict_get("h_7f3a")            -> base, esquerda, direita
conflict_propose("h_7f3a", text)  -> sugestão aparece na UI; humano aceita
```

## Review local (D11, proposto)

Funcionalidade que o autor implementou em outro app. Fluxo: seleciona código e comenta; o agente lê os comentários via MCP, trabalha e resolve cada um com uma devolutiva; o que o agente fez fica rastreado na resolução, dando um retrato de antes e depois.

### Estados

`aberto` → `tratado` (marcado pelo agente) → `resolvido` (confirmado pelo humano). O agente propõe o fechamento e o humano efetiva.

### Âncora sem adivinhar

Ancorar por arquivo, texto selecionado e algumas linhas de contexto. Se o texto reaparece exatamente uma vez, o comentário segue. Se não, ou se há mais de uma ocorrência, o estado vira `desatualizado`. Nunca re-ancorar por aproximação.

### Antes e depois observado, não relatado

- O motor guarda um snapshot do working tree no momento do comentário (`git stash create` gera um commit sem tocar em refs nem no tree), numa ref privada dentro de `.git`.
- Outro snapshot é guardado quando o agente resolve.
- O "o que o agente fez" é o diff entre os dois snapshots, restrito à região do comentário. O texto do agente é só narrativa.
- Vários comentários tratados no mesmo passe compartilham o snapshot "depois": atribuir as mudanças a cada comentário pela sobreposição com a região âncora e mostrar o resto como "outras mudanças".

### MCP

- `review_comments_list(status)` (Read).
- `review_comment_resolve(id, summary)`: marca como `tratado`; o snapshot "depois" é tirado pelo motor, não informado pelo agente.

### Armazenamento

Fora do working tree: `.git/gitmaster/` ou diretório de dados do app (Q3).

### Integração com o diff

Comentário em linha dentro do diff, com seletor Antes/Depois mostrando o trecho da âncora. Ver `05`.
