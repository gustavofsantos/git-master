# GitMaster: visão e princípios

Última atualização: 2026-10-16

## O que é

Cliente Git desktop, nativo, open source, entregue como produto premium. Feito por e para engenheiros que trabalham em repositórios massivos, com muita gente e com agentes de IA gerando mudanças em volume. Consolida scripts e programas sob medida que o autor criou ao longo dos anos.

## Benchmarks

- **Git Tower**: sidebar de branches, Working Copy com stage por linha, histórico, menus de contexto, worktrees, commit com IA.
- **JetBrains (VCS)**: ferramenta de resolução de conflitos em três painéis (aplicar mudanças não conflitantes, destaque por palavra, setas de aceitar esquerda/direita).

## Posicionamento

O GitMaster não é um clone do Tower. Os benchmarks acima são referência: onde eles fizeram uma escolha que funciona, adotamos e fazemos bem; depois vamos além. O produto junta três coisas: um cliente Git completo e rápido; um espaço de trabalho com o código (navegar no repositório onde se está: arquivos, texto, símbolos), e a integração com agentes de IA para quem responde pelo código-fonte. Toda funcionalidade deve servir a pelo menos uma das três.

Duas qualidades acompanham tudo: o app tem de continuar fluido e rápido (uma funcionalidade mais rica só entra se a sensação se mantiver) e autocontido (o único programa externo exigido é o `git`).

## Diferenciais

1. **Hooks extensíveis via `.gitconfig`**, para eventos do próprio app (ex.: `prepare-commit` chamando um LLM por script).
2. **Stacking de branches** como cidadão de primeira classe: cada branch tem um pai e vários filhos; restack com plano, execução e undo.
3. **Amigável a IA**: funcionalidades expostas via MCP; loop de code review local com agentes.

## Princípios

1. **Determinismo.** Plano e execução produzem o mesmo resultado. Nada de previsão "provável". Saída de LLM é sempre proposta, nunca aplicada sozinha.
2. **Humano aprova, agente propõe.** Operações que mudam o repositório exigem aprovação por política.
3. **Git é a fonte da verdade.** O estado do motor é derivado e descartável. Tudo é revalidado contra fingerprints, porque o Git muda por fora (inclusive por agentes usando o CLI).
4. **A UI nunca bloqueia.** Hooks e operações longas são assíncronos.
5. **Nada no working tree do repositório.** Configuração e estado ficam fora dele (git config de usuário, `.git/`, diretório de dados). Em muitos repos de trabalho não se pode escrever nem no `.gitignore`.
6. **Observado vale mais que relatado.** O que um agente fez é medido por diff de snapshots; o texto do agente é só narrativa.
7. **Local-first.** Uso local, por um engenheiro, na própria máquina.

## Não-objetivos

- Modo headless, CI ou servidor.
- Tauri ou front-end em JavaScript.
- Plugins como bibliotecas dinâmicas (extensão é por processo).
- Ainda não discutidos: pull requests, hospedagem, colaboração em tempo real.

## Stack técnica

Rust, GPUI (nativo e eficiente). Detalhes em `06-arquitetura.md`.

## Mapa dos documentos

- `01-decisoes-e-questoes-abertas.md`: log de decisões com status e perguntas em aberto
- `02-stacks-e-restack.md`
- `03-hooks.md`
- `04-mcp-e-review-local.md`
- `05-telas.md`
- `06-arquitetura.md`
- `07-roadmap.md`
