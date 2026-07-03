# Design — Lifecycle de processos WSL via agent

> Status: proposta. Código zero.
> Data: 2026-07-03.
> Encaixa como sprint próprio no [`PLAN.md`](../../PLAN.md) §7. Source of truth continua o PLAN.md.
> Pré-requisito já entregue: reaping determinístico + idle-reaper (commit `4ec9428`).

## 1. Problema

Projetos `Environment::Wsl` viravam um balão de memória: rust-analyzer (5 GB),
node (2 GB) e várias sessões `claude` (~400 MB cada) acumulavam **dentro do
distro** até o WSL estourar a RAM do host e travar o Windows. Só `wsl --shutdown`
resolvia.

Causas atacadas em `4ec9428` (L1–L3):

- **L1** — idle-reaper de LSP (15 min sem query → shutdown, respawn lazy).
- **L2** — `PtySupervisor::kill_all` no exit do app.
- **L3** — `pkill` do `claude` Linux por marcador (session-id) dentro do distro.

### O gap que sobra (motivo deste doc)

L1–L3 são **remendos sobre uma arquitetura errada**. Hoje, para projetos WSL, o
desktop faz spawn de PTY e LSP via `wsl.exe` direto:

- PTY: `wsl.exe -d <distro> -- sh -lc "exec claude …"` (ConPTY hospeda no lado
  Windows, `wsl.exe` é um **relay**).
- LSP: `wsl.exe -d <distro> -- bash -lc "exec rust-analyzer …"` com stdio piped.

Matar esses processos do lado Windows mata só o **relay `wsl.exe`**. O processo
Linux real pode sobreviver órfão — daí o `pkill` por marcador do L3, que:

- só cobre `claude` (tem UUID único no cmdline); LSP não tem marcador único e
  hoje conta com "morre no EOF do stdin" — frágil;
- é uma corrida (mata o relay, depois faz pkill);
- viola a regra de roteamento do PLAN.md §13: **WSL → o agent faz todo spawn/PTY**.
  Hoje PTY e LSP furam o agent.

O agent já roda **dentro do distro**. Se ele for o pai desses processos, matá-los
é um SIGKILL nativo de process-group — **zero órfão, zero relay, zero `pkill` por
string**. É o fix de raiz.

## 2. Objetivo

Rotear spawn / IO / kill de **PTY** e **LSP** de projetos WSL pelo agent, sobre o
protocolo NDJSON que já existe (`crates/oxyris-ipc`). O agent hospeda os
processos como filhos próprios; o desktop vira só um proxy que reconstrói a mesma
semântica de `LiveTerminal` / `LspClient` a partir do stream.

Não-objetivo: mudar o caminho `Environment::Local` (ConPTY nativo continua). Nem
mexer na UI. A fronteira `LiveTerminal` (eventos `terminal:<id>:output/exit`,
`session:<id>:pure-state`) fica **idêntica**, então mobile-takeover, autopilot
(pure-signals) e checkpointing continuam funcionando sem alteração.

## 3. Protocolo (novas ops em `oxyris-ipc`)

O padrão de streaming já existe: `fs.watch` mantém o request aberto e emite
frames de evento sob o `request_id` até um `fs.unwatch` cancelar (ver
`apps/agent/src/main.rs:56`). PTY usa o mesmo shape.

### PTY

- `pty.spawn { cwd, cols, rows, program: Shell | Claude(opts), env: [(k,v)] }`
  — abre request longo. O agent faz openpty (portable-pty no Linux), spawna o
  programa como filho, e **streama** frames `PtyOutput { seq, data_b64 }` sob o
  `request_id`. Retorna primeiro um `PtySpawned { terminal_id }` (não um Result
  que fecharia a rota, igual `fs.watch`).
- `pty.write { terminal_id, data_b64 }`
- `pty.resize { terminal_id, cols, rows }`
- `pty.kill { terminal_id }` — SIGKILL no process-group do filho. Nativo,
  determinístico. Emite `PtyExit { terminal_id, reason }` e fecha o stream.

Bytes viajam base64 (NDJSON é texto; output de PTY é binário/ANSI). `seq` espelha
o `ReplayBuffer.last_seq` de hoje para dedup no re-attach.

### LSP

Duas opções:

- **(A) genérica `proc.*`** — `proc.spawn`/`proc.write`/`proc.kill` streamando
  stdout/stderr; o desktop roda o framing LSP (Content-Length) sobre isso. Mais
  reuso (serve pra qualquer subprocesso futuro).
- **(B) `lsp.*` dedicada** — o agent fala LSP e devolve resultados tipados.

Recomendação: **(A)**. `LspClient` já faz o framing; só troca o transporte de
"stdio do wsl.exe" por "frames `proc.*` do agent". Menos superfície nova.

## 4. Slices (cada uma compila e é verificável isolada)

1. **Protocolo** — structs + `op_name` em `oxyris-ipc::ops` (PTY + `proc.*`).
   Aditivo puro, sem consumidor. Verifica: `cargo build -p oxyris-ipc`.
2. **Host no agent** — `apps/agent/src/ops/pty.rs` + `proc.rs`. Depende de
   `portable-pty` no target musl (validar que cross-compila — provável risco).
   Verifica: teste no distro (spawn `cat`, escreve, lê echo, mata, confirma
   `ps` sem órfão). **Requer build Linux — não dá no host Windows.**
3. **Desktop: PTY dual-path** — `PtySupervisor` ganha ramo WSL que fala com
   `agent_pool` em vez de ConPTY+wsl.exe, remontando `LiveTerminal` a partir do
   stream `PtyOutput`. O reader thread vira uma task que consome frames do agent.
   Mantém pure-sniffer/idle/replay idênticos. Verifica: `tauri dev`, sessão pura
   WSL, digitar/rodar comando, fechar → `wsl -e ps` sem órfão.
4. **Desktop: LSP dual-path** — `LspClient::spawn_wsl` passa a usar `proc.*`.
   Verifica: hover/find-refs numa sessão WSL; remover worktree → rust-analyzer
   some do `ps` na hora.
5. **Remover os remendos** — o `pkill` por marcador do L3 e o `wsl_kill` viram
   desnecessários (o kill nativo cobre). Manter L1 (idle-reaper) e L2 (exit) —
   continuam válidos e agora matam de verdade via agent.

## 5. Riscos / pontos abertos

- **portable-pty em musl** — confirmar cross-compile do agent (target
  `x86_64-unknown-linux-musl`). Se não rolar, usar `nix`/`libc` direto (openpty +
  fork/exec + setsid) no agent. **Bloqueador potencial da slice 2.**
- **Latência de IO** — hoje ConPTY é in-process; via agent cada chunk cruza
  NDJSON. Coalescer no lado agent (já fazemos debounce no watcher) e manter o
  `OUTPUT_BROADCAST_CAP`. Provável imperceptível pra TUI, medir mesmo assim.
- **Reconexão do agent** — se o agent cair, os PTYs morrem junto (são filhos
  dele). Aceitável e alinhado ao PLAN.md ("agent down → ops falham até voltar,
  sem fallback UNC"). UI deve mostrar sessão como morta e permitir respawn.
- **Consumidores de `LiveTerminal`** — mobile (`infra/mobile.rs`), autopilot
  (`infra/pure_signals.rs`, `autopilot.rs`), checkpoint. Todos consomem a
  fronteira de eventos, não o ConPTY. Se a slice 3 preservar os mesmos eventos,
  nada muda neles. **Regressão a vigiar na verificação da slice 3.**

## 6. Build cross-target

O agent é `x86_64-unknown-linux-musl`, então slices 2/4 (código do agent) **não
buildam/testam no host Windows** — precisam de ambiente Linux (WSL serve) ou CI.
Slices 1 e 3 (desktop) buildam no host normalmente.
