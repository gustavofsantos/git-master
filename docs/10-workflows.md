# Workflows

Última atualização: 2026-10-19. Status de cada decisão em `01`. D73 e D74 estão decididas; D75 está decidida na direção, com detalhes propostos; D76 e D77 são propostas; o restante do modelo (D61–D72) segue proposto.

## Ideia central

Workflow é uma sequência de ações ligada a um gatilho: um gatilho, entradas opcionais e passos tipados, em ordem. Cada passo é um `Command` do motor (D08), o mesmo que a UI e o MCP disparam. Não há caminho de lógica novo: workflow só compõe `Command`s (D60).

Para stacking, a divisão é:

- **Primitivas** ficam no motor: modelo de stack, estados derivados, plano, aplicação e undo de restack, reparent, push com lease.
- **Políticas** ficam em workflows: quando restackar, qual subárvore, se faz push, o que notificar.

Até a migração em WF3, hooks (`03`) são pontos de extensão dentro de uma operação. A partir de D75, esses pontos viram fases de workflow (`before`, `transform`, `after`), e workflows orquestram entre operações.

Serve o pilar 1 (automações no cliente Git) e o pilar 3 (agente sob responsabilidade humana, com os mesmos níveis de aprovação e log observado).

## Formato e localização (D61, revisado por D73)

- Um workflow por arquivo TOML, com `schema = 1`.
- Lido do diretório de configuração do usuário: `~/.config/gitmaster/workflows/*.toml` (no macOS, `Library/Application Support/gitmaster/workflows/`) e dos diretórios em `gitmaster.workflowPath` no git config. A chave é multivalorada; `includeIf` ativa por árvore de diretórios.
- Nenhum workflow é lido de um repositório (D73). Um `gitmaster.workflowPath` que aponte para dentro da working tree do repositório aberto é ignorado, com aviso visível. A persona da v1 é um engenheiro sozinho, e a configuração não é compartilhada (Q15).
- Nada é lido nem escrito na working tree.
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

## Validação (D63, revisado por D77)

O catálogo de ações vive em `gm-schema`, com JSON Schema de entrada e saída de cada ação. Ao carregar, o motor verifica ação desconhecida, referência desconhecida ou adiante, tipo incompatível, entrada obrigatória ausente e forma inválida de `for_each`. Também verifica que `block` só aparece em fase `before` e `set` só em fase `transform` (D77). Workflow inválido nunca roda. O erro (arquivo, linha, mensagem) aparece na página Workflows, com o mesmo validador usado pelo editor (D76). O watcher recarrega o arquivo quando ele muda.

## Catálogo de ações v1 (cresce com as fatias)

| Ação | Nível | Fatia |
|---|---|---|
| `notify` | read | WF1 |
| `approve` | pausa para o humano | WF1 |
| `exec` | read (saída não confiável) | WF1 |
| `commit.suggest_message` | propose | WF1 |
| `stage.all`, `commit.create` | apply | WF1 |
| `branch.create`, `branch.checkout` | apply | WF1 |
| `block` | propose (só em `before`) | WF3 |
| `set` | propose (só em `transform`) | WF3 |
| `fetch` | apply | quando Fetch existir |
| `stack.tree`, `stack.stale` | read | WF4 |
| `restack.plan`, `reparent.plan` | propose | WF4 |
| `restack.apply`, `reparent.apply`, `restack.undo` | apply | WF4 |
| `push.lease` | apply, sempre pede | WF4 |

## LLM e scripts (D69, revisado por D74)

O app não tem LLM embutido: não há passo `llm`, não há HTTP e o app não guarda chave de API (Q16). Uma LLM é chamada por `exec`, que roda um programa de linha de comando que o usuário já tem, como um CLI de LLM ou um script próprio. O app continua autocontido: a única dependência obrigatória é o `git`.

Forma do passo:

```toml
[[step]]
id = "llm"
action = "exec"
with = { argv = ["llm-cli", "-p", "Write a conventional commit message for this diff. Reply with the message only."], stdin = "${repo.staged_diff}", timeout = "60s" }
```

Regras:

- `argv` é uma lista executada diretamente, **nunca por um shell**. Cada valor interpolado vira exatamente um elemento de `argv` e não é dividido de novo: conteúdo do repositório não injeta flags nem comandos.
- `stdin` é um template de texto opcional. Valores grandes, como o diff, vão por `stdin`, não por `argv`.
- Saída: `output.text` (stdout, sem espaços nas bordas) e `output.exit_code`. Com `format = "json"`, stdout precisa ser JSON e fica em `output.json`; é o contrato antigo de hooks, agora opcional.
- Código de saída diferente de zero falha o passo, e o stderr é mostrado. `timeout` mata o processo.
- A saída é não confiável (D64(e), D69, `04`, injeção de prompt). Ela alimenta livremente só passos `propose`, como `commit.suggest_message` e `set`, e o humano decide. Em passos `apply`, vale D64(e).

Mensagem de commit com um CLI de LLM, preenchida no compositor como sugestão:

```toml
schema = 1
name = "suggest-commit-message"
description = "Suggest a commit message from the staged diff"

[trigger]
on = "manual"
key = "cmd-shift-m"

[[step]]
id = "llm"
action = "exec"
with = { argv = ["llm-cli", "-p", "Write a conventional commit message for this diff. Reply with the message only."], stdin = "${repo.staged_diff}", timeout = "60s" }

[[step]]
action = "commit.suggest_message"
with = { message = "${steps.llm.output.text}" }
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
with = { argv = ["ticket-branch-name", "${inputs.ticket}"] }

[[step]]
action = "approve"
with = { summary = "Create branch ${steps.name.output.text}?" }

[[step]]
action = "branch.create"
with = { name = "${steps.name.output.text}", checkout = true }
```

## Hooks e workflows (D70, revisado por D75)

Workflows substituem os hooks do GitMaster: o modelo de workflow faz o papel dos hooks e mais. Os pontos de extensão viram fases de gatilho sobre uma operação:

- `before.<operation>` (gate): a operação espera. A espera é assíncrona e pendente, e nunca bloqueia a UI (D05). O run pode terminar com a ação `block` (`with = { reason = "..." }`) para vetar a operação. Falha do run numa fase `before` veta (fail-closed, como os gates de hoje).
- `transform.<field>` (transform): o último passo, `set`, devolve o novo valor do campo (por exemplo `transform.commit_message`). Vários workflows de transform rodam em sequência, na ordem do nome do arquivo. A reconciliação de `03` vale: se o usuário mudou o campo enquanto isso, o valor vira sugestão, e não sobrescreve o que ele escreveu.
- `after.<operation>` (observe): roda depois da operação. Falhas só geram aviso.

Operações iniciais: `commit`, `restack`, `branch_create` (`transform.branch_name`) e `conflict_resolve` (`transform.conflict_hunk`).

Gate que veta o restack quando a raiz está numa branch protegida:

```toml
schema = 1
name = "block-restack-on-protected"
description = "Block restack when the root is a protected branch"

[trigger]
on = "before.restack"

[[step]]
action = "block"
if = "${trigger.root} == 'main' or ${trigger.root} == 'release'"
with = { reason = "Restack on protected branch ${trigger.root} is blocked" }
```

Transform que reescreve a mensagem de commit com `exec` e `set`:

```toml
schema = 1
name = "commit-message-with-llm"
description = "Rewrite the commit message with an LLM command line"

[trigger]
on = "transform.commit_message"

[[step]]
id = "llm"
action = "exec"
with = { argv = ["llm-cli", "-p", "Write a conventional commit message for this diff. Reply with the message only."], stdin = "${repo.staged_diff}", timeout = "60s" }

[[step]]
action = "set"
with = { value = "${steps.llm.output.text}" }
```

Regras e pontos fixos:

- Hooks nativos do Git continuam com o Git (D18, D29), sem mudança.
- O id de correlação de D06 vira o id da execução. O protocolo de resposta por stdout de D07 vale dentro de `exec`.
- Os hooks `pre-restack` e `post-restack` (`02`) viram `before.restack` e `after.restack`.
- **Migração.** O hook `prepare-commit` da fatia 2 (em código, `gm-core/src/hook`) continua funcionando até que o workflow `transform.commit_message` alcance paridade, em WF3. Depois disso, o GitMaster deixa de ler as seções `[gitmaster "<hook>"]` do `.gitconfig`, e uma ação única, "Convert hooks to workflows", grava em um diretório de workflows do usuário os arquivos equivalentes.
- D17 (hooks de repositório exigem aprovação) fica sem efeito com D73 após WF3. D04, D16 e D30 são substituídas por D75 após WF3 (ver `01`).

## Gatilhos (D67, revisado por D75)

| Gatilho | Dispara quando |
|---|---|
| `manual` | o usuário escolhe o workflow na paleta (ou tecla opcional) |
| `repo.opened` | o repositório abre |
| `refs.changed` | o watcher vê mudança de ref (filtrável por branch) |
| `commit.created`, `checkout.done`, `fetch.done`, `restack.done` | a operação termina |
| `before.<operation>` | antes da operação; é gate e pode vetar com `block` (D75) |
| `transform.<field>` | ao montar o valor de um campo, como `transform.commit_message`; `set` devolve o valor (D75) |
| `after.<operation>` | depois da operação; é observação, e falhas só geram aviso (D75) |
| `stack.parent_moved` | derivado: a ponta do pai difere de `stackParentBase` (`02`) |
| `stack.parent_merged` | derivado: o pai sumiu ou já está na `main` (`02`) |
| `composer.opened` | o compositor de commit abre |

`match` usa a sintaxe de hooks (`branch:feat/*`, `03`). Gatilho por tempo não existe na v1 (Q17).

## Ator, níveis e aprovação (D64, proposto)

Cada execução tem ator `workflow:<name>` e registra quem a iniciou: `human`, `event:<name>` ou `agent:<client>`. Cada ação tem nível de efeito, como em D20: `read`, `propose` ou `apply`.

- **(a)** Execução manual pelo humano: passos `apply` rodam. O humano apertou Run e viu a lista de passos.
- **(b)** Execução por evento: passos `apply` pausam em `AwaitingApproval`, a não ser com `approval = "auto"`. A aprovação automática é concedida uma vez, na UI, e amarrada ao hash do arquivo. Qualquer edição a revoga.
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

## Interface (D72, revisado por D76) e MCP (D71)

A página Workflows (D76) permite ver, criar e editar workflows na UI. D72 dizia que não haveria editor visual na v1; D76 revisa isso. O desenho da tela fica em `05`.

Layout: lista à esquerda, com nome, gatilho, habilitado neste repositório, última execução e badge de erro. O botão "New workflow" cria em branco ou a partir de template. À direita fica o detalhe, com três abas: **Steps**, **TOML** e **Runs**.

- **Steps** (editor estruturado): seletor de gatilho (fase e operação ou evento, e o campo `match`); tabela de entradas (`inputs`); cartões de passo ordenados, com adicionar, remover, duplicar e reordenar (por arrastar e pelo teclado); seletor de ação do catálogo, agrupado por domínio e com badge de nível (read, propose, apply); campos tipados gerados do JSON Schema da ação; campos de expressão com completar para `inputs.*`, `trigger.*`, `repo.*` e só as saídas de passos anteriores (sem referência adiante, por construção); `if`, `approval`, `on_error`, e `for_each` como grupo aninhado de cartões. Validação inline enquanto se digita, com o mesmo validador do carregador (D63).
- **TOML**: o mesmo arquivo, como texto na área de edição do app. Edições em qualquer aba atualizam um só modelo. Trocar de aba com erro de parse mantém o usuário na aba TOML, com o erro visível.
- **Runs**: histórico de execuções, com os chips por passo de D72. Cada execução tem Cancelar e Aprovar.

**Dry run** avalia o gatilho com o estado atual do repositório, roda só passos `read` e mostra a lista de passos resolvida e o que cada passo `propose` ou `apply` receberia. Nada com efeito colateral roda. `exec` não roda em dry run: a página mostra o argv e o stdin que seriam enviados (Q21).

**Salvar** grava o arquivo no diretório de workflows do usuário, nunca em um repositório, de forma atômica (arquivo temporário e rename). A edição usa `toml_edit`, então comentários e formatação do usuário são preservados. Se o arquivo mudou no disco desde que foi aberto (hash do conteúdo), salvar para com "Changed on disk: Reload / Overwrite". Salvar muda o hash, então as aprovações automáticas daquele arquivo são revogadas (D64(b)), e a página avisa antes de salvar.

**Excluir** move o arquivo para a lixeira no diretório de dados do app, recuperável. Nunca usa `rm`.

**Teclado**: toda ação do registro de ações e da paleta tem atalho, como no resto do app (fatia 1c). Exemplos: `workflow.new`, `workflow.save`, `workflow.dry_run`, `workflow.run`, `workflow.add_step`.

MCP, com a fatia 7 (WF5): `workflow_list` (read) e `workflow_run(name, inputs)`, com passos `apply` seguindo D64(c).

## Questões abertas

Detalhes em `01`:

- **Q15** **Resolvida (2026-10-19), D73**: workflows versionados no repositório exigem aprovação por hash (como D17) ou nunca são lidos da working tree? Resposta: nunca. Persona v1 é um engenheiro sozinho, que não compartilha configuração. Workflows vêm só da configuração do usuário, e `gitmaster.workflowPath` dentro da working tree do repositório aberto é ignorado com aviso.
- **Q16** **Resolvida (2026-10-19), D74**: passo `llm` embutido (HTTP com a chave do usuário) ou só `exec`? Resposta: só `exec`, por linha de comando. Não há passo `llm` embutido, nem HTTP, nem chave de API guardada pelo app.
- **Q17**: gatilhos de tempo enquanto o app está aberto?
- **Q18** **Resolvida (2026-10-19), D75**: no longo prazo, hooks viram workflows ou ficam separados? Resposta: workflows substituem hooks, com as fases `before`, `transform` e `after`.
- **Q19**: subcomando `gitmaster workflow check <arquivo>` para validar em repositório de dotfiles, sem contrariar D02?
- **Q20**: posição no roadmap: fatia 3w após a 3 e antes da 3c, com as ações de stack (WF4) nas fatias 4 e 5, e o MCP (WF5) na fatia 7. Status em `07`.
- **Q21**: dry run de `exec`. Nunca roda, ou roda passos `exec` cujo argv o usuário marcou como `pure = true`? Padrão proposto: nunca.
