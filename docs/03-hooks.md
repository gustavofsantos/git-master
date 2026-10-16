# Hooks

Última atualização: 2026-10-16. Status de cada decisão em `01`.

## Ideia central

Eventos do próprio app (não os hooks nativos do Git) disparam scripts configurados no `.gitconfig`. Modelo inspirado nos hooks do Claude Code, com schema versionado de entrada e de saída por evento (D04). Execução assíncrona (D05), com id de correlação (D06) e resposta pelo stdout (D07).

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
| `pre-commit` | gate | veta o commit; roda contra a index tree |
| `pre-push` | gate | veta o push |
| `pre-restack` | gate | recebe o plano; veta |
| `prepare-commit` | transform | reescreve a mensagem |
| `prepare-branch-name` | transform | reescreve o nome (ex.: chave do Jira) |
| `resolve-conflict` | transform | propõe resolução de um hunk |
| `post-commit` | observe | |
| `post-push` | observe | |
| `post-restack` | observe | |

## Configuração (proposto)

Um hook por subseção, para poder nomear e desligar. Valor de `mode` ainda em aberto (Q1).

```ini
[gitmaster "prepare-commit.llm"]
    command = claude -p "Mensagem conventional commit para este diff"
    mode    = text
    match   = branch:feat/*
    timeout = 30s

[gitmaster "post-restack.testes"]
    command = cargo test --quiet
```

A ordem de execução é a ordem em que as seções aparecem no config resolvido (inclui `includeIf`), inspecionável com `git config --list --show-origin`.

### Confiança (D17)

Hook em `.gitconfig` de repositório é execução de código vinda de quem criou o repo. Padrão: ler hooks só do config global e do de usuário. Hooks de repo exigem aprovação, guardada fora do repo e amarrada ao hash do comando (como `direnv allow`). `includeIf "gitdir:~/trabalho/"` ativa hooks por árvore de diretórios sem escrever nada no repositório.

### Hooks nativos (D18)

Os hooks nativos do Git (`.git/hooks`, `core.hooksPath`, Husky) continuam rodando, antes dos do GitMaster.

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

## `pre-commit` contra a index tree (D19)

Com stage parcial, passar no working tree não prova que o commit compila. O gate roda num checkout temporário da index tree, que testa exatamente o que vai ser commitado.

## Saída de LLM

Não é determinística, então vale só como proposta: preenche o campo da mensagem, e o commit só acontece por ação do usuário. O operation log registra comando, hash do comando e exit code de cada hook.

## Questão aberta (Q1)

Escrever JSON em one-liners de shell exige `jq`. Opções: (A) JSON puro; (B) pipe de texto com contrato paralelo; (C) JSON canônico com `mode = text` opcional por hook, em que o host extrai o campo principal para o stdin e embrulha o stdout de volta (e faz o eco do id). Recomendação: C.
