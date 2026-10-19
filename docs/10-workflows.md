# Workflows

Última atualização: 2026-10-19. Status de cada decisão em `01`. Tudo aqui é Proposto.

## Ideia central

Workflow é uma sequência de ações ligada a um gatilho: um gatilho, entradas opcionais e passos tipados, em ordem. Cada passo é um `Command` do motor (D08), o mesmo que a UI e o MCP disparam. Não há caminho de lógica novo: workflow só compõe `Command`s (D60).

Para stacking, a divisão é:

- **Primitivas** ficam no motor: modelo de stack, estados derivados, plano, aplicação e undo de restack, reparent, push com lease.
- **Políticas** ficam em workflows: quando restackar, qual subárvore, se faz push, o que notificar.

Hooks (`03`) são pontos de extensão dentro de uma operação. Workflows orquestram entre operações.

Serve o pilar 1 (automações no cliente Git) e o pilar 3 (agente sob responsabilidade humana, com os mesmos níveis de aprovação e log observado).

## Formato e localização (D61, proposto)

- Um workflow por arquivo TOML, com `schema = 1`.
- Lido de `~/.config/gitmaster/workflows/*.toml` (no macOS, `Library/Application Support/gitmaster/workflows/`) e dos diretórios em `gitmaster.workflowPath` no git config. A chave é multivalorada; `includeIf` ativa por árvore de diretórios, como em D17.
- Nada é lido nem escrito na working tree na v1.
- O estado habilitado ou desabilitado é por repositório e fica no diretório de dados do app. Os mesmos dotfiles servem em qualquer máquina.

```toml
schema = 1
name = "restack-children-on-parent-move"
description = "Restack a branch's children when the parent gets new commits"
concurrency = "replace"

[trigger]
on = "stack.parent_moved"
match = "branch:*"

[[step]]
id = "plan"
action = "restack.plan"
with = { root = "${trigger.parent}" }

[[step]]
id = "apply"
action = "restack.apply"
if = "${steps.plan.output.branch_count} > 0"
with = { plan_id = "${steps.plan.output.plan_id}" }
approval = "auto"

[[step]]
action = "notify"
with = { text = "Restacked ${steps.plan.output.branch_count} branches on ${trigger.parent}" }
```

Chaves de cada passo: `id` (opcional, obrigatório se outro passo lê a saída), `action`, `with`, `if`, `approval` (`"ask"` padrão ou `"auto"`, só em `apply`), `on_error` (`"stop"` padrão ou `"continue"`), e `for_each` + `as` + `steps`.

Chaves de topo (`name`, `description`, `concurrency`) vêm antes de qualquer tabela: em TOML, uma chave depois de `[trigger]` pertence ao gatilho.

Raízes de expressão: `inputs`, `trigger`, `repo` (snapshot somente leitura: `root`, `branch`, `head`, `staged_diff`, com caminho de arquivo quando grande, como nos hooks) e `steps.<id>.output`.

## Linguagem limitada (D62, proposto)

Não é linguagem de propósito geral:

- Sem loops, exceto `for_each` sobre lista finita produzida por passo anterior. Sem recursão. Sem variáveis além de `inputs`, payload do gatilho e saídas de passos.
- Expressões `${...}`: acesso por caminho, comparações, `and`, `or`, `not`, `len()`. Avaliador pequeno e puro, em Rust, em `gm-core`. Nenhuma linguagem de script embutida (Lua, JS); lógica arbitrária vai para `exec`.
- Eventos de uma execução carregam o id da execução e nunca disparam gatilhos. Sem cascata, sem loop infinito.

`for_each` deve ter esta forma:

```toml
schema = 1
name = "restack-stale-stacks-after-fetch"
description = "Restack every stale stack root after a fetch"
concurrency = "queue"

[trigger]
on = "fetch.done"

[[step]]
id = "roots"
action = "stack.stale"

[[step]]
id = "each"
for_each = "${steps.roots.output.roots}"
as = "root"
steps = [
  { id = "plan",  action = "restack.plan",  with = { root = "${root}" } },
  { id = "apply", action = "restack.apply", with = { plan_id = "${steps.plan.output.plan_id}" } },
]
```

Dentro do `for_each`, `steps.<id>` aponta primeiro para os passos da mesma iteração, e `${root}` é o item atual. Sem `approval = "auto"`, o `apply` de dentro do `for_each` espera o humano (D64, regra b).

## Validação (D63, proposto)

O catálogo de ações vive em `gm-schema`, com JSON Schema de entrada e saída de cada ação. Ao carregar, o motor verifica ação desconhecida, referência desconhecida ou adiante, tipo incompatível, entrada obrigatória ausente e forma inválida de `for_each`. Workflow inválido nunca roda. O erro (arquivo, linha, mensagem) aparece na página Workflows. O watcher recarrega o arquivo quando ele muda.

## Catálogo de ações v1 (cresce com as fatias)

| Ação | Nível | Fatia |
|---|---|---|
| `notify` | read | WF1 |
| `approve` | pausa para o humano | WF1 |
| `exec` | read (saída não confiável) | WF1 |
| `commit.suggest_message` | propose | WF1 |
| `stage.all`, `commit.create` | apply | WF1 |
| `branch.create`, `branch.checkout` | apply | WF1 |
| `fetch` | apply | quando Fetch existir |
| `stack.tree`, `stack.stale` | read | WF3 |
| `restack.plan`, `reparent.plan` | propose | WF3 |
| `restack.apply`, `reparent.apply`, `restack.undo` | apply | WF3 |
| `push.lease` | apply, sempre pede | WF3 |

Mensagem de commit por script, sem LLM embutido (D69):

```toml
schema = 1
name = "suggest-commit-message"

[trigger]
on = "manual"
key = "cmd-shift-m"

[[step]]
id = "llm"
action = "exec"
with = { command = "~/bin/suggest-commit-message", input = { staged_diff = "${repo.staged_diff}" }, timeout = "30s" }

[[step]]
action = "commit.suggest_message"
with = { message = "${steps.llm.output.message}" }
```

Criar branch a partir de um ticket, com entrada do usuário e aprovação explícita:

```toml
schema = 1
name = "branch-from-ticket"

[inputs]
ticket = { type = "string", prompt = "Ticket key" }

[trigger]
on = "manual"

[[step]]
id = "name"
action = "exec"
with = { command = "~/bin/ticket-branch-name", input = { ticket = "${inputs.ticket}" } }

[[step]]
action = "approve"
with = { summary = "Create branch ${steps.name.output.branch}?" }

[[step]]
action = "branch.create"
with = { name = "${steps.name.output.branch}", checkout = true }
```

## Gatilhos (D67, proposto)

| Gatilho | Dispara quando |
|---|---|
| `manual` | o usuário escolhe o workflow na paleta (ou tecla opcional) |
| `repo.opened` | o repositório abre |
| `refs.changed` | o watcher vê mudança de ref (filtrável por branch) |
| `commit.created`, `checkout.done`, `fetch.done`, `restack.done` | a operação termina |
| `stack.parent_moved` | derivado: a ponta do pai difere de `stackParentBase` (`02`) |
| `stack.parent_merged` | derivado: o pai sumiu ou já está na `main` (`02`) |
| `composer.opened` | o compositor de commit abre |

`match` usa a sintaxe de hooks (`branch:feat/*`, `03`). Gatilho por tempo não existe na v1 (Q17).

## Ator, níveis e aprovação (D64, proposto)

Cada execução tem ator `workflow:<name>` e registra quem a iniciou: `human`, `event:<name>` ou `agent:<client>`. Cada ação tem nível de efeito, como em D20: `read`, `propose` ou `apply`.

- **(a)** Execução manual pelo humano: passos `apply` rodam. O humano apertou Run e viu a lista de passos.
- **(b)** Execução por evento: passos `apply` pausam em `AwaitingApproval`, a não ser com `approval = "auto"`. A aprovação automática é concedida uma vez, na UI, e amarrada ao hash do arquivo, como em D17. Qualquer edição a revoga.
- **(c)** Execução pelo agente (MCP): passos `apply` sempre esperam o humano. Sem override.
- **(d)** `push.lease` sempre pede, em todos os modos (D15).
- **(e)** Passo `apply` cuja entrada depende, por referência estática, da saída de um `exec` sempre pede. Saída de `exec` não é determinística nem confiável.

O passo `approve` pausa com um resumo para o humano, em qualquer modo.

## Execução, concorrência e reconciliação (D65, D66, proposto)

Cada execução é uma operação (`OperationId`). Estados por passo: `Pending`, `Running`, `AwaitingApproval`, `Done`, `Failed`, `Skipped`, `Cancelled`, `Stale`.

- Roda fora da thread de UI e é cancelável. Cancelar mata os processos de `exec`.
- Persistida em `state.db`: sobrevive a fechar o app. Só passos com efeito vão ao command log (D28); a execução inteira vai ao operation log. Gatilhos de watcher têm debounce.

| `concurrency` | Comportamento |
|---|---|
| `queue` (padrão) | no máximo uma execução pendente; gatilhos se fundem |
| `replace` | cancela a execução em curso se ela ainda não chegou a um `apply`; senão, enfileira |
| `skip` | descarta o gatilho enquanto houver execução |

Reconciliação: cada passo registra o fingerprint do alvo (SHAs das refs, árvore do index, hash da mensagem), como os hooks (`03`). Se o alvo mudou desde o cálculo das entradas, o passo termina em `Stale` e a execução para. Planos mantêm `plan_id` amarrado aos SHAs (D14).

Determinismo (D09): mesmo workflow, mesmo estado e mesmas entradas geram os mesmos passos e os mesmos `Command`s.

## Stacks como workflows (D68, proposto)

O motor expõe as primitivas como `Command`s: `stack.tree`, `stack.stale`, `restack.plan`, `restack.apply`, `restack.undo`, `reparent.plan`, `reparent.apply`, `push.lease`. Emite `stack.parent_moved` e `stack.parent_merged`.

Atualização automática de stack não é código. Vem como três templates embutidos, desligados por padrão:

- "Restack children when parent moves"
- "Restack stale stacks after fetch"
- "Reparent to grandparent when parent is merged"

Um workflow que chama `restack.apply` mostra o mesmo fluxo interativo de `02`. Em execução automática, um conflito pausa só aquela execução, no estado de conflito e na subárvore afetada (D14), e notifica. Nunca resolve sozinho.

## LLM e scripts (D69, proposto)

O app não tem LLM embutido. Chamadas a LLM rodam em passos `exec`, scripts do usuário, com o contrato JSON de hooks (D30) e as regras de confiança de D17.

- A saída de `exec` é não confiável (`04`, injeção de prompt). Alimenta livremente só passos `propose`: `commit.suggest_message` preenche o compositor como sugestão, e o humano faz o commit.
- Em passos `apply`, vale D64(e).
- O app continua autocontido. A única dependência obrigatória é o `git`.

## Hooks e workflows (D70, proposto)

Hooks continuam dentro de uma operação. O passo `exec` reusa o contrato de processo de hooks de `gm-schema` (stdin com `schema_version` e `invocation_id`, stdout em JSON, timeout mata o processo). Não há migração de hooks para workflows na v1 (Q18).

## Interface (D72) e MCP (D71), proposto

Workflows são editados como texto, no editor do usuário. Sem editor visual na v1.

A página Workflows lista caminho do arquivo, gatilho, habilitado neste repositório, última execução e erro de validação. Ações: "Abrir no editor" e "Novo a partir de template" (copia para o diretório do usuário, nunca para o repositório). A view de execução tem um chip por passo, no estilo dos hooks, com Cancelar e Aprovar.

MCP, com a fatia 7 (WF4): `workflow_list` (read) e `workflow_run(name, inputs)`, com passos `apply` seguindo D64(c).

## Questões abertas

Detalhes em `01`:

- **Q15**: workflows versionados no repositório exigem aprovação por hash (como D17) ou nunca são lidos da working tree?
- **Q16**: passo `llm` embutido (HTTP com a chave do usuário) ou só `exec`?
- **Q17**: gatilhos de tempo enquanto o app está aberto?
- **Q18**: no longo prazo, hooks viram workflows ou ficam separados?
- **Q19**: subcomando `gitmaster workflow check <arquivo>` para validar em repositório de dotfiles, sem contrariar D02?
- **Q20**: posição no roadmap: fatia 3w após a 3 e antes da 3c, com as ações de stack (WF3) nas fatias 4 e 5. Status em `07`.
