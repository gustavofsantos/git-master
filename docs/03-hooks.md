# Hooks

Última atualização: 2026-10-19. Status de cada decisão em `01`.

## Ideia central

Eventos do próprio app (não os hooks nativos do Git, que continuam sendo do Git: D29) disparam scripts configurados no `.gitconfig`. Modelo inspirado em hooks de eventos de agentes, com schema versionado de entrada e de saída por evento (D04). Execução assíncrona (D05), com id de correlação (D06) e resposta pelo stdout (D07).

## Classes (proposto, D16)

| Classe | O que o hook pode fazer |
|---|---|
| Gate | vetar a ação |
| Transform | reescrever um campo do evento |
| Observe | apenas reagir depois |

Gates e observers rodam em paralelo (o resultado de gates é um AND e a ordem não importa). Transforms rodam em sequência, na ordem do config resolvido, e cada um recebe a saída do anterior.

## Catálogo inicial

| Evento | Classe | Efeito |
|---|---|---|
| `pre-push` | gate | fora do v1: o nome é nativo do Git (D29) |
| `pre-restack` | gate | recebe o plano; veta |
| `prepare-commit` | transform | reescreve a mensagem (fatia 2) |
| `prepare-branch-name` | transform | reescreve o nome (ex.: chave do Jira) |
| `resolve-conflict` | transform | propõe resolução de um hunk |
| `post-commit` | observe | fora do v1: o nome é nativo do Git (D29) |
| `post-push` | observe | fora do v1: o nome é nativo do Git (D29) |
| `post-restack` | observe | |

## Configuração (proposto)

Um hook por subseção, para poder nomear e desligar. O contrato é sempre JSON (D30); não há `mode`.

```ini
[gitmaster "prepare-commit.llm"]
    command = ~/bin/suggest-commit-message
    match   = branch:feat/*
    timeout = 30s

[gitmaster "post-restack.testes"]
    command = cargo test --quiet
```

A ordem de execução é a ordem em que as seções aparecem no config resolvido (inclui `includeIf`), inspecionável com `git config --list --show-origin`.

### Confiança (D17, decidido)

Hook em `.gitconfig` de repositório é execução de código vinda de quem criou o repo. Padrão: ler hooks só do config global e do de usuário. Hooks de repo exigem aprovação, guardada fora do repo e amarrada ao hash do comando (como `direnv allow`). `includeIf "gitdir:~/trabalho/"` ativa hooks por árvore de diretórios sem escrever nada no repositório.

### Hooks nativos (D18, D29)

Os hooks nativos do Git (`.git/hooks`, `core.hooksPath`, Husky) continuam rodando e continuam sendo do Git. O GitMaster não os reimplementa, não os envolve e não mostra chip para eles: o `pre-commit`, por exemplo, roda dentro de `git commit` e, se vetar, a saída dele é o erro do commit (já é assim desde a fatia 1b).

## Contrato

### Entrada (stdin, JSON)

```json
{
  "schema_version": 1,
  "event": "prepare-commit",
  "invocation_id": "01J9Y...",
  "repo": { "root": "/work/ledger", "branch": "ledger-ui", "head": "a1f3c9e" },
  "input": { "message": "", "staged_diff": "diff --git ..." }
}
```

Conteúdo grande (ex.: diff acima de um limite) vem como `*_path`, apontando para arquivo temporário, para o contrato não variar de forma surpreendente com o tamanho.

### Saída (stdout, JSON)

```json
{ "invocation_id": "01J9Y...", "output": { "message": "feat(ledger): aceita Money em total()" } }
```

Gate:

```json
{ "invocation_id": "01J9Y...", "decision": "block", "reason": "branch protegida" }
```

### Regras

- `invocation_id` diferente do enviado é erro do hook: o resultado é descartado e a UI mostra um aviso visível.
- Exit code diferente de zero significa que o hook quebrou. Gate quebrado é fail-closed (veta e mostra o stderr); observer quebrado só avisa.
- Timeout ou cancelamento mata o processo. Como a resposta só vem pelo stdout, não há resposta tardia a reconciliar.

## Reconciliação

O motor guarda o fingerprint do alvo ao iniciar a invocação (ex.: SHA do HEAD, index tree, hash da mensagem) e compara quando a resposta chega. O hook nunca vê esse fingerprint.

1. `invocation_id` desconhecido ou não pendente: descarta e registra.
2. Estado do alvo mudou: transform vira **sugestão** (o usuário decide); gate roda de novo.
3. Estado igual: aplica o resultado e registra no operation log.

Exemplo: o usuário digitou a mensagem enquanto o LLM gerava. A sugestão aparece com botão "Substituir" e não sobrescreve o texto.

## Execução assíncrona na UI

- Gate assíncrono não é gate opcional: a ação fica "pendente" até todos os gates responderem.
- A aprovação vale para os SHAs do momento; se o HEAD mudou, ela é invalidada.
- Cada hook aparece como chip (rodando, ok, bloqueou, falhou), com cancelar.
- Transforms em cadeia somam latência: um chip por hook.

## Saída de LLM

Não é determinística, então vale só como proposta: preenche o campo da mensagem, e o commit só acontece por ação do usuário. O operation log registra comando, hash do comando e exit code de cada hook.

## Q1 resolvida (D30)

Escrever JSON em one-liners de shell exige `jq`. Decisão do autor (2026-10-16): JSON puro, sem modo texto. Quem quiser texto escreve um script curto que lê o JSON.

## Hooks e workflows (proposto, D70)

Hooks continuam pontos de extensão dentro de uma operação (gate, transform, observe). Workflows (`10`) orquestram entre operações.

- O passo `exec` de um workflow reusa exatamente este contrato de processo (stdin JSON com `schema_version` e `invocation_id`, stdout JSON, timeout mata o processo) e as mesmas regras de confiança (D17).
- Os eventos do motor listados na D67 podem ser gatilhos de workflow.
- Sem migração de hooks para workflows na v1 (Q18).
