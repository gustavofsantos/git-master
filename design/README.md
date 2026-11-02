# Design records

This directory is for people who build GitMaster. It is not part of the documentation website.

The product is specified in [`docs/`](../docs/README.md): the official documentation, written as the **desired state** of the product. The code converges on it. This directory holds what the docs deliberately leave out: why choices were made, how things are built, and in what order the work happens.

| File | Content |
|---|---|
| [`decisions.md`](decisions.md) | Decision log (D01–D108) with status, open questions (Q1–Q23) and implementation notes. Decisions marked Decided bind the code. |
| [`roadmap.md`](roadmap.md) | The order of the work: slices, what is done, known gaps, what comes next. |
| [`architecture.md`](architecture.md) | Hexagonal architecture, engine rules, persistence, crates, risks. |
| [`screens.md`](screens.md) | Screen-level design decisions behind the user guide. |
| [`stacks-restack.md`](stacks-restack.md) | Stack model and the restack algorithm. |
| [`code-navigation.md`](code-navigation.md) | Engineering design for Quick Open, the viewer and text search: engines, ranking, risks (symbols removed, D163). |
| [`workflow-scripts.md`](workflow-scripts.md) | Engineering design for script workflows: sandbox, budgets, journal, approvals, tooling. |
| [`hooks-transitional.md`](hooks-transitional.md) | The `prepare-commit` hook that workflows replace. |
| [`visual-identity.md`](visual-identity.md) | Palette, icon, files in `assets/icons/`. |
| [`lessons.md`](lessons.md) | Toolchain pitfalls, GPUI practice, how agents are used on this codebase. |

## Design canvas

The screens and the visual identity are drawn in a design canvas, "GitMaster: o todo": https://claude.ai/artifact/Qb4Ht2rnfEC8HueFR5Gtqx#play-15dd8cd198dd

The canvas is the source for mockups; `docs/` is the source for behavior. The canvas labels are in Portuguese: translate them in the UI.

## Old numbering

Earlier versions of the design documents were numbered `00` to `11` and written in Portuguese. They were replaced by the files above and by `docs/`. Code comments written before the change that cite a `docs/NN` file now point to the renamed file. The last commit that has the old Portuguese files is `5f88889` (`git show 5f88889:docs/10-workflows.md`); the TOML workflow specification, which scripts replace, is only there.
