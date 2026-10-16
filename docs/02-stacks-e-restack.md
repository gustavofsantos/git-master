# Stacks e restack

Última atualização: 2026-10-16. Status de cada decisão em `01`.

## Modelo

Cada branch tem no máximo um pai e vários filhos: uma árvore com raiz em `main` (D03). Diamante (dependência de dois pais) não é suportado. Contorno: uma branch de integração que mergeia as duas e vira filha de uma só.

```
main
└ ledger-base
  ├ ledger-api
  └ ledger-ui
    └ ledger-e2e
```

## Metadata (proposto, D13)

No git config do próprio repositório (`.git/config`), fora do working tree:

- `branch.<nome>.stackParent`: nome do pai.
- `branch.<nome>.stackParentBase`: SHA da ponta do pai quando a branch foi criada ou reancorada pela última vez.

Valores escalares, sem listas. Nome das chaves a confirmar (Q4).

## Estados derivados

- **Desatualizada**: a ponta atual do pai é diferente de `stackParentBase`.
- **Base divergente**: o merge-base real entre filho e pai é diferente de `stackParentBase` (alguém fez rebase fora do app). Sinalizar ao abrir o repo.
- **Pai sumiu ou foi mergeado**: branch do pai deletada ou conteúdo já presente na `main` (patch-id). Oferecer "reparentar para o avô", com confirmação. Caso típico: squash merge do pai.

O cálculo roda fora da thread de UI e a interface mostra "calculando" enquanto isso.

## Restack (proposto, D14)

- Percorrer a árvore em pré-ordem (pais antes dos filhos).
- Por aresta: `rebase --onto <ponta do novo pai> <stackParentBase> <filho>`. Ao concluir, `stackParentBase` recebe a ponta do novo pai.
- **Plano exato e determinístico**: simula a mesma sequência de cherry-picks, commit a commit, com o mesmo algoritmo da execução (ex.: `git merge-tree --write-tree`). Fica amarrado aos SHAs das refs e é invalidado se qualquer ref mudar. Nunca "conflito provável".
- **Execução em memória** (rebase in-memory do libgit2; verificar cobertura), sem tocar no working copy.
- **Snapshot das refs** antes de começar, **undo atômico** de todas as branches afetadas, registro no operation log (modelo parecido com o `jj op log`).
- **Estado persistido**: sobrevive a fechar ou travar o app.
- **Conflito** pausa só a subárvore afetada. Irmãos independentes continuam; descendentes ficam bloqueados.
- **rerere** habilitado: o mesmo conflito reaparece a cada restack e a resolução gravada é reaplicada de forma determinística.
- **Push com lease** (D15) é um passo separado e explícito, nunca automático.
- Working copy sujo com a branch atual no plano: bloquear com banner (Q2).

## Fluxo da UI

1. **Plano**: lista ordenada das branches, quantos commits cada uma reaplica, o que depende do quê.
2. **Executando**: progresso por branch, em memória.
3. **Conflito**: a branch pausa; ação "Resolver conflito", "Pular branch e filhas" ou "Abortar tudo".
4. **Concluído**: "Push com lease" e "Desfazer restack".

## Integração

- Hooks: `pre-restack` (gate, recebe o plano) e `post-restack` (observer). Ver `03`.
- MCP: `stack_tree`, `restack_plan`, `restack_apply(plan_id)`. Ver `04`.
- Sidebar e histórico: ver `05`.
