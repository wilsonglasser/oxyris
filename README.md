# Oxyris Code

Desktop coding agent for Windows that treats **Windows-native** and **WSL** projects as
first-class citizens. Run multiple parallel Claude sessions per worktree, edit files in a
built-in tree + editor, drive git without leaving the app, and rely on an append-only
event log that can rebuild every read model from scratch.

> Status: active development — see [`PLAN.md`](./PLAN.md) for the product plan and
> [`CLAUDE.md`](./CLAUDE.md) for contributor guidance.

## Highlights

- **Two execution environments, no cross-fallback.**
  - *Windows* projects run natively (`std::fs`, `git2`, `Command`, `portable-pty`).
  - *WSL* projects delegate **every** filesystem, git, spawn, and PTY op to a per-distro
    agent over NDJSON stdio — operations run inside the distro on the real Linux
    filesystem, never through the `\\wsl.localhost` 9p bridge.
- **Parallel Claude sessions** — many sessions per worktree, including a **pure mode**
  that runs the `claude` CLI straight in a PTY, plus a supervisor-driven **auto-pilot**
  for unattended pure sessions.
- **File tree + editor** — CodeMirror-based editor with a live file watcher. External
  edits (e.g. an LLM rewriting a file you have open) are detected and either reloaded
  silently or surfaced as a reload/keep-mine conflict. The watcher works on Windows
  (native `notify`) **and** WSL (native inotify inside the distro, via the agent).
- **Git, built in** — branches, worktrees, status/diff, stage/commit, stash, tags,
  cherry-pick/revert, and hidden-ref checkpoints, all via `git2` (shelling out only
  where `git2` can't).
- **Code intelligence** — a shared LSP bridge (rust-analyzer, tsserver, …), symbol
  search, and find-in-files.
- **Provider abstraction** — a generic `Provider` trait. Claude is the first (and only
  shipping) implementation; Codex/Cursor/OpenCode plug into the same trait without
  backend surgery.
- **i18n** — every user-visible string goes through react-i18next. English is the base
  locale; pt-BR ships today.

## Stack

- **Tauri v2** (WebView2, MSI + NSIS installers)
- **Rust 2024** Cargo workspace
- **React 19 + Vite + Tailwind v4**
- **SQLite** append-only event store (rusqlite, bundled) — `Project` / `Worktree` /
  `Session` / `Turn` aggregates with pure `decide()` / `apply()`; read models are
  projections, dropped and rebuilt from events when their schema changes.
- **WSL agent** — static musl Linux binary, NDJSON over stdio.

## Development (Windows)

Prerequisites:

```powershell
winget install Rustlang.Rustup OpenJS.NodeJS.LTS
rustup toolchain install stable
npm install -g bun
```

Install dependencies and run the dev stack:

```bash
bun install
bun run tauri dev
```

### WSL agent

WSL projects need the Linux agent binary. Cross-compile it (musl) before working on
WSL features; the backend deploys it into each distro on first use and redeploys when
the host binary is newer:

```powershell
pwsh -NoProfile -File scripts/build-agent-linux.ps1
```

### Validation gate

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
bun run typecheck
bun run build
```

## Packaging

Signed/release build of the MSI + NSIS installers:

```powershell
pwsh -NoProfile -File scripts/package.ps1
```

Output lands under `./release/`. No code signing is configured by default — set
`TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` to sign, or wire
Azure Trusted Signing into the script if you need MS SmartScreen to trust the installer
out of the box.

A one-shot unsigned build:

```bash
bun run tauri build
```

### Releases

A release is a version bump across `package.json`, `apps/web/package.json`,
`apps/desktop/tauri.conf.json`, and `apps/desktop/Cargo.toml` (plus `Cargo.lock`),
committed and tagged `vX.Y.Z`. Pushing the tag triggers `release.yml`.

## Layout

```
apps/desktop/     Tauri Rust backend (tauri.conf.json + src/main.rs)
apps/web/         React frontend (builds to apps/web/dist/)
apps/agent/       WSL Linux helper (musl binary, deployed at runtime)
apps/mcp-server/  MCP server (LSP/tooling proxy for Claude sessions)
crates/
  oxyris-core       shared domain types (events, commands, IDs)
  oxyris-ipc        backend ↔ agent NDJSON protocol
  oxyris-provider   generic Provider trait + capability types
  oxyris-claude     Claude CLI stream-json adapter (a Provider impl)
  oxyris-git        git operations (git2)
  oxyris-lsp        language-server integration
  oxyris-index      workspace indexing
  oxyris-laravel    framework-awareness support
  oxyris-supervisor session/turn supervision
  oxyris-procutil   process helpers
```

See [`PLAN.md` §6](./PLAN.md) for the full layout.

## License

MIT or Apache-2.0 at your option.
