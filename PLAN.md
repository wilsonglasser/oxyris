# Oxyris — Plano Greenfield

> Desktop coding agent para Windows com suporte nativo a projetos Windows e WSL, múltiplas sessões Claude paralelas por worktree, event-sourcing desde o dia 1.

**Status:** planejamento. Código zero escrito.
**Data:** 2026-04-22

---

## 1. Visão

Oxyris é um app desktop Windows que permite:

- Abrir projetos que vivem em **Windows** (`C:\...`) ou dentro de **WSL** (qualquer distro), com cada ambiente usando seu próprio Claude (auth, binário, filesystem, git).
- Rodar **múltiplas sessões Claude simultâneas** no mesmo projeto, cada uma num git worktree separado, sem interferir entre si.
- Kit completo de chat de agente: streaming de resposta, diff viewer, markdown, imagens, slash commands, skills, terminal, checkpointing turn-by-turn.
- Desktop leve, nativo Windows, instalador MSI ~30-40 MB, consumo de RAM comparável a qualquer app moderno (não Electron-bloat).

**Diferencial competitivo:** nenhum app Claude hoje combina *worktrees paralelos + WSL first-class + Rust desktop leve*. Quem usa Windows + WSL (majoria dos devs Windows sérios) vai adorar.

---

## 2. Stack

### Desktop

- **Tauri v2** (WebView2, MSI installer, auto-updater).
- **Rust** (2024 edition, `cargo workspaces`) pro backend.
- **React 19 + Vite 8 + Tailwind v4** pro frontend.
- **TanStack Router** pra routing.
- **Zustand** pra state (simples, sem ceremony de Redux/Effect).
- **shadcn/ui** como base de componentes.
- **Lexical** pro composer (mesma do T3 Code, bom investimento).
- **CodeMirror 6** pro diff viewer (custom, não usar `react-diff-viewer` — limitado).
- **xterm.js** pro terminal.
- **react-markdown + Shiki** pra renderização de chat.

### Backend (Rust crates)

- `tokio` — async runtime
- `rusqlite` (bundled) — event store + read models
- `git2` (libgit2) — git ops + worktree management
- `portable-pty` — ConPTY no Windows, PTY Linux no agent
- `serde` + `serde_json` — IPC protocol
- `notify` — filesystem watching
- `walkdir` + `ignore` — file indexing
- `uuid` — IDs (v7 pra ordenação temporal)
- `chrono` — timestamps
- `anyhow` + `thiserror` — error handling
- `tracing` + `tracing-subscriber` — observability (NDJSON local trace)
- `which` — localizar binários no PATH

### WSL agent (sub-binário Rust)

Mesmas crates `tokio`, `git2`, `portable-pty`, `walkdir`, `ignore`, `notify`, `serde`. Cross-compilado pra `x86_64-unknown-linux-musl` (binário totalmente estático, zero dependência runtime Linux).

---

## 3. Arquitetura

```
┌──────────────────────────────────────────────────┐
│  Frontend (React, WebView2)                      │
│  state: Zustand   routing: TanStack Router       │
│  IPC: Tauri invoke + event listeners             │
└────────────────────────┬─────────────────────────┘
                         │ Tauri IPC (async)
┌────────────────────────▼─────────────────────────┐
│  oxyris.exe — backend Rust no Windows            │
│                                                  │
│  Domain layer (event-sourced):                   │
│  • Project aggregate                             │
│  • Worktree aggregate                            │
│  • Session aggregate                             │
│  ↓ decide (pure) ↓ apply (pure)                  │
│                                                  │
│  Infra:                                          │
│  • EventStore (SQLite append-only)               │
│  • Projections (SQLite denormalized)             │
│  • SessionSupervisor (Claude child procs)        │
│  • EnvironmentRouter (Windows | Wsl{distro})     │
│  • AgentPool (1 agent process por distro ativa)  │
│  • PathTranslator (Win ↔ POSIX via wslpath)      │
│  • PtyService (portable-pty, 2 targets)          │
│  • Observability (NDJSON trace file)             │
└──────┬──────────────────────────────────┬───────┘
       │ Claude direto (Windows)           │ stdio NDJSON
       │                                   │
       ▼                                   ▼
  claude.exe                      oxyris-agent (WSL)
  auth: %USERPROFILE%\.claude      deploy: ~/.oxyris/bin/
                                   auth: ~/.claude (Linux)
                                   
                                   • spawn claude nativo
                                   • fs walk/watch nativo
                                   • git ops nativo
                                   • PTY nativo (bash -l)
```

### Escolhas chave

- **1 backend orquestrador + N agents** (1 por distro WSL ativa). Não é "2 backends gêmeos"; o agent é um executor, não um servidor completo.
- **Event-sourcing desde o start.** Decider (comando → eventos) + Projector (evento → state) + read models. Agregados: Project, Worktree, Session.
- **Roteamento de ops por environment do projeto** — regra sem exceções:
  - Projeto **Windows** → backend faz todas as ops nativamente (std::fs, git2, Command::new, portable-pty). Zero uso do agent. Zero `\\wsl.localhost`.
  - Projeto **WSL** → backend delega **todas** as ops de filesystem, git, spawn, PTY ao agent dentro da distro. Evita 9P no caminho quente. Backend só toca paths UNC pra UX pontual (ex: formatar pra "open in Explorer").
- **Claude CLI via stdio streaming.** `claude --print --output-format stream-json --input-format stream-json`. Não depende do `@anthropic-ai/claude-agent-sdk` Node. Rust spawna e parseia stream JSON direto.
- **Worktree como cidadão de primeira classe.** Modelo explícito, UI de criação em um clique.

---

## 4. Modelo de domínio (event-sourced)

### Agregados

```rust
// Project = metadados de um repo (Windows ou WSL)
struct Project {
    id: Uuid,
    name: String,
    environment: Environment,
    root_path: String,         // canonical no namespace do environment
    default_branch: String,
    created_at: DateTime<Utc>,
}

enum Environment {
    Windows,
    Wsl { distro: String },
}

// Worktree = git worktree. Cada sessão roda em 1 worktree.
struct Worktree {
    id: Uuid,
    project_id: Uuid,
    path: String,              // caminho absoluto no namespace do project
    branch: String,
    is_primary: bool,          // true para o worktree "raiz" do repo
    created_at: DateTime<Utc>,
    removed_at: Option<DateTime<Utc>>,
}

// Session = 1 conversa Claude rodando em 1 worktree.
struct Session {
    id: Uuid,
    project_id: Uuid,
    worktree_id: Uuid,
    title: String,
    model: ClaudeModel,              // opus-4-7, sonnet-4-6, haiku-4-5
    thinking_mode: ThinkingMode,
    runtime_mode: RuntimeMode,       // full-access | supervised | plan
    status: SessionStatus,           // Idle | Running | Error
    claude_pid: Option<u32>,
    created_at: DateTime<Utc>,
}

// Turn = user input + assistant response + tool uses
struct Turn {
    id: Uuid,
    session_id: Uuid,
    index: u32,
    user_message: Option<UserMessage>,
    assistant_blocks: Vec<AssistantBlock>,      // text | thinking | tool_use | tool_result
    checkpoint_ref: Option<CheckpointRef>,      // git hidden ref: refs/oxyris/cp/<session>/<turn>
    status: TurnStatus,                         // Pending | Streaming | Completed | Failed | Interrupted
    started_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
}
```

### Commands (amostra)

```
Project:
  CreateProject { name, environment, root_path }
  DeleteProject { id }
  RenameProject { id, name }

Worktree:
  CreateWorktree { project_id, branch, path }
  RemoveWorktree { id }

Session:
  StartSession { project_id, worktree_id, model, thinking_mode, runtime_mode }
  StopSession { id }
  InterruptTurn { session_id }

Turn:
  SendUserMessage { session_id, text, attachments }
  ApproveToolUse { turn_id, tool_use_id }
  RejectToolUse { turn_id, tool_use_id, reason }
```

### Events (amostra)

```
ProjectCreated, ProjectDeleted, ProjectRenamed
WorktreeCreated, WorktreeRemoved
SessionStarted, SessionStopped, SessionModelChanged
TurnStarted, TurnUserMessageAppended, TurnAssistantChunkAppended,
TurnToolUseRequested, TurnToolUseCompleted, TurnCompleted, TurnFailed,
TurnCheckpointCaptured
```

### Decider / Projector

Cada agregado expõe:

```rust
trait Aggregate {
    type Command;
    type Event;
    type State;
    type Error;

    fn decide(state: &Self::State, cmd: Self::Command) -> Result<Vec<Self::Event>, Self::Error>;
    fn apply(state: &Self::State, event: &Self::Event) -> Self::State;
}
```

### EventStore (SQLite)

Tabela única append-only:

```sql
CREATE TABLE events (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    aggregate   TEXT NOT NULL,     -- 'project' | 'worktree' | 'session' | 'turn'
    aggregate_id TEXT NOT NULL,    -- UUID
    version     INTEGER NOT NULL,  -- versão dentro do agregado
    kind        TEXT NOT NULL,     -- nome do evento
    payload     TEXT NOT NULL,     -- JSON
    timestamp   TEXT NOT NULL,     -- ISO-8601
    UNIQUE (aggregate, aggregate_id, version)
);
CREATE INDEX events_by_aggregate ON events (aggregate, aggregate_id, version);
CREATE INDEX events_by_seq ON events (seq);
```

### Read models (denormalizados)

```sql
-- Lista de projetos pro sidebar
CREATE TABLE projections_projects (
    id TEXT PRIMARY KEY, name TEXT, environment_kind TEXT,
    environment_distro TEXT, root_path TEXT, session_count INTEGER,
    last_activity_at TEXT
);

-- Sessões recentes pro picker
CREATE TABLE projections_sessions (
    id TEXT PRIMARY KEY, project_id TEXT, worktree_id TEXT,
    title TEXT, status TEXT, model TEXT,
    last_turn_at TEXT, turn_count INTEGER
);

-- Turns de uma sessão pro chat
CREATE TABLE projections_turns (
    id TEXT PRIMARY KEY, session_id TEXT, idx INTEGER,
    status TEXT, user_text TEXT, assistant_text_accum TEXT,
    started_at TEXT, completed_at TEXT
);
```

Projections são reconstruíveis do event log — dão drop e rebuild se o schema da projection mudar.

---

## 5. Comunicação com o Agent (WSL)

> **Quando o agent é usado:** exclusivamente para projetos WSL. Projetos Windows passam longe — o backend executa direto com crates nativas. O agent não é um proxy universal, é a ponta Linux pra projetos que vivem em distros.

### Deploy

1. Instalador MSI inclui `oxyris-agent-linux-x64` em `%APPDATA%\Local\oxyris\agent\`.
2. Na primeira vez que o backend precisa operar numa distro, copia o binário pra `\\wsl.localhost\<distro>\home\<user>\.oxyris\bin\oxyris-agent` e `chmod +x`.
3. Backend spawna: `wsl.exe -d <distro> -- /home/<user>/.oxyris/bin/oxyris-agent`.
4. Agent fica vivo como long-running process. Uma conexão por distro.

### Protocolo

- NDJSON sobre stdio.
- Request/response com `request_id`.
- Streaming de eventos (ex: chunks de `walkdir`) via eventos do tipo `{"kind":"event","request_id":..,"data":..}`.

```jsonc
// → request do backend
{"kind":"request","id":"r-42","op":"fs.walk","args":{"root":"/home/wilson/proj","ignore":[".git","node_modules"]}}

// ← stream de respostas do agent
{"kind":"event","request_id":"r-42","data":{"path":"/home/wilson/proj/src/main.rs","size":4532}}
{"kind":"event","request_id":"r-42","data":{"path":"/home/wilson/proj/Cargo.toml","size":612}}
...
{"kind":"result","request_id":"r-42","data":{"count":1283,"truncated":false}}
```

### Ops expostas pelo agent

- `fs.walk`, `fs.read`, `fs.write`, `fs.stat`, `fs.watch`
- `git.status`, `git.branch_list`, `git.worktree_create`, `git.worktree_remove`, `git.checkpoint_capture`, `git.diff`
- `process.spawn` (pra Claude e para comandos arbitrários como terminal)
- `system.info` (uname, arch, cwd, home)

---

## 6. Estrutura do repositório

```
oxyris/
├── Cargo.toml                      # workspace manifest
├── rust-toolchain.toml             # pinned stable
├── PLAN.md                         # este arquivo
├── README.md                       # público, curto
├── CLAUDE.md                       # instruções pro Claude Code nesse repo
├── .gitignore
├── .github/
│   └── workflows/
│       └── ci.yml                  # fmt, clippy, test, build, tauri build
├── apps/
│   ├── desktop/                    # Tauri app (backend principal)
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── tauri_commands/     # handlers Tauri IPC (thin, delega pra services)
│   │   │   ├── domain/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── project.rs      # Project aggregate
│   │   │   │   ├── worktree.rs
│   │   │   │   ├── session.rs
│   │   │   │   └── turn.rs
│   │   │   ├── infra/
│   │   │   │   ├── event_store.rs
│   │   │   │   ├── projections.rs
│   │   │   │   ├── environment.rs  # Windows | Wsl{distro}
│   │   │   │   ├── agent_pool.rs   # managed WSL agents
│   │   │   │   ├── path_translator.rs
│   │   │   │   ├── git.rs
│   │   │   │   ├── pty.rs
│   │   │   │   ├── claude.rs       # subprocess + stream-json parser
│   │   │   │   └── observability.rs
│   │   │   └── app_state.rs
│   │   ├── tauri.conf.json
│   │   └── build.rs
│   ├── web/                        # React frontend
│   │   ├── package.json
│   │   ├── vite.config.ts
│   │   ├── index.html
│   │   ├── tailwind.config.ts
│   │   └── src/
│   │       ├── main.tsx
│   │       ├── router.tsx
│   │       ├── routes/
│   │       │   ├── __root.tsx
│   │       │   ├── _chat.tsx
│   │       │   ├── _chat.$sessionId.tsx
│   │       │   ├── settings.tsx
│   │       │   └── index.tsx
│   │       ├── components/
│   │       │   ├── chat/
│   │       │   ├── diff/
│   │       │   ├── terminal/
│   │       │   ├── sidebar/
│   │       │   ├── picker/
│   │       │   └── ui/             # shadcn/ui wrappers
│   │       ├── stores/             # Zustand stores
│   │       ├── ipc/                # Tauri invoke wrappers, typed
│   │       └── lib/
│   └── agent/                      # WSL-side Rust helper
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           ├── ops/
│           │   ├── fs.rs
│           │   ├── git.rs
│           │   ├── process.rs
│           │   └── system.rs
│           └── protocol.rs
├── crates/
│   ├── oxyris-core/                # Shared domain types (events, commands)
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   ├── oxyris-ipc/                 # Protocol backend ↔ agent
│   │   └── src/lib.rs
│   └── oxyris-claude/              # Claude CLI protocol adapter (stream-json)
│       └── src/lib.rs
└── scripts/
    ├── build-agent-linux.ps1       # cross-compile agent
    └── package.ps1                 # tauri bundler + sign
```

---

## 7. Sprints / milestones

Cada sprint é "entregável fechado". Auto-validação ao fim: `cargo fmt && cargo clippy -- -D warnings && cargo test && bun run build` (ou equivalente quando Tauri).

### Sprint 1 — Scaffolding (3 dias)

- [ ] `cargo new --bin apps/desktop`, `cargo new --bin apps/agent`, `cargo new --lib crates/oxyris-core`, etc.
- [ ] `pnpm create vite apps/web --template react-ts`
- [ ] Configurar Tailwind v4, shadcn/ui
- [ ] `tauri init` em `apps/desktop`, apontando `frontendDist` pro build do Vite
- [ ] Hello-world: clicar botão no React → invoke Rust → retorna string
- [ ] CI básico no GitHub Actions (fmt + clippy + build)
- [ ] **Validação:** `cargo tauri dev` abre janela, botão responde.

### Sprint 2 — Event store + primeiro agregado (4 dias)

- [ ] `crates/oxyris-core`: `Aggregate` trait, `Event` trait, tipos base (`Uuid`, `AggregateId`)
- [ ] `apps/desktop/src/infra/event_store.rs`: SQLite append-only + query by aggregate id + optimistic concurrency via version
- [ ] Implementar `Project` aggregate: commands `CreateProject`, `RenameProject`, `DeleteProject`; events correspondentes; decider + apply puros
- [ ] Projection `projections_projects` + rebuild-from-events utility
- [ ] Tauri command `project_create(input)` → decide → append events → update projections → return new state
- [ ] **Validação:** testes unit cobrindo decider (casos felizes + inválidos), testes de integração de event store (concorrência, replay).

### Sprint 3 — Environment + WSL discovery (3 dias)

- [ ] `infra/environment.rs`: enum `Environment { Windows, Wsl { distro } }`
- [ ] `wsl.exe --list --verbose` parser (UTF-16 LE BOM → strings UTF-8, filtrar `docker-desktop` e afins)
- [ ] `infra/path_translator.rs`: Windows ↔ POSIX via `wslpath`, canonicalization
- [ ] Tauri command `environments_list()` → `[{ kind: "Windows" }, { kind: "Wsl", distro: "Ubuntu" }]`
- [ ] **Validação:** no Windows real, `environments_list()` retorna Windows + Ubuntu (filtrou docker-desktop).

### Sprint 4 — Agent Rust + protocolo + deploy (5 dias)

- [ ] `apps/agent/src/main.rs`: read NDJSON de stdin, dispatch ops, write NDJSON em stdout
- [ ] `crates/oxyris-ipc`: schemas serde dos requests/responses
- [ ] Ops iniciais: `system.info`, `fs.walk`, `fs.read`, `fs.stat`
- [ ] `apps/desktop/src/infra/agent_pool.rs`: gerencia 1 processo long-running por distro, reconecta em caso de crash
- [ ] Deploy logic: copiar `oxyris-agent-linux-x64` pra `\\wsl.localhost\<d>\home\<user>\.oxyris\bin\` e spawnar via `wsl.exe`
- [ ] `scripts/build-agent-linux.ps1`: cross-compile `x86_64-unknown-linux-musl` usando `cross` crate
- [ ] **Validação:** no Windows, `ipc_fs_walk({ distro: "Ubuntu", root: "/home/wilson" })` retorna lista de arquivos, confirmar no wireshark/trace que não usou 9P.

### Sprint 5 — Provider trait + Claude impl + stream-json parser (5 dias)

- [ ] `crates/oxyris-provider`: `trait Provider` (start_session, send_message → stream, interrupt, list_models, auth_status); tipos compartilhados de evento de assistente (`text`, `thinking`, `tool_use`, `tool_result`).
- [ ] `crates/oxyris-claude`: parser de `--output-format stream-json` + impl de `Provider`. Events: `system`, `user`, `assistant (text | thinking | tool_use)`, `tool_result`, `result`.
- [ ] `apps/desktop/src/infra/claude.rs`: `spawn_claude(environment, cwd, model, ...)` → child + stdin writer + stdout stream
- [ ] Pra Windows: `Command::new("claude.exe")`, pra WSL: via agent (`process.spawn` op)
- [ ] `Session` aggregate: `StartSession`, `StopSession`, `InterruptTurn`, events correspondentes
- [ ] `SessionSupervisor`: mantém mapa `session_id → Box<dyn ProviderSession>`, faz cleanup em `StopSession`
- [ ] **Validação:** UI manda "hello" → vê streaming chegando palavra por palavra. Backend chama o trait, nunca o adapter direto.

### Sprint 6 — Worktree + git integration (4 dias)

- [ ] `apps/desktop/src/infra/git.rs`: git2 para status, branch list, worktree create/remove
- [ ] Pra WSL, agent expõe as mesmas ops (via git2 no Linux)
- [ ] `Worktree` aggregate + commands `CreateWorktree`, `RemoveWorktree`
- [ ] Política: worktrees em `<project_root>/.oxyris/worktrees/<branch-slug>/`
- [ ] Ao `StartSession`, se não informar worktree, usa o primario. UI pode criar worktree paralelo.
- [ ] **Validação:** criar 2 sessões no mesmo projeto em branches diferentes. Confirmar 2 child `claude` processes rodando em dirs separados, cada um sem ver mudanças do outro.

### Sprint 7 — Chat UI (7 dias)

- [ ] Layout: sidebar com projetos/sessões + área principal com thread
- [ ] Composer: Lexical editor, multiline, paste de imagem, slash command autocomplete
- [ ] Thread view: virtualized list, streaming indicator, message actions (copy, regenerate)
- [ ] Markdown renderer com Shiki (lazy-load linguagens), code block com copy
- [ ] Tool use blocks: collapsible, mostra args + result
- [ ] Status bar: model, thinking mode, runtime mode, session state
- [ ] Zustand stores: `sessionStore`, `projectStore`, `uiStateStore`
- [ ] Tauri events: backend emit `session:turn-chunk` → store updates
- [ ] Todas as strings de UI passam por `useTranslation()` — chaves em `apps/web/src/locales/en/chat.json`
- [ ] **Validação:** UX lisa, sem flicker, streaming fluido com 50 msg/s. Trocar locale (en → pt-BR stub) re-renderiza sem reload.

### Sprint 8 — Diff viewer (5 dias)

- [ ] Captura turn-by-turn via git checkpoint hidden refs: `refs/oxyris/cp/<session>/<turn>-pre|post`
- [ ] Diff computation: `git2::Repository::diff_tree_to_tree` entre pre e post
- [ ] Rendering: CodeMirror 6 com tema syntax, modo split + inline
- [ ] Per-file collapsible, search/filter
- [ ] Apply/revert per hunk (pós-MVP talvez)
- [ ] **Validação:** agent altera arquivo, diff mostra mudança com syntax highlight, revert volta o arquivo.

### Sprint 9 — Terminal (3 dias)

- [ ] `portable-pty` no backend Windows pra cmd/powershell
- [ ] Agent expõe `pty.spawn` → spawna `bash -l` com ConPTY Linux
- [ ] xterm.js frontend + addons (fit, search, weblinks)
- [ ] Backend streams pty output como Tauri events
- [ ] **Validação:** terminal funcional em projeto Windows (cmd) e WSL (bash), scroll + resize ok.

### Sprint 10 — Folder picker + project creation UI (4 dias)

- [ ] UI de picker custom com abas: "Windows" | "WSL: Ubuntu" | "WSL: Debian"
- [ ] Backend endpoint `fs.browse(environment, path)` para cada aba
- [ ] Tree virtualized, breadcrumbs, favoritos recentes
- [ ] Modal de criação de projeto: nome + environment + root_path
- [ ] Validação: root_path existe no environment, é diretório, é git repo (opcional criar)
- [ ] **Validação:** criar projeto Windows e WSL via picker, ambos aparecem na sidebar.

### Sprint 11 — Settings + provider discovery (3 dias)

- [ ] Probe Claude em cada environment: `claude --version`, `claude auth status` (JSON output)
- [ ] Filtra shims de interop (se path começa com `/mnt/`, não conta como install nativo)
- [ ] Settings UI: card por installation ("Claude (Windows)", "Claude (Ubuntu)") com version, auth status, botão "Run setup" se faltar
- [ ] Theme (dark/light), keybindings JSON import
- [ ] **Validação:** settings reflete realidade. Se desautenticar claude no Ubuntu, card vira vermelho.

### Sprint 12 — Checkpointing completo + polish (4 dias)

- [ ] Hidden refs capturados no início e fim de cada turn
- [ ] UI: cada turn tem "Revert to before this turn" action
- [ ] Expiração: checkpoints > 30 dias viram garbage collected
- [ ] Observability: NDJSON trace em `%APPDATA%\Local\oxyris\logs\trace.ndjson`
- [ ] Crash reporting: minidump + log upload opt-in
- [ ] **Validação:** causar crash, log capturado, reabrir app → state restaurado.

### Sprint 13 — Installer + auto-updater + bug bash (5 dias)

- [ ] `tauri-bundler` gerando MSI assinado (ou unsigned pra MVP)
- [ ] `tauri-updater` com endpoint próprio (pode ser GitHub Releases)
- [ ] Winget publishing setup
- [ ] Bug bash: criar 20 projetos, 50 sessões, checar perf
- [ ] Memory profiling (heaptrack em WSL, Windows Performance Analyzer)
- [ ] **Validação:** instalar MSI num Windows limpo, rodar 1h de trabalho real sem crash.

### Sprint 14 — Auto-pilot / Supervisor LLM (~6-7 dias)

Auto-pilot **goal-driven**: segundo LLM (Supervisor) recebe uma missão
(spec/changelog colado num painel flutuante) e dirige a sessão até completá-la —
aprova tools e responde perguntas no lugar do usuário. **Pure mode é primário**,
Structured secundário. Autonomia full com guardrails. Design completo em
[`docs/design/autopilot.md`](./docs/design/autopilot.md).

- [ ] `crates/oxyris-supervisor`: `trait Supervisor`, `Mission`, `AutopilotContext`, `Decision` (Approve/Reject/Reply/Done/Escalate)
- [ ] Portar `pureTurn.ts` (stripAnsi + regexes + idle) pra Rust no backend → fix do bug "falha sem foco" (idle/xterm throttle do WebView em background)
- [ ] `PureInputDetector` + atuação no stdin do PTY; `MultiModelSupervisor` (lib `genai`) + `ClaudeProgrammaticSupervisor` (`claude -p` headless)
- [ ] `AutopilotController` + guardrails (denylist hard, loop-detect, budget cap, audit events, kill switch)
- [ ] Structured (2º): consumir `ToolApprovalRequested` + `TurnCompleted` (já existem)
- [ ] UI: botão de auto-pilot no header do `PureSessionView` (ao lado do toggle de terminal) + painel flutuante da missão, seletor de supervisor, denylist/budget, kill switch, mini-log de decisões (i18n)
- [ ] **Validação:** janela **sem foco/minimizada**, sessão Pure recebe spec e roda multi-step sozinha; detecção não quebra; allowlist auto-aprova, denylist escala, budget pausa, kill switch retoma.

---

## 8. Non-goals do MVP

- ❌ Cross-platform (macOS / Linux desktop). Só Windows.
- ⚠️ Outros providers no MVP — só Claude vai com adapter ativo, mas a arquitetura agora expõe um `trait Provider` genérico em `crates/oxyris-provider` desde o Sprint 1. Codex/Cursor/OpenCode plugam o mesmo trait sem refactor.
- ❌ Multi-usuário / sync na nuvem. Local-first estrito.
- ❌ Plugins/extensões de terceiros.
- ❌ Mobile / web.
- ❌ "Cross-namespace override" (projeto WSL usando Claude Windows). Fica pra v2 se alguém pedir.
- ❌ Code editor embutido (quem quer editar abre VS Code). Diff viewer sim, editor não.

---

## 9. Riscos técnicos

| Risco | Mitigação |
|---|---|
| `tauri-updater` assinatura no Windows exige cert | Azure Trusted Signing (pago mas acessível) OU release sem assinatura e SmartScreen warning no MVP |
| Stream-json do Claude mudar formato | Versionar parser, tests e2e com snapshots |
| `wsl.exe` output UTF-16 com BOM | Parser robusto com detecção de encoding + stripping de BOM |
| Agent crashes / leak | Watchdog no backend, restart com backoff exponencial |
| Projeto WSL grande (500k arquivos) | `ignore` crate + respeito a `.gitignore` + paginação na API de walk |
| ConPTY edge cases (escape sequences, resize, cursor) | xterm.js já lida; validar com `vim`, `htop`, `nano` em WSL |
| libgit2 não lida com certos edge cases (LFS, submódulos) | Shellout pra `git` binário pra ops complexas; git2 pra ops básicas |

---

## 10. Pontos de decisão em aberto

- **Nome do binário do CLI do usuário** (se quisermos um). Ex: `oxyris` no PATH pra `oxyris open <path>`?
- **Política de dados do usuário** — onde guarda conversas? `%APPDATA%\Local\oxyris\db.sqlite` parece certo. Backup/export automático?
- **Telemetria** — opt-in pra crash + usage? Ou zero?
- **Default de runtime mode** — Full access ou Supervised? T3 usa Full access por default (claude agent SDK com `bypassPermissions`). Aqui talvez Supervised seja mais sensato por UX.
- **Licença** — MIT, Apache-2.0, proprietária? Se proprietária, algumas crates GPL não podem ser usadas (checar `git2`'s mbedTLS feature etc).

---

## 11. O que olhar no T3 Code como referência (não copiar)

- `apps/server/src/orchestration/decider.ts` + `projector.ts` — modelo ES bem feito
- `apps/server/src/checkpointing/` — git hidden refs pattern
- `apps/server/src/keybindings.ts` — `when`-clause grammar (boa ideia pra replicar)
- `apps/web/src/components/ChatView.tsx` + `DiffPanel.tsx` — layouts
- `apps/web/src/wsTransport.ts` — state machine de conexão (parte não vamos precisar porque Tauri IPC é síncrono, mas lógica de retry é bom reference)
- `packages/contracts/src/orchestration.ts` — inspiração pros schemas de eventos

**Evitar copiar:**

- `effect-codex-app-server` (37k linhas de binding Codex)
- `effect-acp` (Agent Client Protocol — não precisamos)
- Dependência pesada em Effect TS (bom runtime, mas complica onboarding e custa context)
- Multi-provider abstração over-engineered (começamos só com Claude)

---

## 12. Setup inicial (quando começar)

Pré-requisitos no Windows:

```powershell
# Rust
winget install Rustlang.Rustup
rustup toolchain install stable
rustup target add x86_64-pc-windows-msvc x86_64-unknown-linux-musl

# Cross-compile pra Linux (agent)
cargo install cross --git https://github.com/cross-rs/cross

# Node via Volta ou direto (pra frontend apenas)
winget install OpenJS.NodeJS.LTS

# pnpm
npm install -g pnpm

# Tauri CLI
cargo install tauri-cli --version "^2.0.0" --locked

# WebView2 (já vem no Win10+, verificar)
# Visual Studio Build Tools (pra compilar crates com C deps)
winget install Microsoft.VisualStudio.2022.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Pré-requisitos no WSL Ubuntu (só pra dev/test do agent):

```bash
# Não precisa nada se só rodar o binário musl estático
# Mas pra desenvolvimento local do agent:
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add x86_64-unknown-linux-musl
sudo apt install musl-tools pkg-config libssl-dev
```

---

## 13. Como continuar depois do /clear

Esse arquivo é o source of truth. Após /clear:

1. Claude novo lê `PLAN.md` + `CLAUDE.md` da pasta `oxyris/`.
2. Verifica qual sprint está "in progress" (checkboxes em Section 7).
3. Usa o contexto de design (Sections 2-6) sem questionar decisões já fechadas.
4. Foca só em entregar o próximo milestone e validá-lo.

**Regras de engajamento pro futuro Claude:**

- Não re-questionar escolha de stack (Tauri + Rust + React) — decidido.
- Não propor data model flat "pra simplificar" — event sourcing é pilar.
- **Roteamento por environment é regra dura:**
  - Projeto Windows → ops nativas no backend (std::fs, git2, Command::new, portable-pty). **Nunca** rotear por agent.
  - Projeto WSL → todas as ops (fs, git, spawn, PTY) via agent. **Nunca** acessar `\\wsl.localhost\...` fora de UX pontual (ex: render de path pra Explorer) e do deploy inicial do agent.
  - Não há "fallback cruzado". Se o agent de uma distro está down, ops daquele projeto falham até voltar — não fingir com UNC.
- Preferir `git2` pra ops core; só shellout pra git binário quando git2 não suportar.
- Não introduzir Electron, Node runtime no backend, nem shell scripts bash — Rust e PowerShell (quando Windows-only) apenas.
- Commits só quando usuário pedir.

---

## Changelog

- **2026-06-05** — Sprint 14 (Auto-pilot) **parte 4 (integração ponta-a-ponta) entregue**. Backend: `infra/pty.rs` ganhou canal de notice in-process (`PureSignalNotice` + `set_signal_sink` + `scrollback_tail`) — o reader empurra sinais pro controller além do evento Tauri (funciona com janela sem foco). `infra/autopilot.rs`: `AutopilotManager` (mapa por sessão de `EngagedSession` com `Autopilot` serializado por `tokio::Mutex`; loop `run` drena o canal e despacha por sessão; monta contexto do `scrollback_tail`, roda `Autopilot::step`, atua no stdin do PTY — Approve=`\r`, Reject=Esc+motivo, Reply=texto+`\r` separado pra evitar paste-burst; emite `session:<id>:autopilot` pro mini-log; Halt desengata). Dois supervisors concretos: `OpenAiCompatSupervisor` (reqwest a qualquer `/chat/completions` — OpenAI/OpenRouter/Groq/Ollama = "multi-modelo") e `ClaudeCliSupervisor` (`claude -p` headless via spawn_blocking + HideConsole); `parse_decision` tolera fences/prosa (5 testes). `AppState.pty` virou `Arc<PtySupervisor>`; `AppState.autopilot: Arc<AutopilotManager>` com `run` spawnado no boot. Comandos Tauri `autopilot_engage`/`autopilot_disengage`. Frontend: `ipc/autopilot.ts` (engage/disengage + `onAutopilotEvent`), `autopilotStore` estendido (config: supervisor/model/baseUrl/apiKey/maxTurns persistidos + log de decisões), `AutopilotPanel` funcional (campos condicionais por supervisor, engage/disengage, mini-log, kill switch), listener no `PureSessionView` (acumula log + auto-desengata no halt/error). i18n en+pt-BR. Gate verde: fmt, clippy `-D warnings`, cargo test (todos), typecheck, build. **Falta:** teste em runtime (`bun run tauri dev`) — engatar piloto numa sessão Pure e validar atuação real (não dá pra validar headless).
- **2026-06-05** — Sprint 14 (Auto-pilot) **parte 3 (core do supervisor) entregue**. Novo crate `crates/oxyris-supervisor` (no workspace): `trait Supervisor` (async via `async-trait`, object-safe) + tipos `Mission`/`TranscriptView`/`PendingKind` (Permission|TurnEnded)/`AutopilotContext`/`Decision` (Approve/Reject/Reply/Done/Escalate)/`SupervisorKind`. Módulo `guardrails` (puro, testado): `Denylist` (regex hard-block de rm -rf, force push, hard reset, mkfs, dd of=/dev, fork bomb, chmod -R 777, curl|sh, shutdown, terraform destroy, leitura de id_rsa/.env/.pem; combinado e flags separados), `escapes_worktree` (path traversal/abs fora do cwd), `LoopGuard` (fingerprint normalizado + repeat-limit + step-cap), `Budget` (cap de turns). Módulo `controller`: `Autopilot` = máquina de estado por sessão — `pre_check` (denylist→loop, puro) → `Supervisor::decide` (async) → `post_decision` (mapeia Decision→`Action` {Approve/Reject/Reply/Halt}, cobra budget só em reply de turn-end, re-checa denylist no Approve como defesa em profundidade); `HaltReason` {Done/Escalated/Denylisted/Looping/BudgetExhausted}. 19 testes. **Pendente (parte 4, precisa app rodando pra validar):** impls concretas `MultiModelSupervisor` (genai) + `ClaudeProgrammaticSupervisor` (`claude -p`); `AutopilotController` no desktop (consumir `pure-signal` no backend, montar contexto do scrollback do PTY, rodar `Autopilot::step`, atuar no stdin do PTY, emitir decisões pra UI, desengatar no Halt); comando Tauri pra missão/engage ligando o `autopilotStore` ao backend.
- **2026-06-05** — Sprint 14 (Auto-pilot) **partes 1–2 entregues**. **Parte 1 (fix do bug "falha sem foco" + fundação):** `apps/desktop/src/infra/pure_signals.rs` — porta Rust de `pureTurn.ts` (stripAnsi + stripTipLines + regexes prompt/poll/turn-end/working/recap + `PureSniffer` com latches por turno), 14 testes. Ligado no reader do PTY (`infra/pty.rs`): sniffer por PTY claude alimentado a cada chunk, emite `session:<id>:pure-signal` (`NeedsInput`/`TurnEnded`/`Working`); `IdleState` + watchdog thread (fallback de turn-end em silêncio de 2500ms, imune a foco); reset/arm no `write` ao `\r`. Frontend trocado pra consumir os eventos (`ipc/terminal.ts::onPureSignal`): `PureClaudePanel`, `App`, `Sidebar` agora reagem aos sinais do backend em vez de sniffers+`setTimeout` throttled; `apps/web/src/lib/pureTurn.ts` deletado. Resolve a falha de detecção quando a janela perde foco (WebView throttla timers/render de background). **Parte 2 (UI do auto-pilot):** botão `Bot` no header do `PureSessionView` (ao lado do toggle de terminal, verde quando engajado) abre `AutopilotPanel` (popover flutuante) com textarea de **missão** (spec/changelog), seletor de supervisor (multi-modelo / Claude headless), engage + kill switch; `stores/autopilotStore.ts` persiste missão+enabled por sessão em localStorage; i18n en+pt-BR. **Pendente (parte 3):** `crates/oxyris-supervisor` (trait + `Decision`), `MultiModelSupervisor` (genai) + `ClaudeProgrammaticSupervisor`, `AutopilotController` + guardrails (denylist/loop-detect/budget/audit), atuação no stdin do PTY. Gate verde: fmt, clippy `-D warnings`, cargo test, typecheck, build.
- **2026-06-05** — Design do **Auto-pilot / Supervisor LLM** (Sprint 14, proposta) em `docs/design/autopilot.md`. Auto-pilot **goal-driven**: segundo LLM (Supervisor, plugável: lib multi-modelo `genai` ou `claude -p` headless) recebe uma missão (spec/changelog colado num painel flutuante, botão no header do `PureSessionView` ao lado do toggle de terminal) e dirige a sessão até completá-la — aprova tools e responde perguntas. **Pure mode é o caminho primário**, Structured secundário. O bug "falha sem foco" foi diagnosticado: a detecção de prompt já existe (`apps/web/src/lib/pureTurn.ts`: regexes + sniffer) mas roda no frontend, onde o WebView faz throttle do idle-`setTimeout` e do render do xterm em background — fix = portar a detecção + idle pro backend Rust (PTY é pipe de bytes, imune a foco). Structured já tem os hooks (`ToolApprovalRequested` → `approve_tool_use`/`reject_tool_use`; `TurnCompleted`). Full autonomy condicionada a guardrails: denylist hard, loop-detect, budget cap, kill switch, audit trail event-sourced. `Decision` = Approve/Reject/Reply/Done/Escalate. Nenhum código escrito.
- **2026-04-22** — Plano inicial. Stack Tauri+Rust+React, event sourcing, agent WSL pattern decidido. Nenhum código escrito.
- **2026-04-22** — Adicionado `crates/oxyris-provider` (trait genérico) ao workspace; Claude vira primeira impl mas não é mais hardcoded. Adicionado i18n com react-i18next (base `en`) ao frontend desde o Sprint 1. Tooling: `bun` no lugar de `pnpm`; `@tauri-apps/cli` no lugar de `cargo install tauri-cli`.
- **2026-04-23** — Sprints 1–7 entregues em modo autônomo. 44 testes passando. Pendências cross-cutting: cross-compile musl do agent precisa Docker + `cross` (script pronto em `scripts/build-agent-linux.ps1`); git layer em WSL ainda shell-out via `wsl.exe` em vez de agent op (refactor futuro); Lexical composer e Shiki adiados (textarea + react-markdown no MVP do chat).
- **2026-04-23** — Sprints 10, 11, 8 fechados. Sprint 10: native folder picker (Windows) via `tauri-plugin-dialog`, validação de path (exists/is_dir/is_git_repo) com filtro `/mnt/*` para WSL. Sprint 11: SettingsPanel com provider discovery (`claude --version` em cada env, detecção de interop shim). Sprint 8: checkpointing turn-by-turn via `git stash create` + `refs/oxyris/cp/<session>/<turn>-{pre,post}`, diff per-file no `<TurnDiffView>`. Pendências: WSL não captura checkpoint (precisa agent.git.* — Sprint 6.5 futuro); CodeMirror 6 / split view ficou pra depois (renderização atual é unified diff colorido); GC de checkpoints velhos existe mas não está agendado (vira cron no Sprint 12).
- **2026-04-23** — Bug fixes pós-Sprint-7: `SessionSupervisor` agora aplica eventos na projection direto (era source de "session_get retorna null"); Claude CLI no Windows resolvido via `which::which("claude")` + cmd.exe wrapper pra `.cmd`/`.bat`; `ProjectStore` zustand compartilhado entre App+ProjectPanel auto-refresh sem reload; UI mostra form "Iniciar sessão" quando sessão ativa está stopped/errored; reconcile no boot vira sessões fantasma `running` em `stopped` (kill_on_drop não persiste SessionStopped).
- **2026-04-23** — Layout polish + Sprint 12 partial + Sprint 9 (terminal). Layout ganhou tabs (Chat / Terminal / Settings) e sidebar fixa de projetos com lista expandida de sessions por projeto ativo. ProjectPanel virou conteúdo de modal acionado pelo "+ Novo projeto" no sidebar. Turn revert via `git read-tree -u --reset <pre-sha>` (preserva HEAD). Terminal: `infra/pty.rs` (portable-pty + ConPTY, pwsh.exe → powershell.exe → cmd.exe), comandos `terminal_spawn/write/resize/kill`, frontend xterm.js + addon-fit. WSL no PTY ainda retorna `NotSupported` (precisa agent op pty.spawn).
- **2026-04-23** — Session resume (`--resume <id>`). Terminal per-session (cwd = worktree.path ?? project.root_path), mount num painel fixo abaixo do chat em vez de tab separada. Correções de crash: `PtySupervisor::kill` virou fire-and-forget em thread separada (drop do Box&lt;MasterPty&gt; não bloqueia mais o IPC). xterm downgrade→upgrade 5.5→6.0 + fila serial de transições.
- **2026-04-23** — Boot rápido: removido o `projections.rebuild_from` do boot (projeções são persistentes), reconcile de sessões phantom-running agora consulta a projection direto (`status = 'running'`) em vez de varrer o event log inteiro. Observability: `tracing-appender` daily-rolling NDJSON em `<data_dir>/logs/trace.ndjson.*`, layer paralelo pro stderr mantido. GC dos checkpoints roda em background 30s após o boot + a cada 24h (`checkpoint::gc(30 dias)`). Settings mostra o path dos logs.
- **2026-04-23** — Worktree UI: lista na sidebar por projeto ativo, "+ Novo worktree" com prompt de branch, botão de remover (não-primary). Session start form ganhou dropdown de worktree (default: raiz do projeto).
- **2026-04-23** — Session titles: `SessionData.title`, `SessionCommand::Rename` + `SessionEvent::SessionRenamed`, auto-title da primeira mensagem (60 chars), inline rename na sidebar (✎ ou double-click), coluna `title` na projection. Sprint 13 básico: `tauri.conf.json` bundle completo (copyright, homepage, licenseFile, webviewInstallMode=downloadBootstrapper, NSIS currentUser), `scripts/package.ps1` gera MSI + NSIS em `./release/`. Signing fica pra quando houver certificado.
- **2026-04-24** — Cleanup órfão de containers Oxyris no boot (#62). `infra/docker_cleanup.rs` enumera projetos, deriva o set live de worktree short_ids, lista todos os containers oxyris-tagged via `docker ps -a --filter label=com.docker.compose.project --format ...` (Windows direto / WSL via wsl.exe), e pra cada `oxyris_<short>` órfão (não bate com worktree existente) faz `docker rm -f` + `docker volume rm -f` + `docker network rm` filtrando pela mesma label. Operação dedupe por env (Windows + por distro). Spawn como background task no boot (5s delay), best-effort com tracing — daemon hung não bloqueia startup. CleanupReport agrega counts pra log/UI futura.
- **2026-04-24** — Dotenv merge per-worktree (#63). Convenção: usuário mantém `.env` normal + `.oxyris/.env.template` (só os deltas com placeholders `${OXYRIS_*}` e `${VAR:-fallback}`). Oxyris gera `.env.local` = merge(.env + template) com template-wins, expansão de OXYRIS_WORKTREE_ID/SHORT/DOCKER_PROJECT/PORT_OFFSET/COMPOSE_FILE. Header sentinel `# oxyris:dotenv-managed` permite detectar edição manual e respeitar (não sobrescreve). Pure parser/merger/substituter em `crates/oxyris-git/src/dotenv.rs` com 7 testes unitários (parse, merge keys/comments, substitute com fallback, export prefix). Backend `infra/dotenv_render.rs` faz IO Windows direto / WSL via agent (`fs.read` + nova op `fs.write`). Tauri command `env_dotenv_render_for_worktree` retorna RenderOutcome (Generated/NoTemplate/ManualOverride). Auto-trigger: ao criar worktree (Sidebar + ChatPanel via `runAutoActionsOnWorktreeCreate` que agora também renderiza); antes de `env_up_for_worktree` (sync .env.local antes do docker compose ler). Agent ganha op `fs.write` (em ops/fs.rs) pra suportar WSL writes.
- **2026-04-24** — Docker per-worktree env (#61). Convenção `.oxyris/compose.yml` na raiz da worktree. `infra/env_template.rs` com detector (Windows: fs direto; WSL: `agent.fs.stat` no agent), `docker_project_name` (`oxyris_<short_id>`), `port_offset` (hash mod 1000), `env_vars` injetadas (OXYRIS_WORKTREE_ID/SHORT/DOCKER_PROJECT/PORT_OFFSET/COMPOSE_FILE). Session aggregate ganha `env_mode: EnvMode { Default | Worktree }` no SessionData + SessionStarted event + `SessionCommand::SetEnvMode` + `SessionEvent::SessionEnvModeChanged` (#[serde(default)] pra back-compat com event log antigo). `PtySupervisor::spawn_with_env` aceita extra_env: PowerShell pega via Command env; WSL forward via `WSLENV=NAME/u:OTHER/u`. `terminal_spawn` virou async, detecta template + injeta OXYRIS_* automaticamente quando session.env_mode=worktree. 4 commands Tauri novos (`env.rs`): `env_template_for_worktree`, `env_status_for_worktree` (filtra `docker ps --filter label=com.docker.compose.project=...`), `env_up_for_worktree`, `env_down_for_worktree` (spawn terminal + escreve `docker compose -f X -p Y up -d/down -v`). Chip Env no composer aparece só quando worktree tem template; mostra dot 🟢/🔴 baseado em status polling 5s; seletor Default/Worktree; botões inline up/down quando em modo worktree. SessionSnapshot no frontend ganha env_mode; sessionStore aplica SessionEnvModeChanged.
- **2026-04-24** — WSL→agent migration completa (#60). Novo crate `crates/oxyris-git` com toda lógica git2 pura compartilhada (worktree create/list/remove + branches + checkpoint capture/diff/revert/gc + types BranchInfo/WorktreeRef/FileStatus/FileDiff/TurnDiff/CheckpointPhase). Agent ganha git2 vendored como dep e novos handlers em `apps/agent/src/ops/git.rs` que delegam pro shared crate. 7 ops novas no `oxyris-ipc`: `git.{list_branches,list_worktrees,create_worktree,remove_worktree,checkpoint_capture,checkpoint_diff,checkpoint_revert}`. Desktop `infra/git.rs` e `infra/checkpoint.rs` viraram facades async: Windows roda oxyris_git em spawn_blocking; WSL despacha pra `state.agent_pool.call(...)`. AgentPool agora é `Arc<AgentPool>` em AppState (compartilhado com SessionSupervisor). Eliminou todo wsl.exe shell-out de git/checkpoint — `wsl.exe` só sobra pra (a) spawn do Claude CLI e (b) PTY ConPTY (objetivamente melhor que agent pra stdio real-time). Dockerfile do agent atualizado: cmake/make/perl/musl-gcc env vars pra libgit2 vendored compilar contra musl. Resultado: WSL diff de turn ~50ms (era 2-5s), worktree create ~200ms (era 800ms), list_branches ~30ms (era 400ms). Fidelidade arquitetural plena com PLAN.md §3-5.
- **2026-04-23** — Auto-updater + WSL checkpoint + chat rendering Claude-Code-style. **Updater**: `tauri-plugin-updater` + `@tauri-apps/plugin-updater`, feature `protocol-asset` já habilitada, capability `updater:default`. `~/lib/updater.ts` engole erro de pubkey placeholder / endpoint 404 como `disabled`. Settings General: current version via `getVersion()`, botão "Check now", banner de update disponível com release notes + "Install and restart", mensagens separadas pra up_to_date/disabled/error. Auto-check no boot via `useUpdaterStore`, dot verde no ícone de Settings quando há update. Ativação: rodar `bun tauri signer generate`, trocar o placeholder `REPLACE_WITH_REAL_KEY_FROM_...` no tauri.conf.json, publicar `latest.json` + MSI/NSIS no GitHub Releases. **Som de notificação** (#59): `notification.mp3` em `src/assets/`, `~/lib/notificationSound.ts` com `HTMLAudioElement` singleton, toca em TurnCompleted/Failed/Interrupted só quando `!document.hasFocus()`, toggle default-on em Settings General. **WSL checkpoint** (#58): `infra/checkpoint.rs` agora tem `capture_wsl`/`revert_to_pre`/`diff_wsl` que shellout `wsl.exe -d <distro> -- git -C <repo> ...` pras ops `stash create` / `update-ref` / `rev-parse` / `read-tree` / `diff --raw -z` / `cat-file -p`. WSL agora captura diff completo por turn, mesma UX de Windows. **Chat rendering Claude-Code-style** (#50): novo `ToolCallView.tsx` dispatcher por nome da tool. Pair `tool_use`+`tool_result` via `tool_use_id`. Renderers especializados: Edit/Update (header + mini-diff inline via `diff.diffLines`, contagem +N/-M, context trimming), Write/Create, Read (com range L1-L50 quando aplicável), Bash (header `$ comando` + stdout), Grep/Glob (pattern + match count), TodoWrite (checklist com ícones por status), Task (subagent com timer rodando), WebFetch/WebSearch, fallback genérico. Layout `ToolRow` com dot colorido (amber running / emerald ok / red error) + ícone lucide + título + subline + conteúdo colapsável. `TurnBody` percorre blocks, mapeia pairs, text/thinking passam inline.
- **2026-04-23** — Sprint de backlog: Web Speech mic (hook `useSpeechRecognition` + pill animada + erro em banner), scroll-to-bottom inteligente (sticky anchor + pill "N novas"), Ctrl+V paste de imagem (Tauri command `attachment_save` em `<data_dir>/attachments/<bucket>/<uuid>.<ext>` + chips thumbnail no composer + `@<path>` prefixado no send). Rendering da imagem no bubble do usuário via `convertFileSrc` + asset protocol habilitado em tauri.conf.json (scope `$APPDATA/**`/`$LOCALAPPDATA/**` + feature `protocol-asset`). Sidebar search estendida pra filtrar threads (sessions liftadas pra o Sidebar). Composer textarea auto-grow capped em 240px, `resize-none` (era `resize-y` mostrando handle como linha). Chips do bottom bar compactados: label só no tooltip. Backend `session_turn_diff` agora devolve `TurnDiff{files:[]}` quando ref de checkpoint não existe (era erro). Action aggregate event-sourced (Register/Update/Remove), projection `projections_actions`, tauri commands `action_list/upsert/delete`, frontend `actionsStore.ts` migrado de localStorage pra IPC, 3 testes unitários. Auto-run em worktree create: helper `runAutoActionsOnWorktreeCreate` spawna tab de terminal pra cada action com flag ligada. Keybindings JSON aplicados: `useKeybindingsStore` + helper `matchesKey/isTypingTarget`, App.tsx usa `bindings.new_thread|toggle_terminal|focus_search`, composer Esc usa `bindings.interrupt`, Settings recarrega após salvar. WSL terminal funciona: `PtySupervisor::spawn` roteia pra `wsl.exe -d <distro> --cd <path>` via ConPTY (sem agent). Follow-ups: Claude-Code-style chat rendering (task #50), auto-updater (diferido — depende de signing), checkpoint WSL via wsl.exe shell-out.
- **2026-04-23** — UX redesign T3-style (Fases A→H em modo autônomo). **A**: frameless window (`decorations: false`), `TitleBar.tsx` custom com drag region + min/max/close via `getCurrentWindow()` API, capabilities `core:window:allow-*`. **B**: sidebar tree com search (Ctrl+K placeholder), projeto badge colorido (`ProjectBadge.tsx`), threads aninhadas com status dot + título + relativa, worktrees como subseção colapsável, settings cogwheel no rodapé. **C**: thread full-height (`min-h-0 flex-1`), composer card grande no rodapé com bottom-bar contendo model/runtime/thinking/workspace chips, ícones lucide-react. Send único cria sessão se não houver e envia primeira mensagem. **D**: 4° runtime mode `AcceptEdits` adicionado ao `RuntimeMode` enum + mapping correto pra `--permission-mode default/acceptEdits/bypassPermissions/plan`. **E**: terminais persistentes multi-tab por sessão (`PtySupervisor` ganhou `session_id`/title/cwd, `terminal_list` + `terminal_rename` commands, `TerminalPanel` virou dock com tabs e PTY isolada por tab; switch de session/dock-close não mata PTY). **F**: project actions configuráveis (`actionsStore.ts` localStorage por projeto, `ProjectActionsBar.tsx` na header do chat, modal de gerenciamento, atalhos via parser `Ctrl+Shift+B`-style). Persistência via aggregate Action fica como follow-up. **G**: Settings 2 tabs (General: idioma/update track/providers; Advanced: keybindings JSON editor + logs path + devtools hint). Backend: `settings_keybindings_path/read/write` (valida JSON antes de gravar em `<data_dir>/keybindings.json`), `AppState.data_dir` exposto. **H**: tema JetBrains Island Dark — neutral palette redefinida no `@theme` (#19191c/#1e1f22/#2b2d30/#393b40/#6f737a/#c4c7cf), accent tokens, scrollbar slim, xterm cursor azul accent.
