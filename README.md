# Oxyris Code

Desktop coding agent for Windows with native support for Windows and WSL projects, multiple parallel Claude sessions per worktree, and event-sourcing from day one.

> Status: greenfield — see [`PLAN.md`](./PLAN.md) for the full plan and [`CLAUDE.md`](./CLAUDE.md) for contributor guidance.

## Stack

- **Tauri v2** (WebView2, MSI installer)
- **Rust 2024** workspace with `cargo workspaces`
- **React 19 + Vite 8 + Tailwind v4 + TanStack Router**
- **SQLite** event store (rusqlite, bundled)
- **WSL agent** — static musl Linux binary, NDJSON over stdio

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

## Packaging

For a signed/release build of the MSI + NSIS installers:

```powershell
pwsh -NoProfile -File scripts/package.ps1
```

Output lands under `./release/`. No code signing is configured by default — set
`TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` to sign, or
wire Azure Trusted Signing into the script if you need MS SmartScreen to trust
the installer out-of-the-box.

A one-shot unsigned build is also available via:

```bash
bun run tauri build
```

## Layout

```
apps/desktop/   Tauri Rust backend
apps/web/       React frontend
apps/agent/     WSL Linux helper
crates/         Shared Rust libraries (core, ipc, claude)
```

See [`PLAN.md` §6](./PLAN.md) for the full layout.

## License

MIT or Apache-2.0 at your option.
