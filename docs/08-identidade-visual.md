# 08 · Identidade visual

Estado: ícone decidido, paleta decidida, tela escura e tipografia em aberto. As pranchas estão no canvas "GitMaster: o todo". Os arquivos do ícone estão em `assets/icons/`.

## Decisões

| # | Decisão | Status |
|---|---|---|
| V01 | Ícone: direção **Divisa** (chevrons de patente empilhados + um nó acima). Chevron = "master" e = pilha de branches; o nó é o commit HEAD. | Decidido |
| V02 | Os ícones do app são **6a (escuro, padrão)** e **6b (claro)**. As outras variações (Lanes, Azul, Duas, Insígnia) ficam só como exploração. | Decidido |
| V03 | Paleta do app e do ícone: o tema Neovim **cursorized** do Gustavo (`config/nvim/colors/cursorized.lua`). Claro e escuro. | Decidido |
| V04 | Nó do ícone na cor de assinatura `#f54e00`. | Decidido (vale nos arquivos de `assets/icons/`) |
| V05 | Telas desenhadas na variante clara. Variante escura ainda não desenhada. | Aberto |

## Paleta (cursorized)

Regras do tema: tinta primeiro (código é quase todo `fg`), oito acentos com a mesma luminosidade OKLab, rampa de cinco tons de tinta com matiz quente único, cursor em laranja fora das regras.

| Token | Claro | Escuro |
|---|---|---|
| bg | `#f7f7f4` | `#1a1918` |
| bg_hl | `#eeede7` | `#272623` |
| border | `#dbd9d2` | `#393833` |
| fg_faint | `#8a8882` | `#716f68` |
| fg_comment | `#696761` | `#9d9b92` |
| fg | `#31302d` | `#d2cfc5` |
| fg_emph | `#161614` | `#faf8f4` |
| red | `#ba3e4b` | `#ff8b90` |
| orange | `#aa5100` | `#f69557` |
| yellow | `#846600` | `#d3a82d` |
| green | `#007b2f` | `#64c175` |
| cyan | `#007677` | `#00c0c0` |
| blue | `#006cbc` | `#64b2ff` |
| violet | `#7856c0` | `#b89fff` |
| magenta | `#a9438d` | `#ed8ecf` |
| assinatura (cursor) | `#f54e00` | `#f54e00` |

### Uso no app

- Superfícies: `bg` no editor e nos painéis, `bg_hl` no fundo e na barra lateral, `border` nas linhas.
- Estados: verde = ok/resolvido, amarelo = aviso/defasado, vermelho = conflito/erro, azul = seleção e ação primária.
- Lanes do grafo: base = violet, ui = cyan, api = orange, main = fg_faint. Aresta de stack defasado = yellow tracejado.
- Tintas de fundo: acento misturado sobre `bg` (9 a 13%), texto sobre a tinta = acento escurecido 20 a 45% em direção a `fg_emph`.
- Bolinhas da janela usam os acentos da variante escura.

### Papéis em `crates/gm-ui/src/theme.rs`

`ground` = `bg_hl` (fundo da janela e barra lateral); `editor` e `panel` = `bg` (diff, barras, cabeçalhos, painéis flutuantes); `hairline` = `border`; `muted`, `faint`, `placeholder` e `gutter` = `fg_comment` (`fg_faint` não passa 4,5:1 como texto, fica só para elementos sem texto); `caret` = assinatura. Tintas de fundo seguem a regra acima (9 a 13% do acento sobre `bg`); texto sobre tinta = acento misturado a `fg_emph` (30% no claro, 20% no escuro). Os testes de contraste de `theme.rs` (4,5:1 para texto, 3:1 para marcas) cobrem todos os pares.

### Ponto de atenção

`#f54e00` é a cor do Cursor (editor). Como acento pequeno (nó, cursor de texto) é aceitável. Como fundo do ícone evoca a marca deles, por isso não é usada assim.

## Ícone Divisa

Grade 512, margem de segurança 64, chevron com ângulo de 28°.

| Versão | Divisas | Traço | Passo | Nó | Usada em |
|---|---|---|---|---|---|
| Grande | 3 | 44 | 90 | r 34 em (256,100) | 64 px ou mais |
| Médio | 3 | 54 | 92 | r 40 em (256,100) | 32 e 48 px |
| Pequeno | 2 | 70 | 100 | r 50 em (256,130) | 24 px ou menos |

Escuro (6a, padrão): fundo `#1a1918`, divisas de cima para baixo `#faf8f4`, `#64b2ff`, `#b89fff`, nó `#f54e00`.
Claro (6b): fundo `#f7f7f4`, divisas `#161614`, `#006cbc`, `#7856c0`, nó `#f54e00`.

Glifo monocromático (barra de menu, bandeja): 2 divisas + nó, uma cor só, sem fundo.

No macOS o corpo do ícone ocupa 824 de 1024 px, e a variante óptica é escolhida pelo tamanho do corpo (pequena até 32 px, média em 64 px, grande a partir de 128 px).

## Arquivos no repositório

```
assets/icons/
  README.md
  svg/      fonte: gitmaster-{dark,light}[-medium|-small|-macos].svg e gitmaster-glyph.svg
  png/      dark/ e light/, gitmaster-{16,24,32,48,64,128,256,512,1024}.png, cantos transparentes
  macos/    GitMaster-dark.icns, GitMaster-light.icns
  windows/  gitmaster.ico (escuro)
```

Os SVGs são a fonte. PNG, `.icns` e `.ico` são derivados.

## Questões abertas

- Desenhar o app na variante escura.
- Linux: arquivo `.desktop` e instalação em `hicolor`, e como o GPUI define o ícone da janela.
- Tipografia: as telas usam IBM Plex Sans e JetBrains Mono como placeholder. Falta decidir a fonte da UI em GPUI.
- Ícone alternativo por tema (`GITMASTER_THEME`) ou só o escuro como padrão do sistema.
