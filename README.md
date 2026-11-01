# GitMaster

**The Git client for the age of AI agents.**

Agents now write a large share of the code that lands in your repositories. You are still the one accountable for it. GitMaster is a fast, native Git client built so you can let agents work and stay in control: the agent proposes, you approve, and the app records what really happened.

## What you get

### A Git client that is complete and fast

Everything you do in a repository every day, without waiting for the UI.

- **Working Copy** with staging per file, per chunk and per line, and a commit composer.
- **History** as a commit graph, with the files and diff of every commit.
- **Branches and worktrees** one click away in the sidebar, plus fetch, pull and push.
- **Command palette** (`Ctrl+K`, `⌘K` on macOS): every action, reachable from the keyboard.

It is native and built for speed. Long lists stay smooth, and nothing blocks while Git works.

### A workspace for working with code, not only commits

Move around the repository where you stand, without opening another tool.

- **Quick Open** to jump to any file by name.
- **Text search** with case, whole-word and regex options, grouped by file.
- **A code viewer** with the folder tree, all local and instant.

### Agents under your accountability

This is what sets GitMaster apart.

- **Review loop.** Comment on a diff in the app; your agent reads the comments, fixes the code, and marks them handled. You resolve them.
- **Agent sessions you can search.** Every Claude Code session is recorded as a thread: what was asked, what changed, which files were edited. Weeks later, you or your agent can find the reason behind a decision.
- **Observed, not self-reported.** The app records what the agent did itself, instead of trusting the agent's own summary.
- **Stacked branches.** Break a large change into a stack of small, reviewable branches, and restack them automatically when the base moves.
- **Workflows you control.** Automate your own rules as sandboxed scripts that run at the moments you choose.

## Built to be trusted

- **Only needs `git`.** No account, no server, no extra tools to install.
- **Local-first.** Your code and history stay on your machine.
- **Never touches your working tree.** No config files dropped into your repositories, which matters when you cannot edit a `.gitignore`.
- **Plain Git underneath.** What you see is your repository, not a proprietary layer.

## Try it

**Install** (macOS and Linux):

```sh
curl -fsSL https://raw.githubusercontent.com/gustavofsantos/git-master/main/install.sh | sh
```

**Open a repository:**

```sh
gitmaster path/to/repo
```

**Connect Claude Code** to see its sessions as threads and let it read your review comments:

```sh
claude plugin marketplace add gustavofsantos/git-master
claude plugin install gitmaster@gitmaster
```

Start a new Claude Code session in any repository and it shows up in GitMaster's Threads page.

## Learn more

- [Documentation](docs/index.md): start here for guides and reference.
- [Installation options](docs/getting-started/installation.md), including building from source.
- [First steps](docs/getting-started/first-steps.md).
- [Agent integration](docs/agents/agent-integration.md): update, check and remove the plugin.
- [Principles](docs/concepts/principles.md): why GitMaster works the way it does.

Contributors: see [`AGENTS.md`](AGENTS.md). GitMaster is currently unlicensed; see [`LICENSE`](LICENSE).
