# Roadmap

Última atualização: 2026-10-17 (fatia 3 dividida em H1 a H8). **Proposta inicial**, para iterar. Marcas: **(feito)** no código e verificado; **(a fazer)** ainda não começado; sem marca, fatia futura. A ordem prioriza chegar cedo a algo que o autor use no dia a dia e que já mostre um diferencial.

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
   - Barra superior (feito, na fatia 1c): nome do repositório com `branch · caminho` embaixo (o seletor de branch continua nessa linha) e, à direita, Fetch, Pull, Push with lease e Restack, todos desabilitados com "Not available yet" até o motor ter cada comando. Sem contador no Restack até existir o modelo de stacks. Sem busca na barra superior, por decisão desta fatia.
   - Faltam do desenho: indicadores de stack (`restack N`, `↑N`, ponto âmbar, PR), a barra de status com escopo, fsmonitor, operation log e MCP.

Pronto quando: troca de projeto e de branch sem sair do app, todo comando Git executado é inspecionável no painel, e abrir o app sem projeto leva a uma tela que explica o que fazer em vez de uma janela vazia.

## Fatia 1c: Working Copy simples, menus contextuais e paleta de comandos (feita, exceto os menus de commit e stash)

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
   - Menu de branch: Checkout, New child branch, Create worktree, Restack this stack, Push with lease, Copy branch name, Rename, Delete. Só Checkout, Copy branch name e Create worktree rodam (esta última desde W3); os demais aparecem desabilitados com "Not available yet".
   - Menu de arquivo: Stage file e Unstage file (cada um desabilitado quando não há o que fazer), Discard changes, Copy file path, Reveal in file manager. Discard changes fica desabilitado: o motor só descarta chunk (`DiscardHunk`), e nenhum comando novo entrou em gm-core.
   - Atalhos do desenho no menu de branch (feito): Checkout Enter, New child branch ⌘B, Create worktree ⌥⌘W, Restack ⇧⌘R, Push with lease ⇧⌘P, Copy branch name ⌥⌘C, Rename F2, Delete ⌫, vindos do registro. Mostrados também nos itens desabilitados: no menu de contexto, o motivo passa a ser uma dica do item. Só ⌥⌘C roda (no branch atual); os demais esperam o motor. Enter no checkout não tem alvo pelo teclado ainda (a sidebar não tem foco de lista).
   - Lacunas: menus de commit e de stash; o menu de branch não abre pelo teclado (a sidebar não tem foco de lista) e não fecha com `Esc`; a confirmação do Discard não é exercitada enquanto o item está desabilitado; Reveal não é coberto pelo smoke (um clique abriria o gerenciador de arquivos) e oferece o caminho mesmo quando o arquivo sumiu do disco; a tecla de menu usa a posição do ponteiro, não a da linha.
4. **Paleta de comandos** (feito; `⌘K` no macOS ainda não testado ali).
   - Feito: `Ctrl+K` abre a paleta; campo único com busca difusa (`fuzzy.rs`, função pura com testes de ordem e de trechos casados); seções "Actions", "Worktrees" e "Branches", cada uma só quando tem resultado (branches só depois de digitar); trecho casado em destaque; contagem de resultados; setas navegam, Enter executa, Tab escolhe o argumento da ação (branch ou arquivo), Esc sai do passo do argumento ou fecha; rodapé com as dicas; lista virtualizada; ações desabilitadas aparecem com o motivo.
   - Roda pelo mesmo caminho da tecla e do menu (`run_action`, `Actor::Human`), com as ações do registro. A ordenação roda fora da thread de UI; `palette.rs` monta as opções e controla o destaque, e só o que chega da ordenação mais recente é mostrado.
   - Lacunas: ações de chunk aparecem desabilitadas com "Pick a chunk in the diff first.", porque rodam do painel de diff; commit e stash continuam sem menu de contexto; a lista de arquivos só se atualiza com o próximo evento de status; o smoke cobre só o teclado (não o clique numa linha, nem Esc no passo do argumento, nem a paleta sem repositório); o destaque é guloso, então em termos com letras repetidas ele pode marcar letras menos óbvias.
5. **Worktrees** (decidido em 2026-10-17, D31; em sub-fatias). Padrões e fora de escopo em D31 (`01`). Cada sub-fatia só habilita as ações dela no registro; as demais seguem desabilitadas com "Not available yet".
   - **W1, listagem** (feito): `git worktree list --porcelain -z` no core (`worktree.rs`, com a forma com quebra de linha como reserva para Git sem `-z`), com caminho, branch ou HEAD solto, checkout principal (primeira entrada), travado com motivo, removível (`prunable`) e bare; a lista vai junto com cada status (`OpenRepo` e `RefreshStatus`) e também responde a `ListWorktrees`. Testes de parser e com Git real.
     Lacunas: a forma de reserva só é coberta pelos testes do parser, porque o Git instalado tem `-z`; o worktree removido à mão só aparece como `prunable` no próximo refresh; a lista ainda não aparece na tela (W2).
   - **W2, seção na sidebar** (feito): "Worktrees" sob a árvore de branches, com a checkout principal primeiro, selo "aqui" no atual, selo "missing" para o que o Git pode podar, cor pelo hash do nome (FNV-1a sobre seis cores do tema). Sem selo de agente (Q12). O watcher observa o registro `worktrees/` do diretório comum, então um worktree criado ou removido pela CLI atualiza a seção sem refresh manual. Testes unitários de ordem, selo e cor; teste de watcher com Git real; cenário `s-worktrees` do smoke, nos dois temas.
     Lacunas: a seção não tem rolagem, então só as seis primeiras linhas aparecem; travar ou destravar pela CLI só aparece no próximo refresh; uma pasta removida à mão vira "missing" só no próximo refresh, porque o watcher não observa a pasta do worktree; o selo "no repo" do desenho não tem equivalente claro no Git (o estado mais próximo é "missing", que usamos); worktree bare não aparece; a cor vem do hash, e a regra de lanes do D25 ainda não existe; "aqui" compara caminhos como o Git os grava, então um caminho por link simbólico pode não casar.
   - **W3, criar** (feito): diálogo com três modos (branch existente, nova branch a partir de um commit, HEAD destacado) e pasta `../<repo>-<branch>` sugerida, que segue o nome até o usuário digitar outra. `git worktree add` roda no motor, fora da thread da interface, e aparece no log de comandos. A pasta não pode existir (o Git aceitaria uma pasta vazia; a recusa é nossa, com a mesma frase do Git). O erro do Git aparece no diálogo, sem resumo. A ação "Create Worktree" está habilitada no registro: menu de branch, paleta (Tab escolhe a branch) e menu de contexto. Só `Actor::Human`. Testes com Git real para cada modo, branch já em uso (a mensagem do Git, igual à do `git worktree add` direto) e pasta existente; testes unitários de validação, pasta sugerida e comando; cenário `t-create-worktree` do smoke, nos dois temas, que cria pela paleta, confere com `git worktree list` e recusa uma branch em uso.
     Lacunas: o diretório base configurável (`gitmaster.worktree.root`, a chave do desenho) ainda não é lido, então a pasta sugerida é sempre irmã do repositório; a branch se digita, sem lista; os modos de nova branch e destacado têm testes com Git real e de validação, mas o smoke não os percorre; sparse checkout, pai de stack e o hook `post-worktree-add` do desenho não estão em D31 e ficam fora desta sub-fatia.
   - **W4, abrir** (feito): duplo clique numa linha da seção abre o worktree numa janela própria. A janela é outro processo deste programa, iniciado com a pasta do worktree, então passa pelo mesmo `OpenRepo` de um caminho na linha de comando; a janela atual não espera por ele. A linha "aqui" não faz nada, e a de pasta sumida avisa na barra de status. Quando o Git recusa uma branch que outro worktree já usa (W3), o diálogo mostra o botão "Open that worktree", que abre esse worktree. Testes unitários da regra de abertura; teste com Git real: o `OpenRepo` do worktree mostra a branch e as entradas que `git -C <worktree> status` mostra, com um arquivo alterado só no worktree.
     Lacunas: nenhum cenário do smoke cobre a abertura (conferido à mão: o duplo clique abriu a segunda janela); a janela nova é um processo a mais, e não uma janela do mesmo processo, porque o motor de cada janela é próprio e a abertura de repositório da janela não tem como trocar de motor; o item "Abrir worktree" do menu do worktree (W8) e a ação da paleta ainda não existem; sem menu de contexto na linha, a abertura só passa pelo duplo clique e pelo botão do diálogo.
   - **W5, remover com checagens** (feito): o menu da linha do worktree (botão direito) tem Abrir, Revelar no gerenciador de arquivos, Copiar caminho e Remover worktree (em vermelho). Remover abre um diálogo que pede ao motor `InspectWorktree`, uma leitura que não muda nada e responde igual para o mesmo estado: lista o que a remoção perderia ou recusaria. São três bloqueios que a força vence (sujo: staged, unstaged ou não rastreado; travado, com o motivo do Git; commits que nenhuma outra branch local ou remoto tem, ou à frente do upstream, o maior dos dois números) e três que ela não vence (checkout principal, a janela atual e pasta sumida). A regra é a de D31: sem força, qualquer bloqueio recusa; com força, o usuário marca uma caixa de confirmação no diálogo, em vez de digitar o nome (mudança decidida em 2026-10-17, ver D31); o motor recebe `force` e refaz a checagem. O motor refaz a inspeção na hora de remover, e roda `git worktree remove` (com um `--force`, ou dois para um travado), nunca `rm -rf`. Só `Actor::Human`. Testes com Git real (`worktree_remove_real_git.rs`): sujo, travado, principal, janela atual, pasta sumida, commit só no worktree contra commit já enviado e worktree novo a partir de main, confirmação sem marcar e marcada, mensagem do Git repassada sem alterar, inspeção idêntica em duas leituras, agente recusado; testes unitários do diálogo (regra do nome, corpo, resultado) e do registro; cenário `u-remove-worktree` do smoke, nos dois temas.
    Lacunas: o desenho mostra "também apagar a branch" e "desfazer"; nenhum dos dois existe aqui (apagar branch é outra ação, e não há log de desfazer). O desenho também mostra comentários de review abertos e o tamanho dos arquivos ignorados: não calculamos nenhum dos dois, porque o diálogo só conta o que o Git enxerga no worktree. Os ignorados são apagados junto, sem aviso de tamanho. A janela atual compara caminhos com link simbólico resolvido, mas o registro (que desabilita "Remover") compara texto. O diálogo tem um campo só, sem a lista de branches. A paleta não lista as ações de worktree (aparecem desabilitadas, com a dica de usar o menu da linha).
   - **W6, travar e limpar** (feito): o menu da linha tem Travar ou Destravar (o que estiver em vigor) e, na linha de pasta sumida, Prune Worktrees. Travar roda `git worktree lock` e destravar roda `git worktree unlock`, ambos no motor, só `Actor::Human`; a recusa do Git (já travado, não travado, caminho que não é worktree) aparece na barra de status. Limpar tem duas etapas: `ListPrunable` roda o dry run (`git worktree prune -n -v`) e o diálogo mostra cada registro com o motivo do Git; `PruneWorktrees` só roda com a mesma lista, e recusa se a lista mudou desde que foi mostrada. Pastas não são tocadas, e worktree travado fica de fora, como o Git faz. Testes com Git real (`worktree_lock_real_git.rs`): motivo mostrado e limpo, sem motivo, travar duas vezes e destravar um não travado (mensagem do Git), caminho não listado e agente recusados, listagem estável, limpeza só dos registros listados, lista que mudou recusada, travado sem pasta fora da listagem e da limpeza, agente recusado na limpeza; testes unitários do diálogo e do registro. A sidebar mostra o selo "locked".
    Lacunas: o travar não pergunta o motivo (o comando aceita um, a interface manda vazio); o diálogo de limpeza não fecha com Escape, porque não tem campo com foco (fecha por Cancelar ou clique fora); "Prune Worktrees" aparece na paleta, onde roda o diálogo sem alvo; a lista mostra o nome do registro (`repo-gone`), não o caminho; travar a checkout principal fica com a recusa do Git, sem teste próprio; a limpeza não tem contagem de registros travados que o Git pulou.
   - **W8, variante do menu de branch** (feito): a branch que está em um worktree vinculado (não na checkout principal) troca o menu: "Open Worktree", "Reveal Worktree in File Manager", "Copy Worktree Path", "New Child Branch", "Restack This Stack" e "Remove Worktree" (em vermelho), sobre o mesmo registro e as mesmas regras de W4 a W6; checkout, renomear, apagar e "Create Worktree" saem do menu. A checkout principal mantém o menu normal. Testes unitários da troca e da ordem dos itens; sem cenário de smoke, por regra desta sub-fatia.
    Lacunas: "Abrir no terminal" do desenho não existe (não há integração com terminal); as ações de worktree não têm atalho no registro, então ⇧⌘C e ⌘⌫ do desenho não aparecem; o menu da branch no seletor do topo não tem a variante, só a linha da sidebar; a variante não aparece na paleta.
   - W7 (caminho do agente, `Actor::Agent`) não entra agora: fica para a fatia 7 (MCP), conforme Q12.

6. **Atualização automática, sem botão de refresh** (feito; desenho confirmado pelo autor em 2026-10-16). O botão saiu da barra superior; a ação "Refresh" segue no registro e na tecla `r`. Lacunas conhecidas: edição externa dentro dos 150 ms após uma escrita do app pode passar; um refresh extra ao abrir; macOS (FSEvents) e árvores perto do teto de 5.000 diretórios sem teste.
   - Premissa: a tela nunca fica velha e o usuário nunca precisa pedir atualização. O botão sai da barra superior só quando isto estiver pronto; a ação "Refresh" continua no registro (tecla `r` e paleta) como saída de emergência.
   - Gatilhos: a janela volta ao foco; mudança em `.git` (HEAD, index, refs, `packed-refs`, `MERGE_HEAD` e similares), com observação não recursiva do diretório Git; mudança na árvore de trabalho, com observador recursivo que ignora o que o `.gitignore` ignora (`target/`, `node_modules/`) e o próprio `.git`.
   - Coalescência: debounce curto (algo como 150 ms), nunca dois status em voo, e as escritas do próprio app (stage, commit, checkout) não disparam um segundo refresh além do que o motor já faz.
   - Escala: o observador tem teto de diretórios; se o sistema recusar (limite do inotify) ou a árvore for grande demais, cai para atualizar só no foco e a barra de status diz isso uma vez. Relação com o Spike D: não ligar `core.fsmonitor` nem `untrackedCache` sem consentimento.
   - Nunca escreve no repositório; o status roda fora da thread de UI, com indicador discreto só se passar de ~300 ms.
   - Testes: integração com repositório temporário (editar arquivo, mudar de branch por fora com `git`, commit por fora) esperando o evento de status; o debounce e o filtro de ignorados como funções puras.

Pronto quando: a Working Copy fica no desenho simplificado, toda ação da tela existe no registro e é alcançável por tecla, menu de contexto e paleta, e uma ação desabilitada explica por quê.

## Fatia 2: hook `prepare-commit` (feita)

Backend (`gm-core/src/hook`, `gm-schema/src/hook.rs`; comandos `ListHooks`, `ApproveHook`, `RunPrepareCommit`, `CancelHook`; aprovações em `~/.local/share/gitmaster/hook-approvals.json`, por repositório) e interface prontos. Revisada em 2026-10-16 (D29, D30): só o que o Git não tem vira hook do GitMaster. O `pre-commit` e os demais hooks nativos ficam com o Git (D18, D29).

Feito:
- Contrato JSON versionado em `gm-schema` (entrada: mensagem atual, diff staged, repo; saída: mensagem), sempre JSON (D30).
- Configuração em `[gitmaster "prepare-commit.<nome>"]`, lida do config global e de usuário; hook de repo só roda após aprovação guardada fora do repo, amarrada ao hash do comando (D17).
- Hooks rodam com o PATH do shell de login, como o `git` (o `login_path` do adapter).
- Execução assíncrona: o cabeçalho da mensagem mostra o estado do run (etapa, pronto, falhou com a razão de uma linha, cancelado) e tem "Cancel" enquanto roda; "Suggest message" o inicia (`commit.suggest_message`).
- Reconciliação (`gm-ui/src/hook_run.rs`): a resposta preenche a mensagem só se HEAD e o diff staged não mudaram e o texto ainda é o enviado; senão vira sugestão, com "Replace" (troca a mensagem) e "Discard" (mantém o texto). O backend não inclui o hash da mensagem no fingerprint, então essa regra fica na UI.
- Aprovação de hook de repo: o painel mostra o comando completo, com "Approve" e "Deny". Deny só esconde o painel nesta sessão; nada é gravado.
- Log de comandos: cada etapa que termina aparece no painel "Git commands", como `hook <nome>: <comando>`, com código de saída, duração e um resultado de uma linha (`ok`, `exit 2`, `timed out after 30000 ms`, `cancelled`). Entrada, saída e diff nunca entram no registro; um teste com Git real confere que o diff e a saída do hook não aparecem. Registrado no thread do run, no mesmo buffer que guarda os comandos Git (D28).
- Testes: reconciliação e ciclo do run como funções puras; regras das ações no registro; cenários de smoke para texto digitado preservado, Replace, falha no chip e aprovação de hook de repo (nos dois temas).

Lacunas:
- O registro de comandos não guarda o hash do comando (a aprovação usa o hash; o log mostra o comando completo). Uma etapa cancelada ou com timeout aparece como falha no botão "Git log".
- O motivo de uma etapa que falhou é só o código de saída: a primeira linha da saída de erro do hook fica no chip, não no log.
- O cenário `p-hook-failure` do smoke abre o painel depois da falha e confere que ele mudou a janela; não lê o texto da entrada.

Pronto quando: a mensagem de commit pode ser gerada por um script do autor, sem travar a UI, e o `pre-commit` nativo segue rodando como antes.

## Fatia 3: histórico e branches (visão por nome) (em andamento: H1 a H4 feitas, H4b-A e H4b-B feitas)

Ordem definida pelo autor em 2026-10-16: esta fatia, junto com a visão de diff (abaixo), é o próximo trabalho depois de terminar a 1c (incluindo a atualização automática) e a interface da fatia 2. Decisões do autor de 2026-10-17 em D32 a D37 (`01`); a fatia é dividida em sub-fatias H1 a H8, cada uma com o próprio "pronto quando". A leitura do histórico é `git log` como subprocess, sem escrever commit-graph (D32).

- **H1, lista real de commits** (feita).
  - Lista plana do histórico de HEAD, os primeiros 200 commits, linhas de 48 px: chips de ref (HEAD, branches, tags), assunto, hash curto e tempo relativo.
  - HEAD sem commits mostra uma mensagem na página, em vez de lista vazia.
  - Ação de navegação "Show History" no registro; a sidebar abre a página.
  - Comando de leitura, sem registro no log de comandos (D28, D33).
  - Pronto quando: a página lista os commits reais de HEAD em repositório linear, com merge, root, HEAD sem commits, HEAD destacado e assunto em Unicode (testes com Git real), e o cenário `h-history` do smoke passa nos dois temas.
  - Lacunas: o chip não distingue ref local de remota (o `%D` do Git não diz); a lista não atualiza sozinha quando o repositório muda, só ao abrir a página (H2); sem paginação além dos 200; a saída de `git log` é lida de uma vez, sem streaming (D32, H2); sem seleção; "Show History" não tem atalho de teclado; a leitura de HEAD destacado e sem commits tem teste com Git real, mas o smoke só cobre o caso com commits; a lista usa a ordem padrão do Git (por data), não `--topo-order` (H5); o tempo relativo usa a data do autor e é calculado na hora de desenhar.
- **H2, paginação e atualização** (feita).
  - Páginas de 200 commits. Ao rolar perto do fim, a página seguinte é pedida a partir do último commit listado (cursor por oid). O `git log --topo-order` lido em streaming segue de onde parou, sem `--skip`; se o cursor não bate com a leitura em andamento, o processo recomeça e lê até o cursor.
  - Atualização: cada atualização de status do watcher (que segue HEAD e refs) relê o topo enquanto a página está aberta; um commit feito fora do app entra no topo.
  - Seleção por oid: um clique seleciona e destaca a linha; a seleção sobrevive às atualizações da lista.
  - Leitura em thread própria, sem bloquear a UI. A resposta de um pedido superado é descartada pelo `OperationId`; o processo git é parado quando o pedido é superado ou o repositório é trocado. Nada é escrito no commit-graph nem registrado no log de comandos (D28).
  - Pronto quando: um repositório de 5 mil commits, com merges e gerado com `git fast-import`, lista página a página exatamente o que `git rev-list --topo-order HEAD` lista, sem repetição nem buraco; um commit externo entra no topo com a seleção mantida; a resposta de um pedido superado é descartada (teste de engine); o cenário `h-history` do smoke passa nos dois temas, junto com `g-switch-branch`.
  - Lacunas: a atualização relê só a primeira página (200), então as linhas além dela voltam ao rolar; cada atualização de status relê o topo, inclusive quando só o working tree mudou, porque o status não diz o que mudou; a seleção não tem detalhe (H3); "Show History" segue sem atalho de teclado (nenhuma tecla livre e sensata no registro); a paginação ao rolar tem teste de modelo e de engine, mas não tem teste de interface nem de smoke com 5 mil commits (o gate de desempenho é H8).
- **H3, detalhe do commit** (feita).
  - Painel à direita da página de histórico, com 360 px: assunto, corpo completo (parágrafos preservados), hash completo, autor com nome e e-mail, data do autor, pais (curtos) e a lista de arquivos com a letra de status (A, M, D, R; renomeado mostra `antigo → novo`) e as contagens de + e −. Binário mostra `bin`. Lista acima de 200 arquivos mostra "and N more". Sem seleção, mostra "Select a commit to see its details."; falha mostra a mensagem do próprio Git.
  - Em merge, a lista compara com o primeiro pai (`--diff-merges=first-parent`); o commit raiz lista tudo como A.
  - Leitura em thread própria, pelo mesmo modelo de pedido e `OperationId` do H2: uma seleção nova para a leitura em curso, que é parada (o processo git é morto), e a resposta antiga é descartada. Trocar de repositório também encerra a leitura. Leitura de comando, sem registro no log de comandos (D28).
  - Pronto quando: os testes com Git real cobrem adição, modificação, remoção, renomeação, binário, commit raiz, merge (primeiro pai), corpo com vários parágrafos, Unicode e o limite de arquivos; o motor descarta a resposta de um pedido superado e para a leitura; e o cenário `h-commit-detail` do smoke passa nos dois temas, junto com `h-history`.
  - Lacunas: a saída de `git show` é lida inteira na memória antes de o limite de 200 arquivos ser aplicado (sem streaming; um commit com dezenas de milhares de arquivos ainda custa essa leitura); o hash e os pais quebram em duas linhas no painel de 360 px; a data aparece no fuso do autor, sem tempo relativo; as linhas de arquivo não são clicáveis (H4); o painel não tem teste de interface, e o smoke confere pixels, não texto (o texto foi lido nas capturas); o cancelamento que mata um `git show` em curso não tem teste com Git real (há o teste de leitura já parada e o do motor); nomes que não são UTF-8 aparecem com caracteres de substituição; clicar de novo na linha relê o commit; "Intervalo" do desenho não entra, por estar fora do que o H3 pede.
- **H4, diff somente leitura de um commit** (feita).
  - Clicar numa linha de arquivo do painel abre o diff desse arquivo no commit, no lugar da lista e do painel. Cima e baixo vão para o arquivo anterior e o seguinte; Esc ou "Back to commit" volta ao commit. Cabeçalho com caminho e `+ −`, a linha "N chunks, N insertions, N deletions" e os hunks, como na Working Copy, sem botões de stage, de chunk nem o seletor Staged | Unstaged. A numeração de linhas fica, sem ação.
  - Em merge, compara com o primeiro pai; o commit raiz usa `--root`; renomeado sai com `-M` e os dois caminhos; binário mostra "Binary file".
  - O `diff_pane` ganhou um modo somente leitura (`Mode::ReadOnly`, com os controles em `diff_rows`), em vez de um componente novo. Na prancha, a visão é a da Working Copy menos as ações; o autor pode trazer a prancha depois.
  - Leitura em thread própria, pelo modelo de pedido do H2 e do H3: um arquivo novo, ou um commit novo, para a leitura em curso, que é parada, e a resposta antiga é descartada. Leitura de comando, sem registro no log de comandos (D28).
  - Pronto quando: os testes com Git real cobrem adição, modificação, remoção, renomeação com edições, binário, commit raiz, merge, troca de modo, arquivo grande e conteúdo fora de UTF-8; o motor descarta a resposta de um pedido superado, para a leitura e não emite nenhum stage; o cenário `h-commit-diff` do smoke passa nos dois temas, junto com `h-history`, `h-commit-detail`, `a-stage-lines`, `i-stage-chunk` e `j-discard-chunk`.
  - Lacunas: "Back to commit" não para a leitura em curso, que termina sozinha e tem a resposta descartada; um nome de arquivo que não é UTF-8 sai com caracteres de substituição, e o diff sai vazio porque o caminho não volta ao Git; a troca de modo, sem conteúdo, mostra "No text changes in this file.", igual a um arquivo vazio; Cima e Baixo só percorrem os arquivos listados, sem os "and N more"; a saída inteira de `git show` fica na memória, como no H3; o smoke cobre adição e remoção, não binário, renomeação, raiz nem merge (esses têm teste com Git real, não de interface).
- **H4b-A, diff dividido do commit** (feita).
  - Seletor "Unified | Split" no cabeçalho do diff do commit, com a escolha salva por usuário em `settings.json` no diretório de configuração (D39, D40). Padrão: Unificado.
  - Dividido: uma linha por par de linhas do lado antigo e do novo (`stage_math::pair_hunk`, o mesmo pareamento do aviso de par do D24), com o lado que não tem linha em painel simples, sem número nem marcador. A calha de cada lado marca só a linha desse lado; o aviso de meio par segue valendo; um conjunto de botões de chunk na linha de cabeçalho do hunk, na largura toda; sem quebra de linha; rolagem lateral compartilhada entre os dois lados, por roda com componente horizontal (D41).
  - Cursor, navegação e seleção seguem as linhas unificadas, mapeadas para a linha dividida que as contém; trocar de layout mantém a linha do cursor.
  - Sem troca automática para Unificado em janela estreita (D41).
  - Pronto quando: os testes de pareamento (2 por 3, adição pura, remoção pura, linhas intercaladas, contexto) e a propriedade de que cada linha alterada aparece uma vez em um lado passam; os testes com Git real de adição, remoção e renomeação com edição passam; o cenário `h-commit-diff-split` do smoke passa nos dois temas, com a escolha mantida depois de reiniciar o app; e os cenários `h-history`, `h-commit-detail`, `h-commit-diff`, `a-stage-lines`, `i-stage-chunk` e `j-discard-chunk` seguem verdes.
  - Lacunas: não há atalho de teclado para trocar o layout; a rolagem lateral é só pela roda, com passo fixo de 4 colunas e sem medir pixels; o limite de rolagem é a linha mais larga do lado, não a largura visível, então dá para rolar além do fim do texto; a largura de tabulação usa paradas de 4 colunas, mas o desenho de um tab cru fica por conta do gpui, igual ao modo unificado; as células divididas e as linhas unificadas têm funções de desenho separadas, então a mudança de visual de uma exige conferir a outra; a escolha é gravada só pelo cabeçalho (a tela de configurações é a fatia 9, D38); o arquivo é lido na inicialização, na thread da interface, por ser pequeno; o smoke confere pixels e foi lido nas capturas (a visão do arquivo adicionado foi conferida só por pixels); o destaque dentro da linha fica para depois da fatia 3 (D42); não há teste de interface do painel dividido, só do modelo e do Git real.
- **H4b-B, diff dividido da Working Copy** (feita, com lacunas).
  - O mesmo seletor e o mesmo modelo de linhas no diff da Working Copy. A escolha é compartilhada: trocar o layout em um dos dois diffs muda os dois e grava a escolha (D39, D40). As ações de stage, de unstage e de descarte seguem as mesmas, e o pareamento de meio par do D24 segue valendo nas duas visões.
  - Cada célula dividida tem a caixa da própria linha, e só a célula da linha do cursor fica destacada. As teclas são as do unificado: j e k percorrem as linhas alteradas nas duas visões, então um par −/+ leva o cursor a cada uma das suas células em sequência; espaço, s e u agem como antes. Não entram h nem l (D41 não pede um atalho próprio).
  - Também fecham lacunas do H4b-A: o segundo Down pressionado enquanto um arquivo do commit carrega não se perde (o painel fica na árvore, vazio até o arquivo aparecer), e a marca "\ No newline at end of file" é desenhada depois do texto da linha, nos dois layouts, sem linha extra (conferida só no código; nenhum cenário do smoke a desenha).
  - Ao gravar `settings.json`, o app mantém as chaves que esta versão não usa; um arquivo corrompido é trocado por um válido (D40).
  - Pronto quando: os testes de seleção pela célula e pela linha unificada dão o mesmo resultado, com Git real o índice gravado é o mesmo; o cenário `g-stage-split` do smoke passa nos dois temas, com o stage de uma linha pelo checkbox da célula, a segunda tecla de s no meio par (D24), o Stage Chunk e a conferência com o git; os cenários `a-stage-lines`, `i-stage-chunk` e `j-discard-chunk` passam nos dois temas com o layout unificado e com o dividido ligado (`-in-split`); e os cenários `h-history`, `h-commit-detail`, `h-commit-diff` e `h-commit-diff-split` seguem verdes.
  - Lacunas: não há atalho de teclado para o layout; sem h e l, não há como pular de uma célula para a outra de um par; não há teste de interface do painel dividido (o smoke confere git e pixels); o recuo horizontal volta a zero ao trocar de arquivo ou de lado, sem teste unitário, porque o estado está na entidade do GPUI; o aviso de meio par do `g-stage-split` é conferido por pixels na barra do rodapé, não pelo texto; a marca de fim de arquivo tem teste só da função que devolve o texto, e nenhum cenário de smoke a desenha, porque a fixture não tem arquivo sem quebra de linha; intra-line segue para depois da fatia 3 (D42); a escolha segue sem tela própria (fatia 9, D38).
- **H5, grafo com lanes.** Layout puro e determinístico, com `--topo-order`; cor da lane pelo hash do nome da branch (D36, D25).
- **H6, escopo.** Pílula com branch atual, todas as refs ou uma ref. Clique esquerdo numa branch da sidebar, na página de histórico, define o escopo; o checkout fica no duplo clique ou no menu de contexto. "Stack atual" aparece desabilitado com "Not available yet" (D34).
- **H7, busca por conteúdo.** `git log -S` de texto fixo; o campo fica no cabeçalho do histórico, não na barra superior; com atraso e cancelável (D35).
- **H8, gate de performance.** Benchmarks ignorados em repositórios de 50 mil e 200 mil commits, gerados com `git fast-import` (D37).

Fora da fatia 3 (decisão do autor em 2026-10-17): escopo de stack, dono do commit, pai de stack, aresta tracejada, aba Stacks, contador de Restack, Fetch e Pull, reflog, stashes e Review local. Os itens de stack ficam "Not available yet".

Pronto quando: navega o histórico de um repositório grande com rolagem fluida.

## Fatia 3c: navegação no código (proposta, depende de Q13)

Detalhes, motores e riscos em `09-navegacao-no-codigo.md`; decisões D43 a D50 em `01`, todas Proposto. Página Code com Quick Open de arquivos, grep e símbolos, sobre um visualizador somente leitura. Sub-fatias, cada uma com o "pronto quando" em `09`:

- **Spikes S-A, S-B e S-C** (listas e grep em escala, tree-sitter e ast-grep, casador de caminhos), antes de construir.
- **N1, Quick Open** de arquivos do worktree ativo.
- **N2, visualizador e árvore**, com "View file" a partir do diff e do detalhe do commit.
- **N3, grep** com `git grep`.
- **N4, destaque de sintaxe** (depende do S-B).
- **N5, símbolos e caixa única**: índice por blob, classificador da caixa única (D50), outline, ir para a definição, usos classificados.
- **N6, revisões, tools Read do MCP e gate de performance.**

Pronto quando: abrir um arquivo pelo nome, achar um texto e saltar para uma definição leva segundos num repositório de 100 mil arquivos, sem a UI travar.

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

Decidido pelo autor em 2026-10-17 (D38): a tela de configurações entra nesta fatia, consolidando tudo o que o aplicativo permitir configurar até este momento.

1. **Tela de configurações (Settings modal, D38)**
   - Diálogo modal centralizado no GPUI com backdrop semitransparente escurecido (`black/50`), fechável com tecla `Esc`, clique no backdrop ou botão de fechar.
   - Navegação em duas colunas: barra lateral de categorias à esquerda (largura fixa ~200 px) e painel de formulário à direita com rolagem independente.
   - Acesso: atalho `⌘,` (macOS) / `Ctrl+,` (Linux/Windows), ação tipada `open_settings` no registro (`action_registry.rs`), paleta de comandos (`Open Settings`) e botão de engrenagem no rodapé.
   - Persistência reativa imediata (sem botão "Salvar"), respeitando a regra estrita de ficar fora do working tree (Princípio 5):
     - `git config`: alterações de identidade, branch padrão, worktree root e hooks vão para `~/.gitconfig` (global) ou `.git/config` (local) via `git config`.
     - `settings.json`: preferências de tema, fontes, recentes e MCP vão para o diretório de configuração do usuário (`~/.config/gitmaster/`).
     - `hook-approvals.json`: lista de autorizações de scripts por hash no diretório de dados (`~/.local/share/gitmaster/`).
   - Seções abrangentes cobrindo as fatias 1 a 8:
     - *General:* aparência (`System`, `Dark`, `Light`), fontes de UI e código, limpeza de projetos recentes, limite de arquivos do observador de status.
     - *Git & Identity:* seletor de escopo (Global vs Este Repositório), `user.name`, `user.email`, `init.defaultBranch`, `core.editor`, assinatura de commits.
     - *Worktrees:* diretório raiz padrão (`gitmaster.worktree.root`, D31) e opções de proteção na remoção segura.
     - *Diff & Files:* algoritmo de diff (`histogram`, `myers`, `patience`), linhas de contexto (`diff.context`), ignorar espaços por padrão.
     - *Hooks & Security:* catálogo de hooks ativos e tabela de autorizações em `hook-approvals.json` com botão "Revoke".
     - *Stacks & Restack:* estratégia padrão de push com lease e política para restack com working copy sujo (Q2).
     - *Conflict Resolver:* estilo padrão (`merge.conflictStyle = zdiff3`) e reutilização com rerere.
     - *MCP & Agents:* controle do servidor interno, socket, gestão de tokens e matriz de autorização para ações de `Actor::Agent` (D20).
     - *Local Review:* localização de armazenamento de comentários e snapshots.
   - Cenário de smoke no `ui-smoke.sh` cobrindo a abertura pelo atalho, troca de tema e fechamento por `Esc`.

2. **Performance em monorepo**: profiling e otimizações de diff, status e revwalk em repositórios massivos.
3. **Layouts alternativos**: layout unificado/empilhado para telas menores (Q9).
4. **Empacotamento e distribuição**: empacotamento nativo (macOS, Linux), metadados de desktop, ícones de `08`.
5. **Documentação e modelo de produto**: documentação de usuário final, termos de licença e modelo de produto premium (Q10).

Pronto quando: todas as opções do aplicativo e do ambiente Git podem ser inspecionadas e ajustadas visualmente no modal de configurações sem necessidade de editar arquivos de configuração manualmente, e o binário é empacotado para instalação nativa.

## Riscos transversais

- GPUI: texto editável, ecossistema de componentes e licenças.
- Escala: status, grafo e diff em repositórios massivos.
- Escopo: Tower tem pull requests, worktrees, submódulos e LFS. Worktrees entram na v1 (D31); pull requests, submódulos e LFS seguem em aberto (Q8).
