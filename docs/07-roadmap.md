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

## Fatia 1b: o básico que falta (antes dos diferenciais)

Revisão de 2026-10-16: com a Working Copy pronta, faltam três coisas sem as quais o app não substitui um cliente Git no dia a dia. Entram antes da fatia 2.

Ordem: log de comandos, depois trocar de branch, depois trocar de projeto. O log vem primeiro porque as duas outras operações passam a aparecer nele, o que ajuda a validá-las.

1. **Log de comandos Git** (feito; modelo: `rvw-gui/src/git_log.rs`).
   - Só operações com efeito colateral entram (D28): stage, unstage, commit, checkout. Status, diff e leituras em massa não aparecem.
   - Cada processo `git` dessas operações vira uma entrada estruturada: programa, diretório, argumentos, código de saída, duração, stdout, stderr. Buffer em memória, últimas 1000 entradas, na UI nunca no repositório.
   - Ponto único de registro no `SubprocessGit`: `run` ganha uma variante que registra, usada só pelas escritas. A saída do hook de commit entra junto, pois é o stderr do `git commit`.
   - Botão na status bar abre um painel flutuante, somente leitura, com a lista (mais recente embaixo), falhas destacadas e saída expansível.
   - Pré-requisito (feito): `stage`, `unstage` e `commit` passam a rodar pelo executável `git` (D26 revisada). O commit agora executa os hooks nativos do Git (D18), o que antes não acontecia. O stage por linha (D24) passa a escrever o blob com `git hash-object -w` e o index com `git update-index --cacheinfo`, reaproveitando `stage_math`.
   - O restack (libgit2) altera refs; como registrá-lo no log é decisão da fatia 5.
   - É diferente do operation log do motor (D14/docs 06): este mostra o que o Git executou; aquele, o que foi pedido e por quem.
2. **Trocar de branch** (feito).
   - Comandos `ListBranches` e `Checkout { branch }` em `gm-schema`; `GitPort` ganha os métodos correspondentes.
   - Seletor na status bar ou no cabeçalho: lista local e remota com filtro; remota sem local cria a local de rastreio.
   - Com working copy sujo, o checkout que o Git recusar mostra o erro do Git no painel e uma notificação; nada de autostash escondido (mesma linha de Q2). Ver Q11.
3. **Trocar de projeto** (feito).
   - Seletor de pasta nativo e lista de recentes, persistida no diretório de dados do app (nunca dentro do repositório; princípio de 00).
   - `OpenRepo` já existe: a troca descarta o estado do repo atual (seleção, diff, rascunho do commit) de forma explícita e abre o novo.
   - Um repositório por janela na v1; abas ou várias janelas ficam para depois.
4. **Tela de boas-vindas** (a fazer).
   - Aparece quando o app abre sem projeto: primeira execução, sem argumento de linha de comando, diretório atual fora de um repositório e nenhum recente que ainda exista. Hoje essa abertura não tem um estado próprio.
   - Conteúdo: marca, uma frase sobre o app, ação "Open a repository" (o mesmo seletor de pasta de 3), lista de recentes quando houver (as que sumiram do disco saem da lista) e a dica do atalho de teclado.
   - Reaproveita os recentes e o `OpenRepo` de 3; abrir um repositório troca a tela sem reiniciar. Pasta que não é repositório mostra o erro na própria tela, sem diálogo bloqueante.
   - Sem projeto, as ações que dependem de repo (commit, checkout, log de comandos) ficam ausentes da tela, não desabilitadas e vazias.
   - Strings em inglês; arte do ícone de `assets/icons/`, nas cores do tema ativo (08).

Pronto quando: troca de projeto e de branch sem sair do app, todo comando Git executado é inspecionável no painel, e abrir o app sem projeto leva a uma tela que explica o que fazer em vez de uma janela vazia.

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
