# Workflows em script

Última atualização: 2026-10-22. Status de cada decisão em `01`. D93 a D97 foram decididas pelo autor em 2026-10-22, numa sessão de planejamento; os detalhes marcados como "a definir" dependem do spike S-D ou da fatia que os implementa. Este documento substitui, para o que ainda não foi construído, as seções de TOML e de máquina de estados de `10` (formato, máquina de estados, linguagem de expressão e validação). O restante de `10` (gatilhos, fases, ator e níveis, concorrência, reconciliação, stacks como workflows, MCP) continua valendo, com as mudanças listadas aqui.

## Ideia central (D93)

Um workflow é um **script Lua** em um arquivo de texto, executado dentro de um sandbox embutido no binário. O script recebe um objeto `ctx`, que é a única maneira de ele agir, e um valor `input`, que são só dados.

A ideia vem de sistemas de execução durável como o Temporal: o fluxo é código em uma linguagem de programação, e as ações com efeito passam por uma fronteira controlada pelo host. A diferença para o Temporal é a persistência: aqui a execução é efêmera (D95).

Por que trocar a máquina de estados de D78:

- Ela vinha acumulando uma linguagem de programação: avaliador de expressões, `limit`, `for_each`, `wait`, `set`, e um validador grande para conter tudo isso.
- Tarefas pequenas (ler o JSON de um LLM, aparar uma chave de ticket, percorrer branches) são difíceis em `${...}` e simples em Lua.
- A regra antiga "sem linguagem de script, a lógica vai para `exec`" empurrava lógica para programas externos que o usuário teria de instalar. Isso fere "autocontido" mais do que uma VM embutida.

O que não muda: o workflow só compõe `Command`s do motor (D60), as ações têm nível `read`, `propose` ou `apply` (D64), e o que o workflow fez é observado pelo host, não relatado pelo script.

## Formato e local (D93, revisa D61)

- Um workflow por arquivo `.lua`, em `~/.config/gitmaster/workflows/` (macOS: `Library/Application Support/gitmaster/workflows/`) e nos diretórios de `gitmaster.workflowPath`. Nada é lido de um repositório e nada é escrito na working tree (D73 continua valendo).
- Diretórios que começam com ponto (como `.api/`, abaixo) não são lidos como workflows.
- O módulo devolve uma tabela com `meta` e `execute`:

```lua
local M = {}

M.meta = {
  name = "branch-from-ticket",
  description = "Create a branch from a ticket key",
  trigger = { on = "manual" },
  concurrency = "queue",
  inputs = { ticket = { type = "string", prompt = "Ticket key" } },
}

function M.execute(ctx, input)
  local r = ctx.exec({ "ticket-branch-name", input.inputs.ticket }, { timeout = "30s" })
  if ctx.approve("Create branch " .. r.text .. "?") then
    ctx.branch.create({ name = r.text, checkout = true })
  end
end

return M
```

- `M.meta` substitui o cabeçalho do TOML: `name`, `description`, `trigger` (`on`, `match`, `key`), `concurrency` e `inputs`. É lido em um sandbox sem `ctx`, ao carregar, para listar o workflow e seus gatilhos sem rodar lógica nenhuma.
- `input` reúne `inputs` (valores do usuário), `trigger` (payload do gatilho) e `repo` (snapshot somente leitura: `root`, `branch`, `head`, `staged_diff`; um valor grande vem como caminho de arquivo, como nos hooks).
- O arquivo guarda só a definição. Habilitação por repositório e aprovações ficam fora dele, como em D79.

### O que `execute` devolve, por fase

| Fase | Contrato |
|---|---|
| `manual` e eventos | O retorno é ignorado. Terminar sem erro é `done`. |
| `before.<operação>` (gate) | `ctx.block(reason)` termina o run como `blocked` e veta a operação. Um erro também veta (fail-closed). |
| `transform.<campo>` | O valor devolvido é o do campo (`nil` = sem sugestão). A reconciliação de `03` vale: se o usuário mudou o campo, o valor vira sugestão. A ação `set` deixa de existir. |
| `after.<operação>` | Observação. Falhas só geram aviso. |

`ctx.fail(reason)` termina o run como `failed` com um motivo.

## A superfície de `ctx` (D93, D94)

`ctx` é gerado a partir do catálogo de ações (`catalog.rs`), então cada ação do catálogo vira uma função com a mesma assinatura tipada, o mesmo nível e os mesmos outcomes. Nomes: `ctx.<domínio>.<verbo>`, regulares e previsíveis, porque agentes vão escrever scripts (D97).

| Chamada | Ação do catálogo | Nível |
|---|---|---|
| `ctx.notify(text)` | `notify` | read |
| `ctx.approve(summary)` → `true`/`false` | `approve` | pausa para o humano |
| `ctx.exec(argv, opts)` | `exec` | read, saída não confiável |
| `ctx.commit.suggest_message(msg)`, `ctx.stage.all()`, `ctx.commit.create(...)`, `ctx.branch.create(...)`, `ctx.branch.checkout(...)` | idem | propose ou apply |
| `ctx.wait(event, { timeout = "24h" })` → `resumed`/`aborted`/`timeout` | `wait` | pausa até o evento |
| `ctx.stack.*`, `ctx.restack.*`, `ctx.reparent.*`, `ctx.push.lease(...)`, `ctx.fetch()` | WF4 e quando houver | conforme o catálogo |
| `ctx.now()`, `ctx.random()` | n/a | determinísticos para o journal |
| `ctx.block(reason)`, `ctx.fail(reason)` | fim do run | n/a |
| `ctx.try(fn)` | n/a | devolve o outcome em vez de levantar erro |

Regras:

- **Outcome diferente de `done` levanta erro por padrão** (fail-closed). Um `conflict`, `stale` ou `rejected` não passa em silêncio, como na regra "outcome sem transição é erro" de D78. `ctx.try(function() return ctx.restack.apply(...) end)` devolve `{ outcome = "conflict", ... }` para o script tratar.
- Chamada a um nome desconhecido de `ctx` falha com a lista dos nomes válidos naquele nível, para que um agente se corrija pela mensagem (D97).
- Cada chamada de `ctx` vira um `Command` e é uma entrada do journal. Quem decide aprovação é o host (D96), não o script.
- `exec` continua sem shell: `argv` é uma lista, cada elemento é um elemento (D74).
- O `for_each` do TOML vira o `for` de Lua; o ciclo com `limit` vira `while`. O limite de término vem dos orçamentos (D94).

### Exemplos

Gate:

```lua
local M = {}
M.meta = { name = "block-restack-on-protected", trigger = { on = "before.restack" } }

function M.execute(ctx, input)
  local root = input.trigger.root
  if root == "main" or root == "release" then
    ctx.block("Restack on " .. root .. " is blocked")
  end
end
return M
```

Transform, com um CLI de LLM e nova tentativa:

```lua
local M = {}
M.meta = { name = "commit-message-with-llm", trigger = { on = "transform.commit_message" } }

function M.execute(ctx, input)
  for _ = 1, 3 do
    local r = ctx.exec({ "llm-cli", "-p", "Write a conventional commit message for this diff. Reply with the message only." },
                       { stdin = input.repo.staged_diff, timeout = "60s" })
    if #r.text > 0 then return r.text end
  end
end
return M
```

Restack com espera de conflito:

```lua
local M = {}
M.meta = { name = "restack-children-on-parent-move", trigger = { on = "stack.parent_moved" }, concurrency = "replace" }

function M.execute(ctx, input)
  local plan = ctx.restack.plan({ root = input.trigger.parent })
  if plan.branch_count == 0 then return end
  local r = ctx.try(function() return ctx.restack.apply({ plan_id = plan.plan_id }) end)
  if r.outcome == "conflict" then
    local w = ctx.wait("restack.resumed", { timeout = "24h" })
    if w.outcome ~= "resumed" then return ctx.fail("conflict not resolved") end
  elseif r.outcome ~= "done" then
    return ctx.fail("restack " .. r.outcome)
  end
  ctx.notify("Restacked " .. plan.branch_count .. " branches on " .. input.trigger.parent)
end
return M
```

## Sandbox (D94)

**Invariante:** o script é lógica não confiável e o host é o único ator. O script não tem autoridade própria; tudo o que ele faz passa por `ctx`.

- Motor: uma VM Lua compilada no binário (autocontido). O candidato padrão é **Luau via `mlua`** (modo sandbox, gancho de interrupção, limite de memória, endurecido para código não confiável); alternativas: Lua 5.4 (exige lista branca manual) e Rhai (Rust puro, mas não é Lua). Quem decide é o spike S-D (Q22).
- Removidos do ambiente: `io`, `os`, `debug`, `package`/`require`, `load`/`loadstring`/`dofile`, `string.dump`. As fontes de não determinismo (`os.time`, `os.clock`, `math.random`) saem; `ctx.now()` e `ctx.random()` as substituem e passam pelo journal. O ambiente global é somente leitura e é novo a cada run, para que um run não afete o seguinte.
- **Orçamentos por run:** instruções, tempo total de relógio, memória, tamanho de string, profundidade de pilha e número de chamadas de `ctx`. O número de chamadas substitui o teto de 1000 transições de D78. Estourar um orçamento termina o run como `failed` com o motivo. Os valores são a definir no spike.
- **Cancelamento:** o gancho de interrupção da VM checa um sinal. Uma chamada bloqueada (`approve`, `wait`, `exec`) é acordada pelo cancelamento, e `exec` mata o processo (D65).
- **Journal:** o host grava cada chamada de `ctx` (sequência, função, argumentos já resolvidos, resultado, outcome, duração). A aba Runs (D84) é construída só dele. O que o script escreve com `ctx.notify` ou `ctx.log` aparece marcado como "do script".
- **Limite conhecido:** a VM roda no mesmo processo. A segurança dela é a da implementação em C ou C++. Um processo separado para o script endureceria isso e fica fora da v1. `exec` é, por desenho, uma saída para programas do usuário; o sandbox protege o app e o repositório da lógica do script, não de um programa que o próprio script chama (a regra D96 cobre esse caso).

## Execução efêmera (D95)

- Cada run roda em uma thread própria, fora da UI. `ctx.approve` e `ctx.wait` bloqueiam essa thread em um canal. Não há corrotina atravessando chamadas do host.
- Fechar o app encerra os runs em curso. Na abertura seguinte, um run que não terminou é marcado `interrupted`; um `apply` que começou e não tem resultado no journal aparece como interrompido e nunca roda de novo (D80, sem a retomada).
- Esperas e prazos de `timeout` vivem na memória. `workflow_wait` e `workflow_pending` deixam o SQLite; as filas de `concurrency` (D65) também ficam na memória.
- O `state.db` guarda o journal (o histórico do que rodou, para a aba Runs) e a retenção continua (200 runs por workflow e repositório, saídas grandes cortadas). Ele deixa de ser um mecanismo de recuperação.
- **A porta fica aberta para a execução durável estilo Temporal** (Q23): o sandbox já elimina o não determinismo, `ctx` é a única fronteira de efeito e o journal já registra `(seq, função, argumentos, resultado)`. Retomar seria reexecutar o script do início alimentando-o com os resultados gravados até o fim do journal, e continuar ao vivo daí. Ficam para esse dia o versionamento de um script editado com runs em curso, sinais e temporizadores persistentes.

## Aprovação e confiança (D96, revisa D64)

- Cada versão de um script é identificada pelo hash do texto. Um hash que o humano ainda não revisou é **não revisado**.
- Na primeira execução de um hash não revisado (manual ou por evento), o app mostra o script e pergunta "Run this version?". Confirmar marca o hash como revisado e é o momento em que o humano pode conceder a aprovação automática dos `apply` por evento (`approval = "auto"` passa a ser uma escolha na UI, presa ao hash). Qualquer edição, do humano ou de um agente, volta a "não revisado". Isso refina D64(a): o humano que aperta Run sobre um script que um agente acabou de escrever não viu o que ele faz.
- Rodar um hash revisado: ações `apply` seguem D64 (a) a (c): manual roda, evento pausa sem aprovação automática, agente (MCP) sempre espera. `push.lease` sempre pede (D64(d)).
- **Regra D64(e) trocada por uma regra grossa:** depois de qualquer `ctx.exec` num run, toda ação `apply` seguinte desse run pede aprovação, e o diálogo mostra os argumentos já resolvidos. Não dá para saber estaticamente de onde um valor veio; o humano aprova o comando concreto.
- Rever o script aprova as chamadas a `exec` que ele contém. Um run por evento ou por agente só pode chamar `exec` se o hash estiver revisado.

## Agentes escrevem scripts, fora do MCP (D97)

O MCP continua com `workflow_list` e `workflow_run` (D71, WF5). Não há ferramenta para criar ou editar scripts pelo MCP. Um agente (por exemplo o Claude Code) escreve o arquivo `.lua` no diretório de workflows do usuário, como qualquer arquivo de texto, e a regra de confiança por hash (D96) garante que o humano veja o script antes da primeira execução. Para o agente acertar a ligação com o app, ele precisa de documentação atual e de um jeito de testar:

**Documentação gerada.** O app grava em `<diretório de workflows>/.api/`, a cada início em que a versão mudou:

- `API.md`: a referência de `ctx`, gerada do catálogo (assinaturas, níveis, outcomes), mais o formato de `M.meta`, as fases e os exemplos deste documento.
- O arquivo de tipos para completar no editor (por exemplo `ctx.d.luau`; o formato depende do motor).
- Opcionalmente um `SKILL.md` curto para agentes que usam skills.

Gerar do catálogo evita que a documentação se afaste do código. O diretório está no config do usuário, não em um repositório.

**Testar sem a UI.** Três subcomandos de uso pontual, como o `gitmaster mcp` já é (`06`, regra 7). Sem processo residente e sem estado, então não são um modo headless ou servidor (D02), mas essa leitura de D02 é do autor e está registrada em D97:

| Subcomando | Faz |
|---|---|
| `gitmaster workflow check <arquivo>` | Compila no sandbox, valida `M.meta`, confere o gatilho e as entradas. Saída `arquivo:linha: mensagem` (e `--json`); código de saída diferente de zero em erro. |
| `gitmaster workflow dry-run <arquivo> [--input k=v] [--repo <caminho>]` | Roda o script com um `ctx` de simulação: leituras reais; `propose`, `apply` e `exec` só registrados, com resultados de mesma forma (D85, Q21: `exec` nunca roda). Imprime a lista de chamadas. O que vem depois de um valor simulado é uma estimativa e é marcado assim. |
| `gitmaster workflow test <arquivo> [fixture]` | Roda contra um `ctx` falso com respostas combinadas em uma fixture (Lua) e confere a sequência de chamadas e o valor devolvido. O `ctx` falso é o mesmo que os testes de `gm-core` usam. |

Efeitos reais só rodam pela UI, com o humano como ator. A CLI nunca faz `apply`.

## Interface (revisa D76)

A página Workflows continua: lista à esquerda, detalhe à direita, com as abas **Script** e **Runs**. A aba Flow (grafo, inspetor, edição estrutural) deixa de existir.

- **Script:** o texto do arquivo no editor da aplicação, com diagnósticos de linha vindos de `check` ao digitar, e o aviso de que salvar torna o hash "não revisado". Salvar continua atômico e recusa quando o arquivo mudou no disco; excluir move para a lixeira. O destaque de sintaxe usa o tree-sitter de D92 (a gramática de Lua), se o N4 já o tiver.
- **Runs:** a visão de D84, agora com uma entrada por chamada de `ctx` (função, argumentos resolvidos, resultado, outcome, duração), construída do journal.
- **Dry run:** o mesmo mecanismo do subcomando, sobre o texto aberto no editor.
- **Revisão:** o diálogo "Run this version?" da D96.

## O que sobrevive da WF0 a WF2 e o que sai

| Fica (adaptado) | Sai |
|---|---|
| `store` (SQLite, retenção; sem esperas nem fila), `driver` (vira o executor de `ctx`), `exec`, `loader` (arquivos `.lua`, `gitmaster.workflowPath`), `workflow_session`, `catalog` (vira a fonte de `ctx`), `templates` (reescritos em Lua), visão de runs, comandos e eventos de workflow, paleta, aprovação | `model`, `parse`, `expr`, `validate`, `machine`, `edit`, `dry_run` (cerca de 6,6 mil linhas em `gm-core`), `flow_graph`, `flow_graph_view`, `flow_inspector`, a aba Flow, os fixtures `.toml`, o cenário `w-flow-editor` |

Não há suporte aos dois formatos ao mesmo tempo: dois interpretadores dobrariam a superfície. Depois da troca, um arquivo `.toml` no diretório de workflows é ignorado com um aviso visível. Se existirem workflows TOML reais do autor, a conversão é feita à mão uma vez (os modelos embutidos são reescritos em Lua).

## Questões

- **Q22. Motor de script.** Luau (`mlua`), Lua 5.4 ou Rhai. Decidir com os números do spike S-D (ver `07`): tentativas de escape do sandbox, orçamentos e latência de cancelamento, tamanho do binário e compilação nas três plataformas (Luau é C++, o que exige um compilador C++ no CI; hoje o `rusqlite` embutido já exige C).
- **Q23. Execução durável.** Reexecução a partir do journal, como o Temporal, adiada por D95. Reabrir quando a perda de runs ao fechar o app incomodar.
