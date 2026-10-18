# Telas

Última atualização: 2026-10-16. Status de cada decisão em `01`. Os mockups interativos foram feitos na conversa de design de 2026-10-16; este documento guarda as decisões, não a aparência exata.

Referências visuais de clientes Git desktop maduros: sidebar com árvore de pastas por prefixo, Working Copy com composer de commit acima da lista de arquivos, diff com "Stage/Unstage chunk", menu de contexto de commit.

## Sidebar de branches (D10 decidido)

Duas organizações com alternância ("Por stack" e "Por nome"):

- **Por stack**: árvore indentada por dependência. Seções "Stacks" e "Sem stack", para quem não usa stacking não pagar o custo da feature. Adoção gradual: a ferramenta funciona como um cliente Git comum até a primeira branch ganhar um pai.
- **Por nome**: visão clássica, agrupada por prefixo (`feature/`).

Indicadores:

- Ação no **pai** (`restack 3`), porque é ele que mudou. Filhos desatualizados ganham só um ponto âmbar.
- `↑N`: commits à frente do **pai** (não da origin).
- Arrastar uma branch sobre outra define o pai (mesmo idioma de arrastar commit sobre a sidebar, comum em clientes Git desktop).
- Estado calculado fora da thread de UI.
- Lista virtualizada (`uniform_list` no GPUI), pensando em centenas de branches remotas.

## Fluxo de restack

Quatro etapas: Plano, Executando, Conflito, Concluído. Detalhes em `02`.

## Resolução de conflitos (D22)

Referência: resolvedores de conflito de IDEs. Três painéis: esquerda, resultado (editável), direita.

- **Lados rotulados por papel e não por ours/theirs.** Em rebase, `ours` e `theirs` são invertidos. Exibir: nome da branch, papel ("novo pai") e assunto do commit.
- Aplicar sozinho o que não conflita, mostrando o resultado ("3 mudanças aplicadas").
- Três ações por hunk: esquerda, direita, ambos. Base sob demanda (`merge.conflictStyle = zdiff3`).
- Contador "N de M conflitos resolvidos"; aplicar com conflitos pendentes responde com mensagem, em vez de desabilitar o botão.
- **rerere** para reaplicar resoluções gravadas, indicando "resolvido por resolução gravada" com opção de revisar ou esquecer.
- Hook `resolve-conflict` entra como terceira opção rotulada ("sugestão do script"), nunca aplicada sozinha.
- O operation log registra quem resolveu cada hunk (humano, rerere ou script).
- Plano de layout alternativo (unificado ou empilhado) para telas pequenas (Q9).

## Histórico e grafo (D25)

- A **defasagem de stack aparece no grafo**: aresta tracejada âmbar quando a branch filha nasce de um commit que não é a ponta do pai, com selo "N commits atrás do pai".
- **Dono do commit pela metadata do stack** (intervalo `stackParentBase..tip`), sem heurística por nome de ref. Cor da lane por hash do nome da branch (estável entre sessões); não depender só de cor.
- Dois modos: **Commits** e **Stacks** (cada nó é uma branch com seus commits recolhíveis).
- **Escopo padrão: o stack do HEAD mais `main`**, com chip visível "Escopo: stack atual" e opção "todas as refs". Importante em repos grandes e com muita gente.
- Layout de lanes determinístico, em uma passada de cima para baixo, com checkpoints a cada N linhas para rolar sem recalcular.
- Performance: arquivo `commit-graph` (generation numbers) e changed-path Bloom filters; revwalk em streaming em thread separada.
- Busca por conteúdo (pickaxe, `git log -S` e `-G`).
- Risco: o grafo por commit só é legível com poucas lanes; testar cedo com um repositório grande real.

## Working Copy (D23)

- **Staged e lido são eixos ortogonais.** Staged é "vai para o commit"; lido é "eu revisei". O "lido" se invalida sozinho quando o hash do hunk muda. Serve como lembrete, nunca como selo de aprovação.
- Lista única com checkbox tri-state (indeterminado) por pasta, filtro sempre visível, agrupamento por diretório.
- Composer sempre visível, com o chip do hook `prepare-commit` (sugestão não sobrescreve o que o usuário digitou; botão vira "Substituir"; cancelável). O `pre-commit` é nativo do Git (D29): não ganha chip, e a saída dele aparece como erro do commit.
- Escala: status em streaming ("escaneando…"), `core.fsmonitor`, `core.untrackedCache`, sparse-checkout, lista virtualizada, arquivos gerados e lockfiles recolhidos.
- Teclado primeiro: `j`/`k` navegam, espaço faz stage, `r` marca como lido, `a` faz stage do hunk, `⌘↵` faz commit.

## Diff e stage por linha (D24)

- **Modelo exato, visão filtrada.** O stage opera sempre sobre o diff exato; "ignorar espaços" é só filtro de exibição.
- Mecânica: montar o novo blob do index (base do index mais as linhas selecionadas) e escrever direto com libgit2, de forma atômica. Invariante testável: staged mais unstaged é igual à mudança total.
- Seleção: checkbox por linha na gutter, contador por hunk e modo visual estilo Vim (`V` seleciona, `s` stage, `u` unstage).
- Aviso de **par −/+ staged pela metade** (o index ficaria sem a linha antiga e sem a nova, e pode não compilar).
- Algoritmo `histogram` (ou `patience`) com heurística de indentação; detecção de código movido (`--color-moved`); diff intra-linha só nos hunks visíveis.
- Diff e stage dos arquivos gerados: recolher via atributos do `.gitattributes` (somente leitura).
- Comentários de review em linha, com o antes/depois do agente (ver `04`).

## Tela de configurações (Settings modal, D38)

Prevista para a Fatia 9 (acabamento premium), a tela de configurações consolida a gestão de todas as opções do aplicativo e do ambiente Git em um único lugar, cobrindo tudo o que o app permite configurar até aquele momento.

### Comportamento e layout do modal

- **Formato modal:** diálogo centralizado sobre a janela atual (dimensões ~860x580 px), com backdrop semitransparente escurecido (`black/50`). Fecha ao pressionar `Esc`, clicar fora ou no botão de fechar.
- **Acesso:** atalho `⌘,` (macOS) / `Ctrl+,` (Linux/Windows), ação registrada `open_settings` em `action_registry.rs`, comando na paleta (`Open Settings`) e botão de engrenagem no rodapé.
- **Navegação em duas colunas:**
  - Sidebar esquerda (largura fixa ~200 px): lista de seções com ícones e rótulos em inglês.
  - Painel de conteúdo direito: formulário com rolagem independente, seções agrupadas, campos de entrada tipados, switches e botões de ação com feedback inline.
- **Persistência reativa sem botão "Salvar" (Princípio 5):**
  - Cada alteração é persistida imediatamente ao mudar o valor ou ao desfocar o campo de texto.
  - Gravações respeitam rigorosamente a fronteira de persistência:
    - **Git config:** gravado via `git config` (`~/.gitconfig` para escopo global ou `.git/config` para escopo local do repo).
    - **App settings:** gravado em `settings.json` no diretório de configuração do usuário (`~/.config/gitmaster/` no Linux).
    - **Segurança:** gravado em `hook-approvals.json` no diretório de dados do usuário (`~/.local/share/gitmaster/`).
    - Nenhuma configuração é escrita dentro do working tree do repositório.

### Seções da interface

1. **General:**
   - *Appearance:* seletor de tema (`System`, `Dark`, `Light`) persistido em `settings.json`.
   - *Typography:* seletores das famílias de fonte para interface e código monoespaçado, com visualização prévia e fallbacks instalados.
   - *Recent Repositories:* lista de repositórios recentes abertos com opções para remover entradas específicas ou limpar o histórico.
   - *File Watcher:* teto de diretórios observados pelo watcher e status de fallback de inotify.

2. **Git & Identity:**
   - *Scope Selector:* alternador entre escopo **Global (`~/.gitconfig`)** e **This Repository (`.git/config`)**.
   - *Author Details:* campos para `user.name` e `user.email`.
   - *Default Branch:* nome padrão para novas branches e inicializações (`init.defaultBranch`).
   - *Editor:* executável externo configurado em `core.editor`.
   - *Signing:* indicador de assinatura de commits (`commit.gpgSign`) e chave GPG/SSH.

3. **Worktrees (D31):**
   - *Default Location:* diretório base para novos worktrees (`gitmaster.worktree.root`, com padrão `../<repo>-<branch>`).
   - *Safety Rules:* confirmações e regras de segurança para remoção de worktrees com commits não enviados, sujos ou travados.

4. **Diff & Files:**
   - *Algorithm:* seletor de algoritmo de diff (`histogram`, `myers`, `patience`).
   - *Context:* quantidade padrão de linhas de contexto nos hunks (`diff.context`).
   - *Display Options:* ignorar espaços em branco por padrão (`ignore-space-change`) e controle de wrap de linhas.

5. **Hooks & Security (D17, D29, D30):**
   - *Configured Hooks:* lista de hooks do GitMaster declarados no `.gitconfig` (`prepare-commit`, etc.) com seus comandos, matchers e timeouts.
   - *Repository Hook Approvals:* tabela de hooks de repositório autorizados (`hook-approvals.json`), exibindo repositório, comando, hash SHA-256 e data de aprovação, com botão **Revoke** para revogar permissões concedidas.

6. **Stacks & Restack (Fatias 4 e 5):**
   - *Restack with Dirty Working Tree:* comportamento ao rodar restack com alterações não commitadas (política de bloqueio seguro de Q2).
   - *Push Strategy:* preferência padrão para push de stacks (`--force-with-lease`).

7. **Conflict Resolver (Fatia 6):**
   - *Conflict Style:* estilo de exibição de conflitos (padrão `zdiff3`).
   - *Git rerere:* ativar/desativar reutilização automática de resoluções gravadas.

8. **MCP & Agents (Fatia 7):**
   - *MCP Server:* ativar/desativar servidor interno e caminho do socket Unix.
   - *Authentication:* geração e revogação de tokens de autenticação para o shim `gitmaster mcp`.
   - *Agent Policies:* política de permissões para ações de `Actor::Agent` (quais ferramentas exigem aprovação humana explícita na UI antes do apply).

9. **Local Review (Fatia 8):**
   - *Review Storage:* local de armazenamento de comentários e snapshots de review no diretório comum do Git.
