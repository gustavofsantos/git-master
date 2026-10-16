# Roadmap

Última atualização: 2026-10-16. **Proposta inicial**, para iterar. A ordem prioriza chegar cedo a algo que o autor use no dia a dia e que já mostre um diferencial.

## Critério de ordem

1. Risco técnico primeiro (spikes), antes de construir em cima.
2. Cada fatia é vertical e usável sozinha.
3. O primeiro diferencial (hooks) aparece cedo; stacks e MCP depois da base de Working Copy e histórico.
4. Se o diferencial mais urgente for o stacking, antecipar a fatia 5 (a ordem é uma decisão do autor).

## Fatia 0: spikes e esqueleto

Objetivo: validar o que pode invalidar o desenho.

- Spike A: descartado pelo autor em 2026-10-16 (lista virtualizada do GPUI com 100 mil linhas).
- Spike B: texto editável no GPUI (painel de resultado do resolvedor); avaliar bibliotecas e licenças (Q7).
- Spike C: rebase in-memory do libgit2 (cobertura) e `git merge-tree --write-tree`; comparar com gitoxide (Q6).
- Spike D: status em repositório massivo via subprocess com fsmonitor.
- Esqueleto: workspace Cargo com `gm-schema`, `gm-core`, `gm-ui`, `gm-mcp`; janela GPUI; abrir um repo; operation log mínimo; testes de propriedade de base.

Pronto quando: abre um repositório grande e lista as mudanças sem travar; os spikes têm veredito registrado em `01` (Q6, Q7).

## Fatia 1: Working Copy e commit

- Lista de arquivos com checkbox tri-state, filtro e agrupamento.
- Diff unificado (histogram), stage por arquivo, hunk e linha (aviso de par −/+).
- Composer de commit; commit.
- Atalhos de teclado.

Pronto quando: substitui `git add -p` mais `git commit` no uso diário.

## Fatia 2: hooks

- Contrato JSON versionado em `gm-schema`; configuração por `.gitconfig`.
- `prepare-commit` (transform assíncrono) e `pre-commit` (gate contra a index tree).
- Chips de hooks no composer; reconciliação; cancelar.
- Aprovação de hooks de repo.
- Decidir Q1 (modo texto) antes de começar.

Pronto quando: a mensagem de commit pode ser gerada por um script do autor, sem travar a UI.

## Fatia 3: histórico e branches (visão por nome)

- Grafo com lanes, escopo configurável, busca por conteúdo.
- Sidebar na visão por nome.
- Performance com commit-graph.

Pronto quando: navega o histórico de um repositório grande com rolagem fluida.

## Fatia 4: stacks (modelo e visualização)

- Metadata `stackParent` e `stackParentBase`; criar e reparentar branch (arrastar).
- Sidebar por stack; indicadores de defasagem; aresta tracejada no grafo.

Pronto quando: um stack real do dia a dia é representado e a defasagem é visível.

## Fatia 5: restack

- Plano exato por commit, execução em memória, snapshot e undo atômico, estado persistido.
- Fallback sem resolvedor: em conflito, pausa e entrega o conflito no working tree para resolver fora.
- `pre-restack` e `post-restack`.
- Decidir Q2 (working copy sujo).

Pronto quando: um restack completo roda, pode ser desfeito e sobrevive a fechar o app.

## Fatia 6: resolução de conflitos

- Três painéis, lados rotulados por papel, base sob demanda, rerere.
- Hook `resolve-conflict` como proposta.

Pronto quando: resolve os conflitos de um restack sem sair do app.

## Fatia 7: MCP

- Servidor no app mais shim por socket.
- Tools de Read e Propose primeiro; Apply com aprovação na UI depois.
- Política de config; operation log com `actor`.

Pronto quando: um agente consegue ler o stack, propor um restack e aplicá-lo mediante aprovação.

## Fatia 8: review local

- Comentários, estados, âncora sem adivinhar, snapshots antes/depois.
- Tools `review_comments_list` e `review_comment_resolve`.
- Comentário em linha no diff com Antes/Depois.
- Confirmar D11 e Q3 antes de começar.

## Fatia 9: acabamento premium

- Performance em monorepo, layouts alternativos, empacotamento, documentação, licença e modelo de produto (Q10).

## Riscos transversais

- GPUI: texto editável, ecossistema de componentes e licenças.
- Escala: status, grafo e diff em repositórios massivos.
- Escopo: Tower tem pull requests, worktrees, submódulos e LFS (Q8). Decidir o que fica de fora do v1.
