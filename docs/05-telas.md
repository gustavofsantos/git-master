# Telas

Última atualização: 2026-10-16. Status de cada decisão em `01`. Os mockups interativos foram feitos na conversa de design de 2026-10-16; este documento guarda as decisões, não a aparência exata.

Referências visuais do Tower: sidebar com árvore de pastas por prefixo, Working Copy com composer de commit acima da lista de arquivos, diff com "Stage/Unstage chunk", menu de contexto de commit.

## Sidebar de branches (D10 decidido)

Duas organizações com alternância ("Por stack" e "Por nome"):

- **Por stack**: árvore indentada por dependência. Seções "Stacks" e "Sem stack", para quem não usa stacking não pagar o custo da feature. Adoção gradual: a ferramenta funciona como um Tower comum até a primeira branch ganhar um pai.
- **Por nome**: visão clássica, agrupada por prefixo (`feature/`).

Indicadores:

- Ação no **pai** (`restack 3`), porque é ele que mudou. Filhos desatualizados ganham só um ponto âmbar.
- `↑N`: commits à frente do **pai** (não da origin).
- Arrastar uma branch sobre outra define o pai (mesmo idioma do Tower de arrastar commit sobre o sidebar).
- Estado calculado fora da thread de UI.
- Lista virtualizada (`uniform_list` no GPUI), pensando em centenas de branches remotas.

## Fluxo de restack

Quatro etapas: Plano, Executando, Conflito, Concluído. Detalhes em `02`.

## Resolução de conflitos (D22)

Benchmark: JetBrains. Três painéis: esquerda, resultado (editável), direita.

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
