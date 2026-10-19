# Workflows

Última atualização: 2026-10-19. Status de cada decisão em `01`. D73 e D74 estão decididas; D75 está decidida na direção, com detalhes propostos. D78 (workflow como máquina de estados limitada) e D79 (definição em texto, execução em SQLite) foram decididas em princípio em 2026-10-19, com detalhes propostos. D76, D77 e D80 são propostas. O restante do modelo (D61–D72) segue proposto.

## Ideia central

Workflow é uma máquina de estados explícita e limitada: tem um `start`, estados, pelo menos um fim (`end.done`, `end.failed`, `end.blocked`) e transições entre eles. Cada transição é disparada por um resultado (`outcome`) de uma ação. A definição é texto simples, em um arquivo TOML que pode ir para os dotfiles. A execução (runs, transições, saídas, esperas) vive em SQLite (D79). O que vale versionar é o workflow, não a execução.

Cada ação é um `Command` do motor (D08), o mesmo que a UI e o MCP disparam. Não há caminho de lógica novo: workflow só compõe `Command`s (D60).

Para stacking, a divisão é:

- **Primitivas** ficam no motor: modelo de stack, estados derivados, plano, aplicação e undo de restack, reparent, push com lease.
- **Políticas** ficam em workflows: quando restackar, qual subárvore, se faz push, o que notificar.

Até a migração em WF3, hooks (`03`) são pontos de extensão dentro de uma operação. A partir de D75, esses pontos viram fases de workflow (`before`, `transform`, `after`), e workflows orquestram entre operações.

Serve o pilar 1 (automações no cliente Git) e o pilar 3 (agente sob responsabilidade humana, com os mesmos níveis de aprovação e log observado).

## Formato e localização (D61, revisado por D73 e D79)

- Um workflow por arquivo TOML, com `schema = 1`.
- Lido do diretório de configuração do usuário: `~/.config/gitmaster/workflows/*.toml` (no macOS, `Library/Application Support/gitmaster/workflows/`) e dos diretórios em `gitmaster.workflowPath` no git config. A chave é multivalorada; `includeIf` ativa por árvore de diretórios.
- Nenhum workflow é lido de um repositório (D73). Um `gitmaster.workflowPath` que aponte para dentro da working tree do repositório aberto é ignorado, com aviso visível. A persona da v1 é um engenheiro sozinho, e a configuração não é compartilhada (Q15).
- Nada é lido nem escrito na working tree.
- O arquivo guarda só a definição (D79). Estado habilitado por repositório, aprovações automáticas e histórico ficam no SQLite do app (seção de execução). Os mesmos dotfiles servem em qualquer máquina.

```toml
schema = 1
name = "restack-children-on-parent-move"
description = "Restack a branch's children when the parent gets new commits"
concurrency = "replace"
start = "plan"

[trigger]
on = "stack.parent_moved"

[[state]]
id = "plan"
action = "restack.plan"
with = { root = "${trigger.parent}" }
on.done = [
  { if = "${states.plan.output.branch_count} == 0", to = "end.done" },
  { to = "apply" },
]

[[state]]
id = "apply"
action = "restack.apply"
with = { plan_id = "${states.plan.output.plan_id}" }
approval = "auto"
on.done = "notify"
on.stale = { to = "plan", limit = 3 }
on.conflict = "wait_for_resolution"

[[state]]
id = "wait_for_resolution"
wait = "restack.resumed"
timeout = "24h"
on.resumed = "notify"
on.aborted = "end.failed"
on.timeout = "end.failed"

[[state]]
id = "notify"
action = "notify"
with = { text = "Restacked ${states.plan.output.branch_count} branches on ${trigger.parent}" }
```

Chaves de topo (`schema`, `name`, `description`, `concurrency`, `start`) vêm antes de qualquer tabela: em TOML, uma chave depois de `[trigger]` pertence ao gatilho. `[trigger]` (com `on`, `match` ou `key`) e `[inputs]` são tabelas. Cada estado é um `[[state]]`.

Chaves de cada estado:

- `id`: obrigatório e único no workflow.
- Exatamente um tipo: `action`, `wait` ou `for_each` (ver Máquina de estados).
- `with`: entradas da ação.
- `approval`: `"ask"` (padrão) ou `"auto"`, só em ação `apply`.
- `timeout`: só em estado `wait`. Em `exec`, o timeout fica dentro de `with`.
- `on`: transições por outcome.
- `as` e `states`: só em `for_each`.

Não há `if` nem `on_error` no estado. A condição mora na transição, e o erro vira um outcome que a transição trata.

## Máquina de estados (D78)

Os estados são uma **lista ordenada** `[[state]]`, cada um com `id`. A ordem importa, porque a forma curta das transições segue a lista. Não se usa uma tabela por estado (`[state.apply]`): TOML diz que a ordem das tabelas não é significativa, e arrays de tabelas mantêm a ordem. `start` é opcional e vale o primeiro estado da lista.

Estados terminais: `end.done`, `end.failed` e `end.blocked`. `end.blocked` só é válido em fases `before.*` e vem acompanhado do `reason` da transição que o alcança (ver Hooks).

Tipos de estado:

- **Ação** (`action`, com `with`; e `approval` quando a ação é `apply`). Roda uma ação do catálogo.
- **Espera** (`wait = "<evento do motor>"`, com `timeout` opcional). Pausa a execução até o evento. A espera é persistida e sobrevive a reinício. Quando há `timeout`, ele produz o outcome `timeout`. Exemplo: `restack.resumed`, com outcomes `resumed | aborted`, mais `timeout`.
- **For each** (`for_each = "${...}"`, `as = "item"`, `states = [ ... ]`). Roda uma máquina aninhada uma vez por item, na ordem da lista. Outcomes `done | failed`. A profundidade é 1: não há `for_each` dentro de `for_each`.

**Outcomes.** O catálogo de ações declara o conjunto de outcomes de cada ação (em `gm-schema`; coluna Resultados do catálogo). Exemplos: `exec` → `done | failed | timeout`; `restack.apply` → `done | conflict | stale | failed`; `approve` → `approved | rejected`; `restack.plan` → `done | failed`; `notify` → `done`.

**Transições.** Ficam em `on.<outcome>`, de três formas:

- String: `on.done = "notify"` vai para o próximo estado ou para um terminal.
- Tabela: `on.stale = { to = "plan", limit = 3 }`. Campos: `to` (obrigatório), `if` (expressão, opcional), `limit` (quantas vezes essa aresta pode ser tomada por execução) e `reason` (texto, usado com `end.blocked` e `end.failed`).
- Lista: `on.done = [ { if = ..., to = ... }, { to = ... } ]`. É avaliada em ordem, e vale a primeira cujo `if` é verdadeiro e cujo `limit` não se esgotou.

**Forma curta.**

- Estado sem `on.done` vai para o próximo estado da lista. O último vai para `end.done`.
- `failed` sem transição vai para `end.failed`.
- Qualquer outro outcome declarado sem transição é erro de validação. Assim `conflict`, `stale`, `rejected` e `timeout` precisam ser tratados explicitamente.

**Sem transição.** Se nenhuma transição de uma lista vale na execução, a execução termina em `end.failed`, com motivo "no transition". O validador avisa quando uma lista não tem fallback sem `if`.

**Ciclos limitados.** Todo ciclo do grafo precisa ter ao menos uma aresta com `limit = N`, que é o máximo de vezes que aquela aresta é tomada numa execução. O validador rejeita ciclo sem limite. O motor tem um teto rígido de 1000 transições por execução. Assim o término é garantido. Isso substitui a regra de D62 de não ter laços, exceto `for_each`.

**Sem variáveis mutáveis.** A saída de cada estado fica em `states.<id>.output`, e vale a visita mais recente. Não há atribuição fora de `set`, que só existe em `transform`.

**Raízes de expressão.** `inputs` (entradas do usuário), `trigger` (payload do gatilho), `repo` (snapshot somente leitura: `root`, `branch`, `head`, `staged_diff`, com caminho de arquivo quando é grande, como nos hooks), `states.<id>.output` e o nome do item do `for_each` (`as`).

Exemplo de `for_each`, depois de `fetch`:

```toml
schema = 1
name = "restack-stale-stacks-after-fetch"
description = "Restack every stale stack root after a fetch"
concurrency = "queue"

[trigger]
on = "fetch.done"

[[state]]
id = "roots"
action = "stack.stale"

[[state]]
id = "each"
for_each = "${states.roots.output.roots}"
as = "root"
states = [
  { id = "plan", action = "restack.plan", with = { root = "${root}" } },
  { id = "apply", action = "restack.apply", with = { plan_id = "${states.plan.output.plan_id}" }, on.done = "end.done", on.stale = { to = "plan", limit = 3 }, on.conflict = "wait_conflict" },
  { id = "wait_conflict", wait = "restack.resumed", timeout = "24h", on.resumed = "end.done", on.aborted = "end.failed", on.timeout = "end.failed" },
]
```

Dentro do `for_each`, `states.<id>` aponta primeiro para os estados da mesma iteração. Sem `approval = "auto"`, o `apply` de dentro do `for_each` espera o humano (D64, regra b). Um conflito pausa só aquele item, até o evento `restack.resumed` (D14).

A execução de um workflow é uma máquina de estados como a operação do motor (`06`, regra 2). É a mesma forma em todo lugar.

## Linguagem de expressão (D62, revisado por D78)

Não é linguagem de propósito geral:

- Expressões `${...}` em `with`, `if`, `reason`, `for_each` e textos. Acesso por caminho (`states.plan.output.branch_count`), comparações, `and`, `or`, `not` e `len()`. Nos exemplos, cada caminho dentro de `if` vai entre `${...}`. O avaliador é pequeno e puro, em Rust, em `gm-core`. Não há linguagem de script embutida (Lua, JS). Lógica arbitrária vai para `exec`.
- Sem recursão. A repetição vem de `for_each` sobre lista finita produzida por estado anterior, ou de ciclo com `limit` (D78).
- Eventos de uma execução carregam o id da execução e nunca disparam gatilhos. Sem cascata.

## Validação (D63, revisado por D77 e D78)

O catálogo de ações vive em `gm-schema`, com JSON Schema de entrada e saída de cada ação e a lista de outcomes. Ao carregar, o motor verifica:

- ação desconhecida, estado desconhecido (em `start`, `to` ou `on`) ou referência desconhecida;
- referência a saída de estado que não está em todo caminho até o uso (aviso);
- tipo incompatível, entrada obrigatória ausente e forma inválida, como estado sem exatamente um tipo, `for_each` malformado ou `for_each` dentro de `for_each`;
- estado inalcançável a partir de `start`;
- estado sem caminho até um terminal `end.*`;
- outcome declarado sem transição e fora da forma curta (erro);
- ciclo sem aresta com `limit`;
- `end.blocked` fora de `before.*`;
- `set` fora de `transform.*`;
- lista de transições sem fallback sem `if` (aviso).

Workflow inválido nunca roda. O erro (arquivo, linha, mensagem) aparece na página Workflows, com o mesmo validador usado pelo editor (D76). O watcher recarrega o arquivo quando ele muda.

## Catálogo de ações v1 (cresce com as fatias)

| Ação | Nível | Resultados | Fatia |
|---|---|---|---|
| `noop` | read | `done` | WF0 |
| `notify` | read | `done` | WF1 |
| `approve` | pausa para o humano | `approved \| rejected` | WF1 |
| `exec` | read (saída não confiável) | `done \| failed \| timeout` | WF1 |
| `commit.suggest_message` | propose | `done \| failed` | WF1 |
| `stage.all`, `commit.create` | apply | `done \| failed \| interrupted` | WF1 |
| `branch.create`, `branch.checkout` | apply | `done \| failed \| interrupted` | WF1 |
| `set` | propose (só em `transform`) | `done` | WF3 |
| `fetch` | apply | `done \| failed \| interrupted` | quando Fetch existir |
| `stack.tree`, `stack.stale` | read | `done \| failed` | WF4 |
| `restack.plan`, `reparent.plan` | propose | `done \| failed` | WF4 |
| `restack.apply` | apply | `done \| conflict \| stale \| failed` | WF4 |
| `reparent.apply`, `restack.undo` | apply | `done \| failed \| interrupted` | WF4 |
| `push.lease` | apply, sempre pede | `done \| failed \| interrupted` | WF4 |
| espera `wait` | pausa até o evento | `resumed \| aborted` (+ `timeout`) | WF3 |
| `for_each` | agrupa a máquina aninhada | `done \| failed` | WF3 |

Notas:

- `noop` é read e tem só o outcome `done`. Existe para que um estado possa só ramificar, como no gate de exemplo.
- `block` foi removido (D77 revisada por D78). O veto agora é alcançar `end.blocked`.
- `set` existe só em `transform.*`.
- `interrupted` é o outcome de uma ação `apply` que estava em curso quando o motor parou, e que não tem recuperação própria (D80). Ações com recuperação própria, como o restack (D14), retomam pelo próprio fluxo e não produzem `interrupted`.

## LLM e scripts (D69, revisado por D74 e D78)

O app não tem LLM embutido: não há estado `llm`, não há HTTP e o app não guarda chave de API (Q16). Uma LLM é chamada por `exec`, que roda um programa de linha de comando que o usuário já tem, como um CLI de LLM ou um script próprio. O app continua autocontido: a única dependência obrigatória é o `git`.

Forma do estado:

```toml
[[state]]
id = "llm"
action = "exec"
with = { argv = ["llm-cli", "-p", "Write a conventional commit message for this diff. Reply with the message only."], stdin = "${repo.staged_diff}", timeout = "60s" }
on.timeout = "end.failed"
```

Regras:

- `argv` é uma lista executada diretamente, **nunca por um shell**. Cada valor interpolado vira exatamente um elemento de `argv` e não é dividido de novo: conteúdo do repositório não injeta flags nem comandos.
- `stdin` é um template de texto opcional. Valores grandes, como o diff, vão por `stdin`, não por `argv`.
- Saída: `output.text` (stdout, sem espaços nas bordas) e `output.exit_code`. Com `format = "json"`, stdout precisa ser JSON e fica em `output.json`; é o contrato antigo de hooks, agora opcional.
- Código de saída diferente de zero produz o outcome `failed`, e o stderr é mostrado. `timeout` mata o processo e produz `timeout`.
- A saída é não confiável (D64(e), D69, `04`, injeção de prompt). Ela alimenta livremente só ações `propose`, como `commit.suggest_message` e `set`, e o humano decide. Em ações `apply`, vale D64(e).

Mensagem de commit com um CLI de LLM, preenchida no compositor como sugestão, com nova tentativa limitada quando a saída vem vazia:

```toml
schema = 1
name = "suggest-commit-message"
description = "Suggest a commit message from the staged diff"

[trigger]
on = "manual"
key = "cmd-shift-m"

[[state]]
id = "ask"
action = "exec"
with = { argv = ["llm-cli", "-p", "Write a conventional commit message for this diff. Reply with the message only."], stdin = "${repo.staged_diff}", timeout = "60s" }
on.done = [
  { if = "len(${states.ask.output.text}) == 0", to = "ask", limit = 2 },
  { to = "suggest" },
]
on.timeout = "end.failed"

[[state]]
id = "suggest"
action = "commit.suggest_message"
with = { message = "${states.ask.output.text}" }
```

Criar branch a partir de um ticket, com entrada do usuário e aprovação explícita. O `approve` trata os dois outcomes: aprovado segue para criar a branch, e rejeitado termina sem efeito:

```toml
schema = 1
name = "branch-from-ticket"

[inputs]
ticket = { type = "string", prompt = "Ticket key" }

[trigger]
on = "manual"

[[state]]
id = "name"
action = "exec"
with = { argv = ["ticket-branch-name", "${inputs.ticket}"] }
on.timeout = "end.failed"

[[state]]
id = "confirm"
action = "approve"
with = { summary = "Create branch ${states.name.output.text}?" }
on.approved = "create"
on.rejected = "end.done"

[[state]]
id = "create"
action = "branch.create"
with = { name = "${states.name.output.text}", checkout = true }
```

## Hooks e workflows (D70, revisado por D75 e D78)

Workflows substituem os hooks do GitMaster: o modelo de workflow faz o papel dos hooks e mais. Os pontos de extensão viram fases de gatilho sobre uma operação:

- `before.<operation>` (gate): a operação espera. A espera é assíncrona e pendente, e nunca bloqueia a UI (D05). O veto acontece quando o run alcança `end.blocked`, e o `reason` da transição vira a mensagem. Um run de fase `before` que termina em `end.failed` também veta (fail-closed, como os gates de hoje).
- `transform.<field>` (transform): o valor devolvido é o da última ação `set` executada antes de `end.done` (por exemplo `transform.commit_message`). Vários workflows de transform rodam em sequência, na ordem do nome do arquivo. A reconciliação de `03` vale: se o usuário mudou o campo enquanto isso, o valor vira sugestão, e não sobrescreve o que ele escreveu.
- `after.<operation>` (observe): roda depois da operação. Falhas só geram aviso.

Operações iniciais: `commit`, `restack`, `branch_create` (`transform.branch_name`) e `conflict_resolve` (`transform.conflict_hunk`).

Gate que veta o restack quando a raiz está numa branch protegida. O estado `check` usa `noop` para só ramificar, e a transição para `end.blocked` carrega o motivo:

```toml
schema = 1
name = "block-restack-on-protected"
description = "Block restack when the root is a protected branch"

[trigger]
on = "before.restack"

[[state]]
id = "check"
action = "noop"
on.done = [
  { if = "${trigger.root} == 'main' or ${trigger.root} == 'release'", to = "end.blocked", reason = "Restack on ${trigger.root} is blocked" },
  { to = "end.done" },
]
```

Transform que reescreve a mensagem de commit com `exec` e `set`. O último estado é `set`, e seu valor é o retorno da fase:

```toml
schema = 1
name = "commit-message-with-llm"
description = "Rewrite the commit message with an LLM command line"

[trigger]
on = "transform.commit_message"

[[state]]
id = "llm"
action = "exec"
with = { argv = ["llm-cli", "-p", "Write a conventional commit message for this diff. Reply with the message only."], stdin = "${repo.staged_diff}", timeout = "60s" }
on.timeout = "end.failed"

[[state]]
id = "out"
action = "set"
with = { value = "${states.llm.output.text}" }
```

Regras e pontos fixos:

- Hooks nativos do Git continuam com o Git (D18, D29), sem mudança.
- O id de correlação de D06 vira o id da execução. O protocolo de resposta por stdout de D07 vale dentro de `exec`.
- Os hooks `pre-restack` e `post-restack` (`02`) viram `before.restack` e `after.restack`.
- **Migração.** O hook `prepare-commit` da fatia 2 (em código, `gm-core/src/hook`) continua funcionando até que o workflow `transform.commit_message` alcance paridade, em WF3. Depois disso, o GitMaster deixa de ler as seções `[gitmaster "<hook>"]` do `.gitconfig`, e uma ação única, "Convert hooks to workflows", grava em um diretório de workflows do usuário os arquivos equivalentes.
- D17 (hooks de repositório exigem aprovação) fica sem efeito com D73 após WF3. D04, D16 e D30 são substituídas por D75 após WF3 (ver `01`).

## Gatilhos (D67, revisado por D75 e D78)

| Gatilho | Dispara quando |
|---|---|
| `manual` | o usuário escolhe o workflow na paleta (ou tecla opcional) |
| `repo.opened` | o repositório abre |
| `refs.changed` | o watcher vê mudança de ref (filtrável por branch) |
| `commit.created`, `checkout.done`, `fetch.done`, `restack.done` | a operação termina |
| `before.<operation>` | antes da operação; é gate, e veta ao alcançar `end.blocked` (D75, D78) |
| `transform.<field>` | ao montar o valor de um campo, como `transform.commit_message`; devolve o valor do último `set` (D75, D78) |
| `after.<operation>` | depois da operação; é observação, e falhas só geram aviso (D75) |
| `stack.parent_moved` | derivado: a ponta do pai difere de `stackParentBase` (`02`) |
| `stack.parent_merged` | derivado: o pai sumiu ou já está na `main` (`02`) |
| `composer.opened` | o compositor de commit abre |

`match` usa a sintaxe de hooks (`branch:feat/*`, `03`). Gatilho por tempo não existe na v1 (Q17, resolvida).

## Ator, níveis e aprovação (D64, proposto)

Cada execução tem ator `workflow:<name>` e registra quem a iniciou: `human`, `event:<name>` ou `agent:<client>`. Cada ação tem nível de efeito, como em D20: `read`, `propose` ou `apply`.

- **(a)** Execução manual pelo humano: ações `apply` rodam. O humano apertou Run e viu a lista de estados.
- **(b)** Execução por evento: ações `apply` pausam em `AwaitingApproval`, a não ser com `approval = "auto"`. A aprovação automática é concedida uma vez, na UI, e amarrada ao `definition_hash` do arquivo. Qualquer edição muda o hash e a aprovação deixa de valer (D79).
- **(c)** Execução pelo agente (MCP): ações `apply` sempre esperam o humano. Sem override.
- **(d)** `push.lease` sempre pede, em todos os modos (D15).
- **(e)** Ação `apply` cuja entrada depende, por referência estática, da saída de um `exec` (`states.<id>.output` de um estado `exec`) sempre pede. Saída de `exec` não é determinística nem confiável.

A ação `approve` pausa com um resumo para o humano, em qualquer modo, e seus outcomes são `approved` e `rejected`.

## Execução em SQLite (D79, D80, proposto)

### Separação

O arquivo TOML guarda **só a definição**. Nenhum campo de execução (sem `enabled`, sem última execução, sem aprovações) entra no arquivo. Ele é o que vale a pena manter nos dotfiles.

Tudo o que é de execução fica no SQLite, com `rusqlite` e SQLite embutido, compilado no binário, para manter o app autocontido. Há dois lugares, por escopo:

- **Por repositório**: o `state.db` que já existe no diretório comum do Git (`.git/gitmaster/state.db`, `06`), ao lado do log de operações. Ele acompanha o repositório e funciona com worktrees. Guarda execuções, transições, saídas, esperas e gatilhos pendentes.
- **Por usuário**: `~/.local/share/gitmaster/gitmaster.db` (diretório de dados do app). Guarda qual workflow está habilitado em qual repositório, as aprovações automáticas por hash de definição e o cache de definições validadas.

### Tabelas

Esboço, com nomes em inglês. Pode mudar na implementação de WF1.

```text
workflow_run(id, workflow_name, definition_hash, definition_text, trigger_json,
             started_by, status, current_state, started_at, ended_at, end_reason)
workflow_transition(run_id, seq, from_state, outcome, to_state, at)   -- append-only
workflow_state_output(run_id, state_id, visit, output_json, fingerprints_json,
                      started_at, ended_at)
workflow_wait(run_id, state_id, event, deadline)
workflow_pending(workflow_name, trigger_json, queued_at)              -- unique per workflow

-- por usuário
workflow_enabled(repo_common_dir, workflow_name)
workflow_approval(definition_hash, state_id, granted_at)
```

### O que isso facilita (D80)

1. **O interpretador é uma função de transição pura**: `next(definition, run, event) -> (transition, effects)`. Não faz I/O e pode ser testada por propriedade: mesma definição, mesma execução e mesmo evento dão a mesma transição (D09). O motor executa os efeitos (despachar um `Command`, iniciar uma espera) e grava o resultado. Boa parte da WF0 é essa função.
2. **Segurança contra falhas, sem custo extra**: cada transição é gravada em uma transação SQLite, *antes* de executar o efeito do próximo estado ("entrando em X") e *depois* ("X → outcome"). Retomar após uma queda é ler `current_state`. Uma ação `read` ou `propose` interrompida roda de novo. Uma ação `apply` interrompida nunca roda de novo às cegas: recebe o outcome `interrupted`, que vai para `end.failed` a não ser que seja tratado. Ações com recuperação própria, como o restack (D14), retomam por ali.
3. **A execução roda a versão com que começou**: `definition_text` é gravado no início. Editar o arquivo durante a execução não muda a execução em curso. A aba Runs mostra exatamente o que rodou (observado, não relatado).
4. **Aprovações e habilitação não precisam escrever no arquivo**: a aprovação automática é uma linha indexada por `definition_hash`. Qualquer edição muda o hash, e a aprovação simplesmente deixa de bater (D64(b), sem contabilidade extra).
5. **Concorrência em uma transação**: `queue`, `replace` e `skip` (D65) viram uma chave única de execução ativa por workflow, junto da linha em `workflow_pending`. Não há trava em memória entre threads.
6. **Esperas são linhas**: um evento do motor é casado com `workflow_wait` em uma consulta. O prazo de `timeout` é a coluna `deadline`, verificada pelo motor enquanto o app roda. Isso não é gatilho por tempo: Q17 continua fechada.
7. **Histórico e aba Flow são consultas**: o caminho que a execução percorreu são as suas transições, e a UI o destaca no grafo. Retenção: as últimas 200 execuções por workflow e por repositório (configurável depois, em Settings), podadas no início de cada execução. Saídas maiores que 64 KiB ficam como hash, tamanho e os primeiros 4 KiB (diffs e textos de LLM não são guardados inteiros). Apagar um workflow mantém as execuções até a poda.
8. **Dry run continua somente leitura**: roda a função pura contra um SQLite em memória, descartável, sem efeitos para `propose`, `apply` e `exec` (D76, Q21).

Ressalva a registrar: isto é um **log da execução**, mantido pelo motor. Não é event sourcing do estado do repositório (`06` alerta contra isso, e continua valendo). Git continua sendo a fonte da verdade, e todo efeito revalida os fingerprints antes de agir (D66).

### Estados da execução e de cada estado

Cada execução é uma operação (`OperationId`). Estados por estado de workflow, como antes: `Pending`, `Running`, `AwaitingApproval`, `Done`, `Failed`, `Skipped`, `Cancelled`, `Stale`. Uma espera aparece como `Waiting` e tem uma linha em `workflow_wait`.

- Roda fora da thread de UI e é cancelável. Cancelar mata os processos de `exec`.
- Persistida em `state.db`: sobrevive a fechar o app. Só ações com efeito vão ao command log (D28); a execução inteira vai ao operation log. Gatilhos de watcher têm debounce.

### Concorrência

| `concurrency` | Comportamento |
|---|---|
| `queue` (padrão) | no máximo uma execução pendente; gatilhos se fundem na mesma linha de `workflow_pending` |
| `replace` | cancela a execução em curso se ela ainda não chegou a uma ação `apply`; senão, enfileira |
| `skip` | descarta o gatilho enquanto houver execução ativa |

A exclusão vem da chave única de execução ativa por workflow, na mesma transação que cria a execução.

### Reconciliação (D66)

Cada ação registra em `workflow_state_output` o fingerprint do alvo (SHAs das refs, árvore do index, hash da mensagem), como os hooks (`03`). Se o alvo mudou desde o cálculo das entradas, o estado termina em `Stale`, e a execução para pelo outcome `stale`, se ele não for tratado. Planos mantêm `plan_id` amarrado aos SHAs (D14).

Determinismo (D09): mesmo workflow, mesmo estado e mesmas entradas geram as mesmas transições e os mesmos `Command`s.

## Stacks como workflows (D68, proposto)

O motor expõe as primitivas como `Command`s: `stack.tree`, `stack.stale`, `restack.plan`, `restack.apply`, `restack.undo`, `reparent.plan`, `reparent.apply`, `push.lease`. Emite `stack.parent_moved` e `stack.parent_merged`.

Atualização automática de stack não é código. Vem como três templates embutidos, desligados por padrão:

- "Restack children when parent moves"
- "Restack stale stacks after fetch"
- "Reparent to grandparent when parent is merged"

Um workflow que chama `restack.apply` mostra o mesmo fluxo interativo de `02`. Em execução automática, um conflito leva a execução a um estado de espera (`wait`) só para aquela execução e para a subárvore afetada (D14), e notifica. Nunca resolve sozinho.

## Interface (D72, revisado por D76 e D79) e MCP (D71)

A página Workflows (D76) permite ver, criar e editar workflows na UI. D72 dizia que não haveria editor visual na v1; D76 revisa isso. O desenho da tela fica em `05`.

Layout: lista à esquerda, com nome, gatilho, habilitado neste repositório, última execução e badge de erro. O botão "New workflow" cria em branco ou a partir de template. À direita fica o detalhe, com três abas: **Flow**, **TOML** e **Runs**.

- **Flow** (editor estruturado): mostra o grafo de estados e, ao lado, a lista ordenada dos mesmos estados para edição pelo teclado. Nós são estados, com a ação e um badge de efeito. Arestas são outcomes, e o rótulo mostra `if` e `limit`. Dá para adicionar um estado e ligar um outcome a outro estado ou a um terminal, por um seletor. Erros do validador aparecem no nó ou na aresta correspondente. Outcome não tratado aparece como aresta pendurada em vermelho. Seletor de gatilho (fase e operação ou evento, e o campo `match`); tabela de entradas (`inputs`); seletor de ação do catálogo, agrupado por domínio e com badge de nível (read, propose, apply); campos tipados gerados do JSON Schema da ação; campos de expressão com completar para `inputs.*`, `trigger.*`, `repo.*` e só as saídas de estados anteriores (sem referência adiante, por construção). Validação inline enquanto se digita, com o mesmo validador do carregador (D63).
- **TOML**: o mesmo arquivo, como texto na área de edição do app. Edições em qualquer aba atualizam um só modelo. Trocar de aba com erro de parse mantém o usuário na aba TOML, com o erro visível.
- **Runs**: lista de execuções, lida do SQLite. Selecionar uma destaca o caminho percorrido no grafo da aba Flow, e mostra a saída e os fingerprints de cada estado. Uma execução em espera mostra "Waiting for <event>" e o botão Cancelar. Uma execução parada em `approve`, ou em `apply` sem aprovação automática, mostra Aprovar. Toda execução tem Cancelar.

**Dry run** avalia o gatilho com o estado atual do repositório, executa os estados `read`, e mostra a lista de estados resolvida com o que cada ação `propose` ou `apply` receberia. Nada com efeito colateral roda. `exec` não roda em dry run: a página mostra o argv e o stdin que seriam enviados (Q21).

**Salvar** grava o arquivo no diretório de workflows do usuário, nunca em um repositório, de forma atômica (arquivo temporário e rename). A edição usa `toml_edit`, então comentários e formatação do usuário são preservados. Se o arquivo mudou no disco desde que foi aberto (hash do conteúdo), salvar para com "Changed on disk: Reload / Overwrite". Salvar muda o hash, então as aprovações automáticas daquele arquivo são revogadas (D64(b)), e a página avisa antes de salvar.

**Excluir** move o arquivo para a lixeira no diretório de dados do app, recuperável. Nunca usa `rm`. As execuções já feitas ficam no SQLite até a poda (D79).

**Teclado**: toda ação do registro de ações e da paleta tem atalho, como no resto do app (fatia 1c). Exemplos: `workflow.new`, `workflow.save`, `workflow.dry_run`, `workflow.run`, `workflow.add_state`.

MCP, com a fatia 7 (WF5): `workflow_list` (read) e `workflow_run(name, inputs)`, com ações `apply` seguindo D64(c).

## Questões (todas resolvidas em 2026-10-19)

Detalhes em `01`:

- **Q15** **Resolvida (2026-10-19), D73**: workflows versionados no repositório exigem aprovação por hash (como D17) ou nunca são lidos da working tree? Resposta: nunca. Persona v1 é um engenheiro sozinho, que não compartilha configuração. Workflows vêm só da configuração do usuário, e `gitmaster.workflowPath` dentro da working tree do repositório aberto é ignorado com aviso.
- **Q16** **Resolvida (2026-10-19), D74**: estado `llm` embutido (HTTP com a chave do usuário) ou só `exec`? Resposta: só `exec`, por linha de comando. Não há estado `llm` embutido, nem HTTP, nem chave de API guardada pelo app.
- **Q17** **Resolvida (2026-10-19)**: sem gatilhos de tempo na v1.
- **Q18** **Resolvida (2026-10-19), D75**: no longo prazo, hooks viram workflows ou ficam separados? Resposta: workflows substituem hooks, com as fases `before`, `transform` e `after`.
- **Q19** **Resolvida (2026-10-19)**: sem subcomando de validação na primeira versão; a validação fica só na UI (D76).
- **Q20** **Resolvida (2026-10-19)**: fatia 3w logo depois da fatia 3, antes da 3c; ações de stack (WF4) com as fatias 4 e 5; MCP (WF5) com a fatia 7.
- **Q21** **Resolvida (2026-10-19)**: `exec` nunca roda no dry run.
