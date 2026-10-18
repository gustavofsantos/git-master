# Decisões e questões abertas

Última atualização: 2026-10-17

## Como ler o status

- **Decidido**: o autor afirmou explicitamente.
- **Proposto**: sugerido durante os exercícios de design e não contestado. Precisa de confirmação antes de virar compromisso.
- **Aberto**: sem resposta ainda (ver a seção de questões).

## Log de decisões

| ID | Decisão | Status | Doc |
|---|---|---|---|
| D01 | Implementação em Rust com GPUI, nativo; sem Tauri nem front-end JS | Decidido | 06 |
| D02 | Produto local, de uso por engenheiro; sem modo headless, CI ou servidor | Decidido | 00 |
| D03 | Cada branch tem um único pai e vários filhos (árvore, não DAG) | Decidido | 02 |
| D04 | Hooks seguem a ideia dos hooks do Claude Code, com schema próprio de entrada e de saída | Decidido | 03 |
| D05 | Hooks executam de forma assíncrona; a UI nunca bloqueia por causa de um script lento | Decidido | 03 |
| D06 | Hook recebe um id de correlação na entrada e devolve o mesmo id na saída; o motor reconcilia | Decidido | 03 |
| D07 | Hook responde pelo stdout (sem modelo detached/callback) | Decidido | 03 |
| D08 | Motor único com dois front-ends (GPUI e MCP) | Decidido | 06 |
| D09 | Evitar previsões não determinísticas | Decidido | 00 |
| D10 | Sidebar com visão dupla: por stack e por nome | Decidido | 05 |
| D11 | Loop de review local (comentários, agente via MCP, devolutiva, antes/depois) | Proposto | 04 |
| D12 | Servidor MCP dentro do app; shim `gitmaster mcp` por socket Unix com token | Proposto | 04 |
| D13 | Metadata de stack no git config: `stackParent` e `stackParentBase` | Proposto | 02 |
| D14 | Restack com plano exato por commit, snapshot de refs, undo atômico e estado persistido | Proposto | 02 |
| D15 | Push com lease como passo explícito após o restack | Proposto | 02 |
| D16 | Hooks em três classes (gate, transform, observe); gates e observers em paralelo, transforms em sequência | Proposto | 03 |
| D17 | Hooks de repo exigem aprovação (hash do comando); padrão lê só config global/usuário; `includeIf` por projeto | Decidido (2026-10-16) | 03 |
| D18 | Hooks nativos do Git continuam rodando, antes dos do GitMaster; o GitMaster não os reimplementa nem os envolve (ver D29) | Decidido (2026-10-16) | 03 |
| D19 | ~~`pre-commit` roda contra a index tree~~ Substituída por D29: o `pre-commit` é do Git | Substituída (2026-10-16) | 03 |
| D20 | MCP em tiers Read, Propose, Aprovação, Apply; apply só aceita `plan_id` | Proposto | 04 |
| D21 | IDs de hunk determinísticos (hash do conteúdo) | Proposto | 04 |
| D22 | Resolvedor de conflitos em três painéis, lados rotulados por papel (não ours/theirs) | Proposto | 05 |
| D23 | Working Copy: "staged" e "lido" como eixos ortogonais; lista única com checkbox tri-state | Decidido (2026-10-16) | 05 |
| D24 | Stage por linha sobre modelo de diff exato, escrevendo o blob do index direto | Decidido (2026-10-16) | 05 |
| D25 | Histórico: defasagem de stack como aresta tracejada; dono do commit pela metadata do stack | Proposto | 05 |
| D26 | Backend Git híbrido: subprocess `git` para toda operação com efeito colateral nativa do Git (stage, unstage, commit, checkout), para status em escala e para a leitura do histórico (`git log`, ver D32); libgit2 para o restack (que não existe no Git) e as demais leituras. Revisado em 2026-10-16 e 2026-10-17 | Decidido (2026-10-16) | 06 |
| D28 | O log de comandos Git registra só operações com efeito colateral; leituras (status, diff, histórico) não aparecem | Decidido (2026-10-16) | 07 |
| D29 | O GitMaster não mexe no que é nativo do Git: `pre-commit` e os demais hooks nativos ficam com o Git (rodam via `git commit`, e a saída aparece como erro do commit). Só o que o Git não tem é hook do GitMaster; a fatia 2 entrega só o `prepare-commit` | Decidido (2026-10-16) | 03 |
| D30 | Contrato de hooks em JSON puro (Q1, opção A): sem `mode = text`, sem codec no host | Decidido (2026-10-16) | 03 |
| D27 | Começar com quatro crates (schema, core, ui, mcp) e extrair o resto sob demanda | Proposto | 06 |
| D31 | Worktrees entram na v1 (núcleo do fluxo do autor); padrões e fora de escopo na seção D31 abaixo | Decidido (2026-10-17) | 05, 07 |
| D32 | A leitura do histórico roda `git log` como subprocess: faz streaming da saída, usa o commit-graph que já existe e nunca o escreve. Corrige D26 (que dizia libgit2 para leituras em massa) | Decidido (2026-10-17) | 06, 07 |
| D33 | Lista do histórico (H1): lista plana de commits de HEAD, os primeiros 200, linhas de 48 px com chips de ref, assunto, hash curto e tempo relativo. HEAD sem commits mostra uma mensagem. A leitura é um comando sem registro no log de comandos (D28) | Decidido (2026-10-17) | 07 |
| D34 | Escopo do histórico (H6): pílula com três modos, branch atual, todas as refs ou uma ref. Clique esquerdo numa branch da sidebar, na página de histórico, define o escopo; o checkout fica no duplo clique ou no menu de contexto. "Stack atual" aparece desabilitado com "Not available yet" | Decidido (2026-10-17) | 05, 07 |
| D35 | Busca por conteúdo (H7) com `git log -S` de texto fixo. O campo fica no cabeçalho do histórico, não na barra superior; a busca é com atraso (debounce) e cancelável | Decidido (2026-10-17) | 05, 07 |
| D36 | Cor das lanes do grafo (H5, D25) deriva de um hash do nome da branch, estável entre sessões. O layout é determinístico e usa `--topo-order`. D25 segue Proposto no restante (aresta tracejada, dono do commit) | Decidido (2026-10-17) | 05, 07 |
| D37 | Gate de performance do histórico (H8): benchmarks ignorados em repositórios de 50 mil e 200 mil commits, gerados com `git fast-import` | Decidido (2026-10-17) | 06, 07 |
| D38 | Tela de configurações como diálogo modal na Fatia 9; configura tudo o que o app permitir até lá, persistindo fora do working tree (Princípio 5) | Decidido (2026-10-17) | 05, 07 |
| D39 | Diff com layout Unificado | Dividido (H4b): o seletor vale para o diff do commit e, na sub-fatia H4b-B, para o diff da Working Copy, com um só modelo de linhas. Este passo entrega o do commit primeiro | Decidido (2026-10-17) | 05, 07 |
| D40 | Preferência de layout do diff guardada por usuário em `settings.json` no diretório de configuração do usuário (`~/.config/gitmaster/` no Linux, `Library/Application Support/gitmaster/` no macOS), fora do repositório (Princípio 5). Padrão: Unificado. Arquivo ausente ou inválido vale como Unificado. A tela de configurações (D38) passa a expor a opção | Decidido (2026-10-17) | 05, 07 |
| D41 | Modo Dividido: a calha de cada lado marca só a linha desse lado (o par não é unidade); o aviso de par do D24 segue valendo para meio par; um conjunto de botões de chunk, na linha de cabeçalho do hunk na largura toda; sem quebra de linha e com deslocamento horizontal compartilhado; lado vazio é painel simples, sem número nem marcador; sem troca automática para Unificado em janela estreita | Decidido (2026-10-17) | 05, 07 |
| D42 | Destaque de diferença dentro da linha (intraline) fica adiado para depois da Fatia 3 | Decidido (2026-10-17) | 07 |
| D43 | Navegação no código (`09`): página Code com Quick Open de arquivos, grep e símbolos, sobre um visualizador somente leitura do worktree ativo; revisões quaisquer só no N6 | Proposto | 07, 09 |
| D44 | Grep com `git grep` como subprocess (mesmo argumento do D32: já instalado, respeita `.gitignore`, lê blobs de qualquer revisão, faz streaming, cancelável). Padrão sempre depois de `-e`, pathspec depois de `--`. Se o benchmark pedir, a alternativa é busca em processo com crates Rust, não outro programa (D51) | Proposto | 09 |
| D45 | Símbolos com tree-sitter compilado no binário (`tags.scm`) para um conjunto curado de linguagens; índice endereçado pelo OID do blob, derivado e descartável, fora do working tree. "Find usages" é `git grep -w` e a interface diz que é texto. Confirmar no spike S-B (compilação, binário, qualidade) | Proposto | 09 |
| D46 | Lista de arquivos de `git ls-files -z` (ou `ls-tree -r` numa revisão) em buffer contíguo, atualizada pelo watcher; casador de caminhos próprio (bônus de nome de arquivo, top-K paralelo), separado do `fuzzy.rs` das ações | Proposto | 09 |
| D47 | Fora do escopo: edição, LSP e referências semânticas, índice de texto próprio (estilo Zoekt) | Proposto | 09 |
| D48 | Tools Read do MCP (`files_search`, `code_grep`, `symbols_search`, `file_read`) entram na Fatia 7, sobre os mesmos comandos do motor que a UI usa | Proposto | 04, 09 |
| D49 | Gate de performance da navegação (N6): benchmarks ignorados em repositórios de 100 mil e 1 milhão de arquivos gerados com `git fast-import`; metas a calibrar nos spikes | Proposto | 09 |
| D50 | Caixa única de busca, sem sintaxe: um classificador determinístico por regras escolhe os motores (arquivos, símbolos, usos, texto), que rodam em paralelo e entram em seções de ordem fixa. Sem LLM no caminho; filtros são só atalho opcional | Proposto | 09 |
| D51 | O GitMaster é autocontido: a navegação não exige nenhum programa além do `git`, que o app já exige. Sem ripgrep, ctags, ast-grep nem LSP como executáveis; motores extras entram só como bibliotecas Rust compiladas no binário | Proposto | 09 |

## D31: worktrees na v1

O autor decidiu em 2026-10-17 que worktrees entram na v1, por serem parte central do fluxo dele. Os padrões abaixo ficam registrados como decididos; mudam só por nova decisão do autor. Alteração decidida pelo autor em 2026-10-17: a confirmação da remoção forçada passou de digitar o nome para marcar uma caixa de confirmação (ver o item "Remoção segura").

- **Destino:** um novo worktree vai para um diretório base configurável, cujo padrão é irmão do repositório: `../<repo>-<branch>`.
- **Remoção segura:** remover worktree sujo, com commits não enviados ou travado é bloqueado por padrão. A remoção forçada só roda depois de marcar a caixa de confirmação do diálogo (rótulo em inglês, que nomeia o que será descartado), que fica desmarcada até o usuário marcar; o botão Remover fica desabilitado até lá. Checkout principal, janela atual e pasta sumida continuam recusados, com ou sem a caixa. Alterado em 2026-10-17 (antes: digitar o nome do worktree). O motor refaz a checagem e recusa a remoção forçada sem a confirmação.
- **Uma janela por worktree:** cada worktree abre na própria janela, em linha com um repositório por janela (Q11).
- **Checkout principal primeiro:** a checkout principal aparece como a primeira linha da seção "Worktrees" da sidebar.
- **Branch já em uso:** fazer checkout de uma branch que já está em outro worktree mostra o erro do Git e a ação "Ir para esse worktree".
- **Cor:** a cor do worktree deriva de um hash do nome, estável entre sessões (mesma regra das lanes do D25).
- **Fora da v1:** submódulos, sparse checkout e repositórios bare.
- **Sem agente:** esta decisão não inclui integração de worktrees com agentes. Não há tag de agente na lista, nem caminho `Actor::Agent` para worktrees; ver Q12.

As ações de worktree no registro seguem desabilitadas com "Not available yet" até a sub-fatia correspondente da fatia 1c (item 5 de `07`) entrar. Criar (W3), abrir (W4), remover (W5), travar, destravar e limpar (W6) e a variante do menu de branch (W8) já estão.

## Decisões da fatia 3 (histórico)

Detalhes de cada sub-fatia (H1 a H8) em `07`.

Fora da fatia 3 (decidido pelo autor em 2026-10-17, sem ID próprio): escopo de stack, dono do commit, pai de stack, aresta tracejada, aba Stacks, contador de Restack, Fetch e Pull, reflog, stashes e Review local. Os itens de stack ficam "Not available yet".

## D38: tela de configurações (Settings)

O autor decidiu em 2026-10-17 que a tela de configurações será construída na **Fatia 9 (acabamento premium)** e terá formato de **diálogo modal**. Quando implementada, ela cobrirá tudo o que o aplicativo permitir configurar até aquele momento.

- **Formato modal:** diálogo centralizado sobre a janela atual com backdrop escurecido (`black/50`), fechável com tecla `Esc`, clique no backdrop ou botão de fechar. Layout com coluna de navegação lateral por categorias à esquerda e formulário de campos tipados à direita.
- **Acesso:** atalho `⌘,` (macOS) / `Ctrl+,` (Linux/Windows), ação tipada `open_settings` no registro de ações (`action_registry.rs`), paleta de comandos (`Open Settings`) e botão/ícone no rodapé ou cabeçalho.
- **Persistência estrita fora do working tree (Princípio 5):**
  - Git config: alterações de identidade, branch padrão, worktree root e hooks vão para `~/.gitconfig` (global) ou `.git/config` (local), com seletor explícito de escopo.
  - App settings: preferências visuais e comportamentais da aplicação vão para `settings.json` no diretório de configuração do usuário (`~/.config/gitmaster/` no Linux).
  - Segurança e autorizações: lista de aprovações de hooks de repositório em `hook-approvals.json` no diretório de dados do usuário (`~/.local/share/gitmaster/`).
  - Sem botão "Salvar": gravações imediatas ao alterar ou perder o foco do campo, com validação inline.
- **Escopo abrangente na Fatia 9:**
  - *General:* tema (System/Dark/Light), fontes de UI e código, limpeza de projetos recentes, limite de arquivos do observador de status.
  - *Git & Identity:* `user.name`, `user.email` (Global vs Repositório), `init.defaultBranch`, `core.editor`.
  - *Worktrees:* raiz padrão de novos worktrees (`gitmaster.worktree.root`), regras de remoção segura (D31).
  - *Diff & Files:* algoritmo padrão (`histogram`, `myers`, `patience`), linhas de contexto, ignorar espaços por padrão.
  - *Hooks & Security:* catálogo de hooks ativos, gestão de aprovações de scripts com hash e botão de revogação.
  - *Stacks & Restack:* políticas de restack com repo sujo (Q2), push com lease padrão (D15).
  - *Conflict Resolver:* estilo de conflito (`merge.conflictStyle`), rerere.
  - *MCP & Agents:* ativação do servidor MCP, caminho do socket, tokens, políticas de autorização para ações de agentes (D20).
  - *Local Review:* armazenamento de comentários e snapshots locais (D11).

## Questões abertas

- **Q1. Contrato de hooks para shell:** (A) JSON puro; (B) pipe de texto; (C) JSON canônico com `mode = text` opcional por hook (codec no host). **Decidido (2026-10-16): A**, ver D30.
- **Q2. Restack com working copy sujo:** bloquear com banner ou autostash determinístico registrado no operation log. Começar pelo bloqueio.
- **Q3. Review local:** confirmar a inclusão no produto e onde guardar comentários e snapshots (SQLite em `.git/` ou diretório de dados do app).
- **Q4. Nomes das chaves de config:** `gitmaster.*` ou espelhar o formato de hooks por config do Git recente (`hook.<nome>.command`, a verificar).
- **Q5. Hook `split-commits`:** proposta de agrupar hunks em vários commits, confirmada pelo humano. Ideia para depois da base.
- **Q6. libgit2 versus gitoxide:** checar maturidade de status, merge e rebase em memória antes de fechar D26. **Spike C (2026-10-16, `spikes/c-rebase`):** o rebase em memória do libgit2 bate com `git rebase --onto` em 12 cenários (stack limpa, conflito com os três lados, rename, merge commit, binário, modo, 3000 arquivos, pai amendado) sem tocar index nem working tree; 200 commits em 135 ms (contra 2377 ms de `git merge-tree` por commit). Commit que fica vazio exige tratar `Applied` como skip. `merge-tree --merge-base` exige git 2.40+. Não testados: rerere, hooks de rebase, assinatura, submódulos. Gitoxide não foi testado. **Spike D (`spikes/d-status`):** status por subprocess é 2,5x mais rápido que o libgit2 a 500 mil arquivos e `--no-optional-locks` evita quebrar commits concorrentes; confirma o status por subprocess de D26. fsmonitor sem veredito (daemon indisponível nesta máquina).
- **Q7. Texto editável no GPUI:** o painel de resultado do resolvedor e a visualização de diff exigem texto editável e virtualizado. Avaliar bibliotecas de terceiros (ex.: gpui-component) e licenças (verificar a licença dos crates do Zed além do `gpui`). **Spike B (2026-10-16, `spikes/b-text`):** portar o text area do rvw (do mesmo autor, MIT; 55 testes passam, janela abre; mouse, IME, clipboard e wrap só lidos do código, falta teste manual). O editor do Zed é GPL-3.0-or-later; `gpui-component` é Apache-2.0 mas depende de outra versão do gpui. O texto inteiro é shaped a cada frame: serve ao composer, não ao painel de conflitos. Decisão do autor (2026-10-16): o GitMaster será todo open source e o código do rvw pode ser usado.
- **Q8. Escopo de Tower sem decisão:** pull requests, submódulos e LFS. Worktrees foram decididos em 2026-10-17 (D31). Para worktrees, submódulos ficam fora da v1; a pergunta sobre suporte geral a submódulos segue aberta.
- **Q9. Layout alternativo do resolvedor** (unificado/empilhado) para telas pequenas.
- **Q11. Fatia 1b (resolvida em 2026-10-16):** (a) checkout com working copy sujo: repassa o erro do Git, sem autostash nem "levar as mudanças". (b) Um repositório por janela na v1. (c) O log de comandos fica só em memória na v1.
- **Q10. Licença e modelo de produto:** como combinar open source com "produto premium" (open core, binário pago com fonte aberta, etc.).
- **Q12. Worktrees e agentes:** um agente criar, marcar ou remover worktrees (tag de agente na lista, `Actor::Agent`, ações pelo MCP) não faz parte de D31. Decidir junto com a fatia 7 (MCP), com aprovação do humano pela UI.
- **Q13. Navegação no código (`09`):** isto entra no produto e em que posição do roadmap (Fatia 3c depois de H8, ou depois da 5)? É escopo além do Tower, então precisa do aval do autor antes do N1.
- **Q14. Motor de símbolos:** tree-sitter embutido (D45) ou só nome de arquivo e texto na primeira versão? Decidir com os números do spike S-B (tempo de compilação, tamanho do binário em Linux e macOS, qualidade das consultas). Também: quais linguagens vêm na primeira onda e onde fica o índice (`.git/gitmaster/` ou diretório de dados do app).
