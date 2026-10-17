# Roadmap

Última atualização: 2026-10-16. **Proposta inicial**, para iterar. Marcas: **(feito)** no código e verificado; **(a fazer)** ainda não começado; sem marca, fatia futura. A ordem prioriza chegar cedo a algo que o autor use no dia a dia e que já mostre um diferencial.

## Critério de ordem

1. Risco técnico primeiro (spikes), antes de construir em cima.
2. Cada fatia é vertical e usável sozinha.
3. O primeiro diferencial (hooks) aparece cedo; stacks e MCP depois da base de Working Copy e histórico.
4. Se o diferencial mais urgente for o stacking, antecipar a fatia 5 (a ordem é uma decisão do autor).

## Fatia 0: spikes e esqueleto (feito)

Objetivo: validar o que pode invalidar o desenho.

- Spike A (descartado): descartado pelo autor em 2026-10-16 (lista virtualizada do GPUI com 100 mil linhas).
- Spike B (feito): texto editável no GPUI (painel de resultado do resolvedor); avaliar bibliotecas e licenças (Q7). Veredito em Q7.
- Spike C (feito): rebase in-memory do libgit2 (cobertura) e `git merge-tree --write-tree`; comparar com gitoxide (Q6). Veredito em Q6.
- Spike D (feito, fsmonitor sem veredito): status em repositório massivo via subprocess com fsmonitor. Em `spikes/d-status/FINDINGS.md`; o fsmonitor não pôde ser medido neste build do Git.
- Esqueleto (feito): workspace Cargo com `gm-schema`, `gm-core`, `gm-ui`, `gm-mcp`; janela GPUI; abrir um repo; operation log mínimo; testes de propriedade de base.

Pronto quando: abre um repositório grande e lista as mudanças sem travar; os spikes têm veredito registrado em `01` (Q6, Q7).

## Fatia 1: Working Copy e commit (feito)

- Lista de arquivos com checkbox tri-state, filtro e agrupamento por diretório (feito).
- Diff unificado (histogram), stage por arquivo, hunk e linha (aviso de par −/+) (feito).
- Composer de commit; commit (feito).
- Atalhos de teclado (feito: navegação, stage, filtro, atualizar, foco).

Pronto quando: substitui `git add -p` mais `git commit` no uso diário. Código completo; o uso diário é o teste do autor.

## Fatia 1b: o básico que falta (antes dos diferenciais) (feito)

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
4. **Tela de boas-vindas** (feito).
   - Aparece quando o app abre sem projeto: primeira execução, sem argumento de linha de comando, diretório atual fora de um repositório e nenhum recente que ainda exista. Hoje essa abertura não tem um estado próprio.
   - Conteúdo: marca, uma frase sobre o app, ação "Open a repository" (o mesmo seletor de pasta de 3), lista de recentes quando houver (as que sumiram do disco saem da lista) e a dica do atalho de teclado.
   - Reaproveita os recentes e o `OpenRepo` de 3; abrir um repositório troca a tela sem reiniciar. Pasta que não é repositório mostra o erro na própria tela, sem diálogo bloqueante.
   - Sem projeto, as ações que dependem de repo (commit, checkout, log de comandos) ficam ausentes da tela, não desabilitadas e vazias.
   - Strings em inglês; arte do ícone de `assets/icons/`, nas cores do tema ativo (08).
   - Como ficou: sem argumento e fora de um repositório, abre o recente mais novo que ainda existe; sem recentes, mostra a tela. Um caminho passado na linha de comando que não é repositório mostra o erro do Git na própria tela. Sem atalho de teclado para abrir (não existe um). O seletor nativo só foi testado até a porta (não dá para dirigir o diálogo sem tela).
5. **Sidebar do desenho** (feito, com placeholders).
   - Coluna à esquerda, como no canvas: "Workspace" (Working Copy com a contagem de arquivos; Histórico, Stashes, Review local e Reflog são placeholders que avisam que ainda não existem), alternância "By stack" / "By name" e a árvore de branches.
   - "By name" é real: locais e remotas agrupadas pelo prefixo; clicar numa branch faz checkout, e a atual ganha o selo HEAD. "By stack" mostra "No stacks yet" e todas as locais em "No stack", até a fatia 4; a dica de arrastar para definir o pai também é placeholder.
   - Faltam do desenho: indicadores de stack (`restack N`, `↑N`, ponto âmbar, PR), botões Fetch/Pull/Push/Restack e a busca na barra superior, a barra de status com escopo, fsmonitor, operation log e MCP.

Pronto quando: troca de projeto e de branch sem sair do app, todo comando Git executado é inspecionável no painel, e abrir o app sem projeto leva a uma tela que explica o que fazer em vez de uma janela vazia.

## Fatia 1c: Working Copy simples, menus contextuais e paleta de comandos (em andamento)

Pedido do autor em 2026-10-16, com base nas duas pranchas novas do canvas (menu de contexto de branch; paleta de comandos). Vem antes de a fatia 2 ganhar interface, porque o composer e os chips dela moram na tela simplificada.

1. **Simplificar a Working Copy** (feito; descartar hunk entrou como `DiscardHunk`, no stage lado Staged o descarte é recusado).
   - Cabeçalho "Working Copy" com a contagem ("6 changed"); lista plana, uma linha por arquivo: letra de estado (M, A, D, ?) e caminho completo, sem filtro, sem campo de contagem e sem checkbox por linha.
   - Composer na base da coluna: caixa da mensagem e botão "Commit on <branch>".
   - Diff com o caminho e o resumo `+5 −1` no cabeçalho, sem barra de botões extra.
   - Decidido pelo autor em 2026-10-16: stage e unstage como no Git Tower (prancha de referência na conversa de design).
     - O diff tem o seletor "Staged | Unstaged" no topo, ao lado do nome do arquivo, que escolhe de que lado do arquivo se olha (hoje os dois lados aparecem juntos).
     - Abaixo do cabeçalho, um resumo ("2 chunks, 3 insertions, 4 deletions").
     - Cada hunk tem na linha do seu cabeçalho `@@` os botões "Discard Chunk" (pede confirmação) e "Stage Chunk" ("Unstage Chunk" no lado staged). O stage por linha (D24) continua, pelo gutter.
     - O arquivo inteiro faz stage e unstage por tecla (espaço) e pelo menu de contexto do arquivo; a letra de estado na lista ganha uma marca discreta quando o arquivo tem algo staged. O "lido" segue como eixo ortogonal (D23).
2. **Registro único de ações** (feito, em `gm-ui/src/action_registry.rs`; pré-requisito de 3 e 4; o stage por linha do D24 ficou fora dele).
   - Cada ação tem id estável, título, atalho, argumentos tipados (branch, arquivo) e um predicado "habilitada agora?" que devolve o motivo quando não está. Teclado, menus e paleta leem do mesmo registro, para nunca divergirem; o que o motor ainda não faz entra como ação desabilitada com o motivo "not available yet".
   - Rodar uma ação despacha o mesmo `CommandKind` de hoje, com `Actor::Human`. Nada de caminho paralelo.
3. **Menus contextuais** (feito para branch e arquivo; commit e stash continuam a fazer).
   - Feito: clique direito em branch da sidebar e em arquivo da lista; `shift-F10` abre o menu do arquivo selecionado e `Esc` fecha. Itens vêm do registro (`context_menu.rs`); atalho à direita; separadores por grupo; quando o item não roda, o motivo aparece no lugar do atalho; item destrutivo em vermelho e pede confirmação antes de rodar.
   - Menu de branch: Checkout, New child branch, Create worktree, Restack this stack, Push with lease, Copy branch name, Rename, Delete. Só Checkout e Copy branch name rodam; os demais aparecem desabilitados com "Not available yet".
   - Menu de arquivo: Stage file e Unstage file (cada um desabilitado quando não há o que fazer), Discard changes, Copy file path, Reveal in file manager. Discard changes fica desabilitado: o motor só descarta chunk (`DiscardHunk`), e nenhum comando novo entrou em gm-core.
   - Lacunas: menus de commit e de stash; o menu de branch não abre pelo teclado (a sidebar não tem foco de lista) e não fecha com `Esc`; a confirmação do Discard não é exercitada enquanto o item está desabilitado; Reveal não é coberto pelo smoke (um clique abriria o gerenciador de arquivos) e oferece o caminho mesmo quando o arquivo sumiu do disco; a tecla de menu usa a posição do ponteiro, não a da linha.
4. **Paleta de comandos** (feito; `⌘K` no macOS ainda não testado ali).
   - Feito: `Ctrl+K` abre a paleta; campo único com busca difusa (`fuzzy.rs`, função pura com testes de ordem e de trechos casados); seções "Actions", "Worktrees" e "Branches", cada uma só quando tem resultado (branches só depois de digitar); trecho casado em destaque; contagem de resultados; setas navegam, Enter executa, Tab escolhe o argumento da ação (branch ou arquivo), Esc sai do passo do argumento ou fecha; rodapé com as dicas; lista virtualizada; ações desabilitadas aparecem com o motivo.
   - Roda pelo mesmo caminho da tecla e do menu (`run_action`, `Actor::Human`), com as ações do registro. A ordenação roda fora da thread de UI; `palette.rs` monta as opções e controla o destaque, e só o que chega da ordenação mais recente é mostrado.
   - Lacunas: Worktrees mostra só "Create Worktree" desabilitado (Q8 em aberto); ações de chunk aparecem desabilitadas com "Pick a chunk in the diff first.", porque rodam do painel de diff; commit e stash continuam sem menu de contexto; a lista de arquivos só se atualiza com o próximo evento de status; o smoke cobre só o teclado (não o clique numa linha, nem Esc no passo do argumento, nem a paleta sem repositório); o destaque é guloso, então em termos com letras repetidas ele pode marcar letras menos óbvias.
5. **Worktrees** (a decidir, Q8). O desenho traz a seção "Worktrees" na sidebar (aqui, agente, sem repo) e as ações de criar e remover. Não entram na v1 sem decisão do autor; até lá as ações ficam desabilitadas no registro.

6. **Atualização automática, sem botão de refresh** (feito; desenho confirmado pelo autor em 2026-10-16). O botão saiu da barra superior; a ação "Refresh" segue no registro e na tecla `r`. Lacunas conhecidas: edição externa dentro dos 150 ms após uma escrita do app pode passar; um refresh extra ao abrir; macOS (FSEvents) e árvores perto do teto de 5.000 diretórios sem teste.
   - Premissa: a tela nunca fica velha e o usuário nunca precisa pedir atualização. O botão sai da barra superior só quando isto estiver pronto; a ação "Refresh" continua no registro (tecla `r` e paleta) como saída de emergência.
   - Gatilhos: a janela volta ao foco; mudança em `.git` (HEAD, index, refs, `packed-refs`, `MERGE_HEAD` e similares), com observação não recursiva do diretório Git; mudança na árvore de trabalho, com observador recursivo que ignora o que o `.gitignore` ignora (`target/`, `node_modules/`) e o próprio `.git`.
   - Coalescência: debounce curto (algo como 150 ms), nunca dois status em voo, e as escritas do próprio app (stage, commit, checkout) não disparam um segundo refresh além do que o motor já faz.
   - Escala: o observador tem teto de diretórios; se o sistema recusar (limite do inotify) ou a árvore for grande demais, cai para atualizar só no foco e a barra de status diz isso uma vez. Relação com o Spike D: não ligar `core.fsmonitor` nem `untrackedCache` sem consentimento.
   - Nunca escreve no repositório; o status roda fora da thread de UI, com indicador discreto só se passar de ~300 ms.
   - Testes: integração com repositório temporário (editar arquivo, mudar de branch por fora com `git`, commit por fora) esperando o evento de status; o debounce e o filtro de ignorados como funções puras.

Pronto quando: a Working Copy fica no desenho simplificado, toda ação da tela existe no registro e é alcançável por tecla, menu de contexto e paleta, e uma ação desabilitada explica por quê.

## Fatia 2: hook `prepare-commit` (backend feito; interface a fazer)

Backend entregue (`gm-core/src/hook`, `gm-schema/src/hook.rs`; comandos `ListHooks`, `ApproveHook`, `RunPrepareCommit`, `CancelHook`; aprovações em `~/.local/share/gitmaster/hook-approvals.json`, por repositório). Falta a interface (chip e sugestão no composer, aprovação de hook de repo) e fazer os hooks usarem o PATH do shell de login, como o `git` já usa. Revisada em 2026-10-16 (D29, D30): só o que o Git não tem vira hook do GitMaster. O `pre-commit` e os demais hooks nativos ficam com o Git; Q1 decidida (JSON puro).

- Contrato JSON versionado em `gm-schema` (entrada: mensagem atual, diff staged, repo; saída: mensagem), sempre JSON.
- Configuração em `[gitmaster "prepare-commit.<nome>"]` (`command`, `match`, `timeout`), lida do config global e de usuário; hook de repo só roda após aprovação guardada fora do repo, amarrada ao hash do comando (D17).
- Execução assíncrona com id de correlação, timeout e cancelamento; fingerprint do alvo (HEAD, index tree, hash da mensagem) e reconciliação: estado igual aplica, estado mudado vira sugestão.
- Chip no composer (rodando, ok, falhou) com cancelar; sugestão com "Replace" e "Discard", sem sobrescrever o que o usuário digitou.
- Operation log com comando, hash do comando e exit code.

Pronto quando: a mensagem de commit pode ser gerada por um script do autor, sem travar a UI, e o `pre-commit` nativo segue rodando como antes.

## Fatia 3: histórico e branches (visão por nome)

Ordem definida pelo autor em 2026-10-16: esta fatia, junto com a visão de diff (abaixo), é o próximo trabalho depois de terminar a 1c (incluindo a atualização automática) e a interface da fatia 2. O desenho da página de histórico e da visão de diff vem do canvas; o autor traz as pranchas quando começar.

- Página de histórico (destino "History" da sidebar, hoje placeholder): lista de commits com o grafo, detalhe do commit selecionado e os arquivos dele.
- Visão de diff de um commit: a mesma visão de diff da Working Copy (cabeçalho, resumo, hunks), somente leitura, sem botões de stage; navegação entre arquivos do commit.
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
