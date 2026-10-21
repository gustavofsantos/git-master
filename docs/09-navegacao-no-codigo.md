# Navegação no código

Última atualização: 2026-10-21. Status: escopo decidido (D43). S-A, S-C, N1 e N2 estão feitos (`07`, D89, D90); D44 e D46 foram sustentadas pelos spikes. O resto (S-B, N3 a N6, D45, D47 a D51) segue como **proposta** em `01`.

## Por que

Quem revisa mudanças de agentes (Fatia 8) e navega num monorepo precisa achar contexto sem sair do app: abrir um arquivo pelo nome, buscar um texto no repositório, saltar para a definição de um símbolo. Serviços de hospedagem oferecem isso na web; localmente deveria ser mais rápido, porque os dados já estão no disco e não há rede.

Isto vai além de um cliente Git comum, de propósito: o produto é um cliente Git unido a ferramentas para trabalhar com o código, e os clientes existentes são referência, não molde (`00`, Posicionamento). O escopo está decidido (D43); a posição no roadmap segue em aberto (Q13).

## O que entra

Uma página **Code** na sidebar, ao lado de Working Copy e History, e três buscas:

| Busca | Pergunta que responde | Atalho proposto |
|---|---|---|
| **Arquivos** (Quick Open) | "abrir `diff_pane.rs`" | `Ctrl/⌘+P` |
| **Texto** (grep) | "onde aparece `HistoryStream`" | `Ctrl/⌘+Shift+F` |
| **Símbolos** | "onde está definida `fuzzy_match`" | `Ctrl/⌘+Shift+O` (arquivo atual) e `#` no Quick Open (repositório) |

Mais um **visualizador somente leitura** com árvore de pastas, que é para onde as três buscas levam. Os atalhos precisam ser conferidos contra o registro de ações (`action_registry.rs`); o `Ctrl+K` da paleta já está tomado.

"No lugar atual" quer dizer: o worktree ativo (D31), no estado em que está no disco. Uma revisão qualquer (branch, tag, commit) entra na última sub-fatia (N6).

## Princípios aplicados

- **Git é a fonte da verdade (3).** A lista de arquivos vem de `git ls-files`, a busca de texto de `git grep`. O app não tem a própria noção de "o que é do projeto": `.gitignore`, sparse-checkout e submódulos valem como o Git os aplica.
- **Nada no working tree (5).** O índice de símbolos mora no diretório de dados do app (ou em `.git/`, ver D45), nunca ao lado do código.
- **A UI nunca bloqueia (4).** Toda busca roda em thread própria, com atraso (debounce), cancelável (mata o processo, como o `Canceller` do histórico) e com resposta de pedido superado descartada por `OperationId`. É o mesmo modelo do H2 a H4.
- **Determinismo (1).** Mesma entrada, mesma ordem de resultados: o desempate do ranking é o caminho, nunca a ordem de chegada das threads. Navegar até a definição nunca "adivinha": se há mais de um candidato, lista todos.
- **Leitura não vai ao log de comandos (D28).**

## Fluidez como condição

A análise estrutural (tree-sitter) é bem-vinda, mas só se o app continuar fluido e rápido. Na prática: o índice roda em segundo plano, em poucas threads e com prioridade baixa; nenhuma busca espera por ele (resultado parcial é sinalizado); o destaque de sintaxe só trata o intervalo visível; e o gate do D49 inclui "rolagem e digitação sem queda de quadros durante a indexação". Se o spike S-B mostrar que não dá para cumprir isso, a primeira versão sai sem símbolos (arquivos e texto) e os símbolos esperam.

## Sem programas extras (D51)

O GitMaster é autocontido: a busca não pode exigir que o engenheiro instale nada além do que o app já exige. O único programa externo é o `git` (D26, D32), que a Working Copy e o histórico já usam. Ficam de fora, como executáveis, ripgrep, ctags, ast-grep e servidores de linguagem (LSP). Qualquer motor adicional entra como **biblioteca Rust compilada no binário** (tree-sitter para símbolos; `regex` e `ignore` se um dia o `git grep` não bastar), e seu custo de compilação e de tamanho entra nos spikes.

## Caixa única (D50)

O usuário digita o que quer, sem sintaxe. Um **classificador determinístico** (regras, sem LLM, Princípio 1) decide quais motores rodam, todos em paralelo, e a resposta chega em seções que aparecem conforme cada motor termina:

| O que foi digitado | Motores | Seções |
|---|---|---|
| `diff_pane`, `DiffPane` (identificador, sem espaço) | arquivos + símbolos + usos | Files, Definitions, Usages |
| `gm-ui/diff`, `foo.rs`, `foo.rs:120` | arquivos | Files (abre na linha, se houver) |
| `foo(` ou `foo()` | símbolos + usos, com dica de chamada | Definitions, Calls |
| várias palavras, pontuação, aspas (`"not available yet"`) | texto literal | Text |

- Seções em ordem fixa; dentro de cada uma, pontuação por regra e desempate por caminho e linha, nunca pela ordem de chegada. Mesma entrada, mesma lista.
- Cada tecla cancela os motores anteriores (mata os processos), com atraso curto.
- Sintaxe nunca é obrigatória. Filtros (`lang:`, `path:`, caso, regex) podem existir como atalho para quem quiser, mas a caixa sem eles já tem de resolver o dia a dia.
- "Semântica" no sentido de busca por significado (descrever em linguagem natural e achar o código) é outra coisa: depende de embeddings ou de um LLM, não é determinística e fica fora daqui. Um agente pode fazê-la pelo MCP (`code_grep`, `symbols_search`) e propor; o app não decide sozinho.

## Busca de arquivos

- **Lista:** `git ls-files -z` do worktree ativo (rastreados) e, opcionalmente, `--others --exclude-standard` (não rastreados que não são ignorados). Para uma revisão, `git ls-tree -r -z --name-only <rev>`.
- **Estrutura:** um buffer único de bytes com offsets, ordenado, sem uma `String` por caminho. Um monorepo de 1 milhão de arquivos com caminhos de 60 bytes custa uns 60 MB, não centenas.
- **Invalidação:** o watcher que já existe (`gm-core/src/watch`) avisa mudança de índice, HEAD e refs; a lista é reconstruída em segundo plano e a antiga serve até a nova ficar pronta. Reconstrução em streaming, com "indexando…" visível, porque `ls-files` em repositório grande leva segundos.
- **Ranking:** o `fuzzy.rs` atual serve para ações e branches (centenas de itens). Para caminhos precisa de uma variante: bônus para o nome do arquivo sobre o diretório, para início de segmento e para extensão, `/` como separador de palavra, e pesquisa paralela com heap de top-K em vez de ordenar tudo. O `fuzzy_match` aloca `Vec<char>` por chamada, o que não escala para 1 milhão de candidatos; a variante trabalha sobre bytes (com fallback para Unicode só quando a consulta não é ASCII).
- **UI:** reaproveita a `palette_view` (lista virtualizada, trecho casado em destaque) com um provedor de resultados diferente do provedor de ações. A decisão entre "modo da paleta" e "overlay próprio" fica para o N1, depois de ler `palette.rs`.
- **Filtros úteis:** `foo.rs:120` abre na linha 120; prefixo de pasta (`gm-ui/ diff`) restringe.

## Busca de texto (grep)

**D44, proposto: `git grep` como motor padrão.** Mesmo argumento do D32: já está instalado, respeita `.gitignore`, funciona em qualquer revisão (blobs) e no worktree, usa várias threads e faz streaming. Chamada base:

```
git grep -I -n --column -z --no-color [-i] [-w] [-F|-E|-P] [--untracked] -e <padrão> [<rev>] -- [<pathspec>]
```

- `-I` ignora binários; `-z` torna a saída segura para nomes com caracteres estranhos; `--column` exige Git 2.19+ (conferir a versão mínima do app).
- **Opções na UI:** diferenciar maiúsculas, palavra inteira, regex (`-E`; `-P` só se o Git foi compilado com PCRE), e um campo de pathspec ("incluir" e "excluir" com `:(exclude)`), como em serviços de hospedagem.
- **Limites:** por exemplo 1000 linhas no total e 100 por arquivo, com "N mais" explícito; a busca para ao atingir o limite (mata o processo). Linhas longas truncadas ao redor do trecho casado.
- **Resultado:** agrupado por arquivo, com contagem, trecho casado em destaque e clique que abre o visualizador na linha. Ordem determinística: por caminho, depois por linha (a saída do `git grep` é paralela, então é reordenada antes de mostrar, ou a busca roda com `--threads=1` se o custo ficar baixo).
- **Alternativa embutida:** se o benchmark de N3 mostrar que `git grep` não basta, a saída é uma busca em processo (crates Rust `regex` e `ignore`, compiladas no binário), nunca um programa externo (D51).
- **Segurança:** o padrão vai sempre depois de `-e`, e o pathspec depois de `--`, para que texto digitado nunca seja lido como opção.

## Símbolos

É a parte difícil e a de maior risco. Quatro caminhos:

| Caminho | A favor | Contra |
|---|---|---|
| **tree-sitter** com `tags.scm`, compilado no binário | Em processo, sem dependência externa, determinístico, incremental; a mesma gramática dá o destaque de sintaxe do visualizador | Uma gramática (código C) por linguagem aumenta tempo de compilação e tamanho do binário; cobertura é por linguagem |
| `universal-ctags` | Cobre dezenas de linguagens | Programa externo que o engenheiro teria de instalar (D51): descartado |
| LSP | Referências e definição precisas | Pesado, um servidor por linguagem e por projeto, estado não determinístico, contraria "local-first, simples" |
| Regex via `git grep` (`fn \w+`, `class \w+`) | Zero dependência | Falsos positivos e negativos, por linguagem; só serve de plano B |

**D45, proposto: tree-sitter compilado no binário, com consultas `tags.scm`, para um conjunto curado de linguagens.** É uma biblioteca, não um programa a instalar (D51). Continua sendo sintaxe, não semântica: não resolve escopo nem tipos (D47). Pontos do desenho:

- **Cache da classificação** pelo OID do blob, como o índice de definições.

- **Índice endereçado por conteúdo.** A chave é o OID do blob (ou o hash do conteúdo, para arquivo modificado no worktree). O mesmo blob em outra branch, ou no histórico, reaproveita o resultado, e nada fica velho: o índice é derivado e descartável (Princípio 3). Perder o arquivo só custa reconstruir.
- **Onde guardar:** SQLite em `.git/gitmaster/` (alinhado com o `gm-store` previsto em `06`) ou no diretório de dados do app. Decidir no spike; ambos respeitam o Princípio 5.
- **Construção em segundo plano**, com prioridade baixa, limite de threads e progresso visível ("símbolos: 12 340 / 98 000 arquivos"). A busca de símbolos funciona com o índice parcial e diz que está parcial.
- **Busca:** fuzzy sobre o nome, com filtro por tipo (`fn`, `struct`, `class`, `trait`…), nome do arquivo e linha ao lado. A "visão geral do arquivo" (outline) usa a mesma tabela filtrada por arquivo.
- **Ir para a definição** (clique ou atalho sobre um identificador no visualizador): busca o nome no índice. Um candidato abre direto; vários listam todos. Sem resolução semântica de escopo.
- **Usos:** "Find usages" é `git grep -w <nome>`, e a interface diz que são ocorrências de texto, não referências semânticas. Linguagem sem gramática continua achável por nome e por texto. Não vamos fingir precisão que a abordagem não tem; referências precisas pedem LSP, fora de escopo (D47).
- **Linguagens, em ondas:** Rust, TypeScript/JavaScript, Python, Go; depois Java, C/C++, C#, Kotlin, Ruby, Swift. Arquivo de linguagem sem gramática continua achável por nome e por texto, só não tem símbolos.

## Visualizador

- Somente leitura: mostra arquivo do worktree (lido do disco) ou blob de uma revisão (`git cat-file --batch`, processo longo, ou blob do libgit2). Sem edição (Q7 trata do texto editável do resolvedor, não disto). "Open in editor" usa `core.editor`, como já previsto em `05`.
- Virtualizado por linha, reaproveitando o que o `diff_pane` já resolveu (medição de largura, rolagem horizontal, tabulação a 4 colunas). Número de linha, "ir para a linha", copiar `caminho:linha`.
- Guardas: binário vira "Binary file"; arquivo acima de um limite (ordem de grandeza: 2 MB) abre truncado com aviso, nunca congela; fora de UTF-8 sai com caracteres de substituição, como no diff.
- **Destaque de sintaxe** (N4) vem do mesmo tree-sitter, com `highlights.scm`, e só para o intervalo visível. O diff ganha o destaque de graça depois, mas isso é outra decisão.
- **Árvore de pastas:** gerada da lista de arquivos, recolhida por padrão, com caminhos de pasta única compactados (`a/b/c`), como em serviços de hospedagem.
- **Pontos de entrada fora da página Code:** a linha de arquivo do diff e do detalhe do commit ganha "View file"; o item de menu de arquivo da Working Copy também. Depois de H3/H4, é só uma ação nova no registro.

## Escala

Os alvos são repositórios massivos (`00`). Por isso:

- Nenhuma lista cresce sem limite na tela: tudo é virtualizado; resultados têm teto.
- Nenhuma busca roda sem cancelamento; mudar a consulta mata o processo anterior.
- Sparse-checkout: a lista de arquivos do worktree só tem o que existe; a busca em revisão pode ter mais. A interface deve dizer qual das duas está vendo.
- **Gate de performance (D49)**, no estilo do H8: benchmarks ignorados em repositórios gerados com `git fast-import` de 100 mil e 1 milhão de arquivos. Metas iniciais, a calibrar no spike: primeira lista de arquivos utilizável em segundos com "indexando…"; tecla no Quick Open responde em menos de 50 ms com a lista pronta; primeiro resultado de grep em menos de 1 s em 100 mil arquivos; o índice de símbolos nunca sobe a CPU a ponto de atrapalhar a UI.

## MCP

Tools do tier Read (D20), baratas porque o motor já as tem: `files_search`, `code_grep`, `symbols_search`, `file_read` (com faixa de linhas). Leitura não precisa de aprovação. Entram na Fatia 7; o motor deve nascer com `Command`s e `Event`s que a UI e o MCP compartilham (regra 1 de `06`), então nada aqui é "só de UI".

## Divisão em sub-fatias

Cada uma é vertical, usável sozinha e tem "pronto quando" com Git real, como as da Fatia 3.

0. **Spikes (primeiro, risco técnico).**
   - **S-A, listas e grep em escala:** tempos de `git ls-files`, `git grep` (worktree e revisão) e da leitura em streaming num repositório de 100 mil e outro de 1 milhão de arquivos.
   - **S-B, tree-sitter:** custo de compilação e tamanho do binário em Linux e macOS com 4 gramáticas; qualidade de `tags.scm` em repositórios reais; tempo para indexar 100 mil arquivos; onde guardar o índice.
   - **S-C, casador de caminhos:** a variante do fuzzy sobre 1 milhão de caminhos, com top-K paralelo.
   Resultado de cada spike vira linha em `01` (confirma ou derruba D44 e D45).
1. **N1, Quick Open.** Lista do worktree, casador de caminhos, `arquivo:linha`, atualização pelo watcher. *Pronto quando:* testes do casador (ordem, desempate, Unicode), lista idêntica à de `git ls-files` em repositório com ignorados, renomeados e sparse-checkout, pedido superado descartado, cenário do smoke nos dois temas. Entregue junto com o N2 (2026-10-21): o clique abre o arquivo no visualizador.
2. **N2, visualizador e árvore.** Página Code, árvore, leitura do worktree, limites de arquivo grande e binário, "View file" a partir do diff e do detalhe do commit. *Pronto quando:* arquivo UTF-8, fora de UTF-8, binário, vazio, sem newline final e muito grande se comportam sem travar; rolagem fluida em 100 mil linhas.
3. **N3, grep.** `git grep` com as opções acima, limites, cancelamento, resultado agrupado, clique que abre o N2 na linha. *Pronto quando:* testes com Git real de nomes estranhos, padrão que parece opção (`-v`, `--help`), regex inválida, binários, limites e ordem determinística; benchmark do S-A repetido no app.
4. **N4, destaque de sintaxe** (depende do S-B). Quatro linguagens primeiro. *Pronto quando:* destaque só do intervalo visível, sem tocar o tempo de rolagem; arquivo de linguagem desconhecida segue sem cor.
5. **N5, símbolos.** Índice por blob, busca, outline do arquivo, ir para a definição com candidatos, usos como texto. *Pronto quando:* o mesmo blob em duas branches indexa uma vez; índice parcial é sinalizado; apagar o índice e reabrir reconstrói o mesmo resultado (determinismo); índice nunca fica dentro do working tree.
6. **N6, revisões, MCP e gate.** Seletor de revisão na página Code (reaproveita a pílula de escopo do H6), "Browse files at this commit" no menu de commit (junto com a parte da 1c que espera o menu), tools Read do MCP e os benchmarks do D49.

## Onde entra no roadmap

Proposta: **Fatia 3c**, depois de H5 a H8, antes das stacks, porque reaproveita o que a Fatia 3 constrói (leitura em thread própria, `OperationId`, virtualização, pílula de escopo) e dá valor imediato a quem já vive no histórico. A ordem é decisão do autor; se o stacking for mais urgente (`07`, critério de ordem 4), a Fatia 3c pode ir depois da 5 sem custo, só que N6 depende do menu de commit e da Fatia 7.

## Fora do escopo

- Edição de arquivos e qualquer escrita no worktree.
- Navegação semântica (referências, tipos, renomear símbolo): é LSP, não esta fatia (D47).
- Busca em histórico de conteúdo (`git log -S`): já é o H7.
- Blame e histórico do arquivo: natural depois, mas pertence à Fatia 3 (`git log -- <caminho>` e `git blame`), não a esta.
- Busca em submódulos e em LFS (Q8).
- Indexar texto de todo o repositório num índice próprio (estilo Zoekt): só se o `git grep` não alcançar o gate, e então como decisão nova.

## Riscos

| Risco | Mitigação |
|---|---|
| Gramáticas tree-sitter pesam na compilação e no binário, em dois SOs | Spike S-B antes de decidir; linguagens atrás de features do Cargo; fallback para regex via `git grep` |
| `git grep` lento em monorepo enorme | Pathspec obrigatório sugerido na UI, limites, benchmark; busca embutida em processo (D51); índice próprio como último recurso |
| Lista de arquivos de 1 milhão de caminhos engasga o casador | Buffer contíguo, top-K paralelo, spike S-C |
| Índice de símbolos cria expectativa de precisão semântica | Texto da interface honesto ("ocorrências de texto"), candidatos listados, sem adivinhar |
| Escopo cresce para virar um IDE | O visualizador é somente leitura por decisão; edição e LSP ficam em D47 |
