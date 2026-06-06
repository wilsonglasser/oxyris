# Design — Auto-pilot (Supervisor LLM)

> Status: proposta. Código zero.
> Data: 2026-06-05 (rev 2).
> Encaixa como **Sprint 14** no [`PLAN.md`](../../PLAN.md) §7. Source of truth continua o PLAN.md.

## 1. Problema

Quando o Claude precisa de interação humana (aprovar tool ou pedir
opinião/clarificação), a sessão para esperando o usuário. O auto-pilot põe um
segundo LLM (o **Supervisor**) que recebe contexto + uma **missão** (spec /
changelog do que deve ser feito) e responde no lugar do usuário, dirigindo a
sessão até completar a missão.

Decisões fechadas (Q&A 2026-06-05):

- **Pure mode é o caminho PRIMÁRIO. Structured é secundário.**
- Autonomia **full**: aprova permissões **e** responde perguntas abertas.
- Supervisor **plugável**: (a) lib Rust multi-modelo (usuário escolhe o modelo)
  ou (b) Claude Code programático (`claude -p` headless).
- **Goal-driven, não só reativo.** Botão de auto-pilot no header abre um painel
  flutuante onde o usuário cola a missão (spec do que está sendo feito,
  changelog do que falta, etc). O Supervisor usa isso como objetivo e empurra a
  sessão Pure rumo a ele.

## 2. O bug "falha sem foco" — diagnóstico e fix

Pure mode **já tem** detecção de "precisa de input": `detectPrompt` no
`PureClaudePanel.tsx` + as regexes em `apps/web/src/lib/pureTurn.ts`
(`PURE_PROMPT_RE`, `PURE_POLL_RE`, `PURE_TURN_END_RE`, `PURE_WORKING_RE`,
`createPromptSniffer`). Funciona farejando os bytes crus do PTY.

Mas roda no **frontend**, e é aí que quebra sem foco:

- O idle-timer (`window.setTimeout`, `IDLE_DONE_MS`) é **throttled pelo
  WebView** quando a janela está em background → "turn done" e clears atrasam ou
  não disparam.
- `TerminalView.onOutput` depende do ciclo de render do xterm, que também
  desacelera oculto.

**Fix:** mover detecção + idle pro **backend Rust**. O PTY já é pipe de bytes no
backend (`infra/pty.rs`); nada disso depende de foco GUI. As regexes do
`pureTurn.ts` portam 1:1 pra Rust (mesmos padrões). O frontend continua só
renderizando o xterm; o backend vira a fonte de verdade de "needs input" /
"turn done" e emite eventos Tauri. Auto-pilot **exige** isso de qualquer forma:
ele precisa rodar com a janela sem foco / minimizada.

> Portar `pureTurn.ts` → Rust também desduplica: hoje a mesma lógica vive em
> `PureClaudePanel`, `App` e `Sidebar` (watchers de background). Uma fonte só.

## 3. Modelo goal-driven (a missão)

Auto-pilot não é só "responde quando perguntam". É um loop dirigido por missão:

```
missão (spec/changelog do painel)  ──┐
transcript (projeção / scrollback)  ─┤
evento detectado (prompt|turn-end) ─┴─►  Supervisor.decide()  ─►  ação no PTY
```

- **Missão** = texto livre que o usuário cola no painel flutuante. Ex.: o spec
  do que está sendo construído, ou um changelog "falta fazer X, Y, Z".
- Em cada **prompt** (Claude pede aprovação/opinião) → Supervisor responde
  alinhado à missão (aprova tool segura, escolhe abordagem, etc).
- Em cada **turn-end** (Claude terminou e ficou ocioso) → Supervisor decide:
  missão completa? Se não, manda a **próxima instrução** pra avançar. Se sim,
  para e notifica. Se incerto/perigoso → `Escalate` (chama o humano).

É isso que torna o auto-pilot capaz de tocar uma sessão Pure de ponta a ponta a
partir de um spec, não só desbloquear prompts pontuais.

## 4. Arquitetura

```
   PTY bytes (backend) ──► PureInputDetector (regexes portadas) ──┐
   (Pure, PRIMÁRIO)                                               │ Permission | TurnEnded
                                                                  ▼
   provider events ─────► (já existe) ToolApprovalRequested ──► AutopilotController
   (Structured, 2º)        / TurnCompleted                       │  + missão + guardrails
                                                                 ▼
                                                       ┌──────────────────┐
                                                       │ trait Supervisor │
                                                       └────────┬─────────┘
                                              ┌─────────────────┴─────────────────┐
                                              ▼                                   ▼
                                  MultiModelSupervisor              ClaudeProgrammaticSupervisor
                                  (lib `genai`)                     (`claude -p` headless)
                                              │
                                              ▼ Decision
                       Pure:       escreve no stdin do PTY (texto + `\r` separado)
                       Structured: approve_tool_use / reject_tool_use / send_user_message
```

Tudo no backend. Frontend = botão, painel da missão, kill switch, stream de
decisões.

## 5. Componentes

### 5.1 `crates/oxyris-supervisor`

```rust
pub struct Mission { pub text: String }            // spec/changelog do painel

pub enum PendingKind {
    /// Pure: menu de aprovação na tela.  Structured: ToolApprovalRequested.
    Permission { request_id: Option<String>, tool_name: Option<String>, raw_prompt: String },
    /// Claude terminou o turno e ficou ocioso — pode precisar da próxima instrução.
    TurnEnded { last_output: String },
}

pub struct AutopilotContext {
    pub mission: Mission,
    pub transcript: TranscriptView,   // Structured: projeção; Pure: scrollback do PTY
    pub cwd: String,
    pub environment: Environment,
}

pub enum Decision {
    Approve,                          // aprova tool / "sim, pode"
    Reject { reason: String },        // nega tool, reason volta pro modelo
    Reply { text: String },           // responde pergunta / manda próxima instrução
    Done { summary: String },         // missão completa → para e notifica
    Escalate { why: String },         // incerto/perigoso → chama o humano
}

#[async_trait]
pub trait Supervisor: Send + Sync {
    fn id(&self) -> &'static str;
    async fn decide(&self, ctx: &AutopilotContext, ask: &PendingKind)
        -> Result<Decision, SupervisorError>;
}
```

`Done` e `Escalate` são first-class: o supervisor pode encerrar ou pedir socorro
em vez de chutar pra sempre.

### 5.2 Detecção (backend)

- **Pure (primário):** `PureInputDetector` em `infra/pty.rs` (ou módulo
  vizinho). Porta `stripAnsi` + as regexes de `pureTurn.ts` pra Rust, mantém um
  tail rolante de ~2000 chars e o idle-timer **no backend** (tokio timer, imune
  a throttle). Emite `PureSignal::{NeedsInput, TurnEnded}` que o
  `AutopilotController` consome. O frontend continua a receber os mesmos sinais
  pro bull vermelho — só que agora vindos do backend, então funciona sem foco.
- **Structured (secundário):** zero detecção nova. Consome
  `ProviderEvent::ToolApprovalRequested` (→ `Permission`) e `TurnCompleted`
  (→ `TurnEnded`), que o `SessionSupervisor` já emite.

### 5.3 Resposta (atuação)

- **Pure:** escreve no stdin do PTY. Reusar o padrão `sendToPty` já existente —
  texto e depois `\r` numa escrita **separada** (a TUI do claude tem detecção de
  paste-burst; `texto\r` junto vira newline literal, não submit). Pra menus de
  aprovação, mandar a seleção (`1`+`\r` ou navegação). Lógica vai pro backend
  (terminal_write).
- **Structured:** `approve_tool_use` / `reject_tool_use` / `send_user_message`
  no `SessionSupervisor` (já existem).

### 5.4 Impls de Supervisor

- **`MultiModelSupervisor`** — lib `genai` (Anthropic/OpenAI/Gemini/Ollama/Groq,
  Rust-native, sem Effect-TS). One-shot: prompt = missão + contexto + sinal →
  `Decision` via structured output. Barato, baixa latência.
- **`ClaudeProgrammaticSupervisor`** — `claude -p --output-format stream-json`
  como supervisor; mais esperto (pode ler repo), + caro/lento. Reusa
  `oxyris-claude`.

### 5.5 `AutopilotController`

Por sessão: guarda missão + config + estado do loop. Recebe sinal de detecção,
aplica guardrails (§6), monta `AutopilotContext` (Pure: scrollback do PTY;
Structured: projeção), chama `Supervisor::decide`, traduz `Decision` em ação
(§5.3). Toda decisão vira evento no log → auditável.

## 6. Guardrails — full autonomy exige isto

Auto-aprovar e auto-instruir significa que o Supervisor pode liberar comandos
destrutivos ou entrar em loop. Antes de habilitar full autonomy, obrigatório:

- **Denylist hard.** Operações irreversíveis/perigosas — `git push --force`,
  deleção em massa, escrita fora do worktree, `rm -rf`, exfiltração de
  segredos — **nunca** auto-aprovam; sempre escalam. Avaliada **antes** de
  chamar o Supervisor.
- **Loop / oscillation detection.** Cortar quando Supervisor e Claude ping-pongam
  sem progresso (mesma instrução N vezes, X turnos sem mudança de estado). Ao
  cortar: pausa e notifica.
- **Budget cap.** Teto por sessão: turnos / tokens / tempo de parede. Estourou →
  pausa, devolve controle.
- **Kill switch.** Botão sempre-visível pra desligar e retomar manual na hora.
- **Audit trail.** Toda decisão (o quê, por quê, modelo) persistida como evento.

Default seguro: auto-pilot **desligado**; sem missão não roda. Full autonomy de
permissão exige opt-in explícito.

## 7. UI — botão + painel flutuante da missão

Âncora concreta: o header do `PureSessionView` em
`apps/web/src/components/PureClaudePanel.tsx` (~linha 796). Hoje tem o toggle
`SquareTerminal` em `ml-auto` (o ícone top-right da screenshot). **O botão de
auto-pilot fica ao lado dele**, no mesmo cluster top-right.

- **Botão auto-pilot** (ícone, ex.: `Bot`/`Sparkles` do lucide): estado
  on/off/escalated por cor (cinza off / accent ligado / âmbar escalado).
- **Click → painel flutuante** (popover ancorado no botão, não modal full):
  - Textarea grande pra **missão** (spec / changelog). Persistida por sessão.
  - Seletor de Supervisor (impl + modelo).
  - Denylist / budget cap (ou link pra Settings).
  - Toggle "Ativar auto-pilot" + **kill switch** proeminente quando rodando.
  - Mini-log das últimas decisões do auto-pilot (🤖 aprovou X / respondeu Y /
    escalou Z).
- Stream de decisões também pode aparecer inline no fluxo do PTY como linhas
  marcadas 🤖.
- Embedded (Multi View) e Structured (`ChatPanel`) ganham o mesmo botão depois.
- Todas as strings via `useTranslation()` (regra i18n do CLAUDE.md).

## 8. Sprint proposto (PLAN.md §7)

**Sprint 14 — Auto-pilot / Supervisor LLM (~7-8 dias)**

- [ ] `crates/oxyris-supervisor`: `trait Supervisor`, `Mission`,
      `AutopilotContext`, `PendingKind`, `Decision`.
- [ ] Portar `pureTurn.ts` (stripAnsi + regexes + sniffer + idle) pra Rust no
      backend; emitir sinais e alimentar o bull do frontend daí (fix do foco).
- [ ] `PureInputDetector` + atuação no stdin do PTY (texto + `\r` separado,
      seleção de menu).
- [ ] `MultiModelSupervisor` (via `genai`, structured output → `Decision`).
- [ ] `AutopilotController` + guardrails (denylist, loop-detect, budget, audit
      events, kill switch). Aggregate/eventos pra config + missão + decisões.
- [ ] Structured wiring (secundário): consumir `ToolApprovalRequested` +
      `TurnCompleted`.
- [ ] `ClaudeProgrammaticSupervisor` (headless) como 2ª impl.
- [ ] UI: botão no header do `PureSessionView` + painel flutuante da missão,
      seletor de supervisor, denylist/budget, kill switch, mini-log. i18n.
- [ ] **Validação:** com a janela **sem foco / minimizada**, uma sessão Pure
      recebe missão (spec) e roda multi-step sozinha — detecção não quebra,
      permissões dentro da allowlist auto-aprovam, denylist escala, budget cap
      pausa, kill switch retoma na hora.

## 9. Decisões em aberto

- Onde mora a detecção no backend: dentro de `infra/pty.rs` vs módulo novo
  `infra/pure_signals.rs`.
- Classificador de "pergunta aberta vs turn-end neutro": regex pura
  (`PURE_PROMPT_RE` já distingue) vs LLM leve. Começar com regex.
- Persistência: aggregate novo (`Autopilot`) vs campos na `Session` +
  `settings.json`. Missão provavelmente no aggregate (event-sourced, replay).
- Denylist default concreta (lista inicial).
- Pure-mode atuação em WSL: stdin do PTY já roteia via ConPTY/`wsl.exe` — validar
  que a escrita de seleção de menu funciona igual Windows.
```
