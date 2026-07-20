# Design — Oxy (assistente de voz cross-thread)

> Status: proposta. Código zero.
> Data: 2026-07-07.
> Encaixa como sprint novo no [`PLAN.md`](../../PLAN.md) §7. Source of truth continua o PLAN.md.

## 1. Problema / visão

Um assistente geral — **Oxy** — que enxerga e dirige **qualquer thread aberta**
(todas as sessões/worktrees), ativado por **texto** ou **voz**. Wake word "Oxy"
abre um canal de áudio; o usuário fala, o Oxy age nas threads e responde por voz.
Longo prazo: ferramentas MCP extras (busca, navegador, etc.) plugam no Oxy sem
cirurgia.

Decisões fechadas (Q&A 2026-07-07):

- **Oxy escreve** (drive completo): injeta input, cria/interrompe sessões — não
  é só observador.
- **Alcance global**: enxerga e roteia entre **todas** as threads abertas.
- **Wake word dedicado** ("Oxy"), sempre-ligado no backend.
- **Voz é efêmera** — fora do event log. Turns do Oxy = eventos normais
  (agregados `Session`/`Turn`).
- Todas as peças de voz **plugáveis atrás de traits** → troca sem cirurgia.
- Começo **leve/local**: Windows-nativo onde der, modelos locais grátis.

## 2. Oxy = meta-agente supervisor

Oxy **não** é uma sessão comum. É uma sessão privilegiada rodando o provider
(Claude), com **MCP tools cross-session** que operam no event store + supervisor.

- Novo `SessionKind::Assistant` em `apps/desktop/src/domain/session.rs:72`
  (hoje só `Structured`/`Pure`). O aggregate não muda de forma — só sinaliza que
  esta sessão tem o toolset privilegiado montado.
- Tools novas, moldadas no template `autopilot_bridge` já existente
  (`apps/desktop/src/infra/autopilot_bridge.rs` + defs em
  `apps/mcp-server/src/main.rs:437`):

  | Tool | Ação |
  |---|---|
  | `oxyris_threads_list` | enumera sessões/worktrees abertas (via `projections::list_running_sessions`, projections.rs:522) |
  | `oxyris_thread_read` | lê estado/scrollback/turns de uma thread |
  | `oxyris_thread_send` | injeta input numa thread (Structured: `StartTurn`; Pure: escreve no PTY — reusa driver do `AutopilotManager`, autopilot.rs:162) |
  | `oxyris_thread_create` | cria sessão nova (projeto/worktree/kind) |
  | `oxyris_thread_interrupt` | interrompe turn/PTY corrente |

- O bridge cross-thread generaliza o `autopilot_bridge` (que hoje tem `session_id`
  baked). A versão do Oxy recebe `thread_id` como **argumento** — daí o alcance
  global.

### Guardrails (Oxy escreve)

Reusa `crates/oxyris-supervisor/src/guardrails.rs` (`Denylist`, `LoopGuard`,
`Budget`). Ações destrutivas (delete sessão, interromper) pedem confirmação
(por voz ou UI) antes de executar — nunca silencioso.

## 3. Camada de voz (módulo Rust, fora do hot-path event store)

```
mic (cpal) ─► frames PCM ─► Rustpotter (wake "Oxy")
                                   │ detectou
                                   ▼
                          STT (trait SttEngine) ─► texto ─► buffer
                                   │ endpointing (§5)
                                   ▼ commit
                          Oxy turn (Session/Turn) ─► resposta texto
                                   │
                                   ▼
                          TTS (trait TtsEngine) ─► áudio ─► speaker
```

Máquina de estados:

```
Idle ──wake "Oxy"──► Listening ──commit──► Thinking ──► Speaking ──► Listening (loop)
  ▲                                                          │
  └──────────────── silêncio longo / dispensa ◄─────────────┘
Barge-in: "Oxy" durante Speaking = corta TTS, volta pra Listening.
```

- **Áudio nunca vira evento.** Estado wake/listen = transiente. Só os
  turns do Oxy (texto de entrada já transcrito + resposta) entram no event log,
  como qualquer sessão.

## 4. Stack de voz — **sherpa-onnx** (revisado 2026-07-07)

Uma lib só (`sherpa-onnx`, k2-fsa, crate oficial `sherpa-onnx = "1.13"`) cobre
**wake + STT + TTS**, offline, on-device. `cpal` só alimenta o mic. Substitui o
plano antigo de 4 libs (rustpotter + WinRT + whisper.cpp + Kokoro separado).

| Camada | Impl | Nota |
|---|---|---|
| Mic capture | `cpal` | única dep extra além do sherpa |
| Wake "Oxy" | **sherpa KWS** (keyword spotting open-vocabulary) | "Oxy" como **texto/tokens, sem treinar modelo**. Adeus wizard de amostras. |
| STT | **sherpa recognizer** (zipformer/whisper offline) | on-device; modelo ONNX baixado 1x |
| TTS | **sherpa TTS — Kokoro `pf_dora`** (pt-BR fem) | mesma lib; ElevenLabs pool = opt-in (§4.1) |

Por que sherpa e não rustpotter (plano original):
- rustpotter **abandonado**: só v3.0.2 não-yanked, e ela depende hard de
  `candle-core 0.2.2` que **não compila** aqui (conflito `half`/`rand`).
- sherpa-onnx é **mantido** (2026), KWS open-vocab (custom sem treino), e o
  crate `-sys` **baixa binários prebuilt** no Windows (build ~60s, sem cmake do
  source). Validado: compila e linka.
- Bônus: unifica STT + TTS → menos superfície, um runtime.

Custo: modelos ONNX (KWS + STT + TTS/Kokoro) baixados 1x pro `data_dir` (uns MBs
cada). Binário do app cresce (onnxruntime embutido). Tudo offline depois.

Traits `SttEngine`/`TtsEngine` continuam como ponto de extensão (ex.: ElevenLabs
como `TtsEngine` extra), mas o default de todas as camadas agora é sherpa.

### 4.1 TtsEngine — cadeia + ElevenLabs com rotação de keys

Cadeia de fallback: **ElevenLabs (pool rotativo) → Kokoro pf_dora → Windows WinRT**.

`ElevenLabsTts` (engine adicional, opt-in):

- **Pool de múltiplas API keys** — estica o free tier (10k credits ≈ ~10 min
  áudio por key/mês).
- Keys guardadas **seguras** (Windows DPAPI / Credential Manager) — nunca
  plaintext em config.
- Seleção: pré-checa saldo via `GET /v1/user/subscription`
  (`character_count`/`character_limit`) e escolhe key com budget. Sem saldo →
  próxima key.
- Rotação em runtime: resposta `401`/`429` de quota → marca key
  `exhausted_until` (reset mensal do free tier) → próxima key.
- Todas as keys secas → fallback automático pro Kokoro.
- Modelo Flash v2.5 (latência ~75ms) pra respostas curtas do assistente.

> Trade-off aceito: ElevenLabs é cloud + manda o texto da resposta pro servidor
> deles. Por isso é opt-in e não o default. Kokoro cobre o uso ilimitado local.

## 5. Endpointing (quando o turno do usuário acaba)

O usuário fala pausado. Buffer acumula os partials/finais do STT numa mensagem só.

**Commit (manda pro Oxy) quando:**
1. **Silêncio ≥ `silence_commit_ms`** (VAD detecta parada) e o fim do buffer
   **não** é filler → auto-aceita.
2. Usuário diz **`"câmbio"`** → commit imediato, sem esperar. A palavra é
   removida do texto. `"câmbio"` é **sleep word do usuário** — o Oxy **nunca**
   fala isso; responde normal.

**Guarda de hesitação** (o "ehhh"/"hmmm"):
- Se o fim do buffer é filler pt-BR → **não** commita no timeout; estende a espera
  (`filler_grace_ms`). Usuário ainda está pensando.
- Sai da hesitação quando vem fala real depois, ou no `"câmbio"`.

```
Wake "Oxy" ─► Listening
   ├─ fala        → append buffer, reset timer de silêncio
   ├─ silêncio Ns + fim ∉ filler → COMMIT
   ├─ silêncio  + fim ∈ filler   → espera (grace estendido)
   ├─ "câmbio"                    → COMMIT já
   └─ COMMIT → Thinking → Oxy responde (TTS) → volta Listening
```

VAD = detecção acústica de silêncio (energia do mic via `cpal`).
Filler-guard = léxico (última palavra do buffer). Combina os dois.

Knobs (config, ajustáveis):

| Knob | Default |
|---|---|
| `silence_commit_ms` | 5000 |
| `filler_grace_ms` (espera extra pós-filler) | +5000 |
| `submit_keyword` | `"câmbio"` |
| `hard_cap_ms` (teto de segurança) | 60000 |
| `filler_set` | `é, éé, ééé, eh, ehh, hmm, hum, humm, hã, ããã, uhm, tipo, então, deixa eu ver, …` |

Reusa `stripVoiceSubmitCommand` (front já tem a convenção voz `"câmbio"`=submit,
`apps/web/src/hooks/useSpeechRecognition.ts`).

## 6. Frontend

Oxy é um **dock global à direita** (`OxyDock`), sempre disponível em qualquer
tab, **não** uma sessão na lista/área principal. Decisões (2026-07-07):

- **Uma** sessão Oxy pro app inteiro (`SessionKind::Assistant`), id persistido em
  `localStorage` (`oxyStore`), cwd = projeto ativo na criação. Reaproveitada
  entre reloads.
- Renderizada **sempre** via `ChatPanel` com prop `sessionId` (structured,
  **nunca** o PTY Pure) — reply text limpo pro TTS.
- Botão "Oxy" no titlebar = **toggle** do dock (mostra/esconde), não cria sessão
  no centro.
- Cuidado: o render central escolhe Pure/Structured pela flag global `pureMode`
  (`App.tsx`), ignorando `session.kind` — por isso Oxy vive no dock via
  `ChatPanel` direto, imune a esse toggle.

Painel Oxy (com voz, F2+):
- indicador de estado (Idle/Listening/Thinking/Speaking) + nível de mic
- transcript ao vivo (o que o Oxy ouviu)
- input de texto (ativação alternativa, sem voz) — já funciona na F1
- toggle mic on/off; config de engines (STT/TTS) + keys ElevenLabs

Eventos backend→front seguem a convenção dinâmica `oxy:<kind>` (espelha
`session:<id>:<kind>`, session_supervisor.rs:425).

### 6.1 Settings — seção Oxy / Voz (F2)

O wake word é **personalizado pela voz do usuário** (não grammar genérica).
Fluxo na tela de Settings:

1. **Gravar amostras** — wizard: usuário fala "Oxy" 3-8x, `cpal` captura os wavs.
2. App gera o **modelo Rustpotter** (`.rpw`) das amostras, salva no `data_dir`.
3. O detector de wake carrega o `.rpw` no boot; regravável a qualquer momento.

Controles da seção:

| Controle | O quê |
|---|---|
| Gravar/regravar wake | wizard de amostras → gera `.rpw` |
| Sensibilidade | slider do threshold Rustpotter (baixa = menos falso-positivo) |
| Mic | device de entrada (enumerado via `cpal`) |
| Wake on/off | liga/desliga a escuta sempre-ligada |
| STT engine | `WindowsStt` / `WhisperStt` |
| TTS engine | `WindowsTts` / `KokoroTts` / `ElevenLabsTts` |
| Keys ElevenLabs | pool: add/remove, guardadas em DPAPI |
| Endpointing | `silence_commit_ms`, `filler_grace_ms`, `filler_set`, `submit_keyword` |

Config persistida via o mesmo mecanismo dos defaults do autopilot
(`tauri_commands/autopilot.rs` como referência).

## 7. Plano faseado

- **F1 — Oxy core (ativação por TEXTO).** `SessionKind::Assistant` + tools
  cross-session (generaliza `autopilot_bridge` + advertise em `mcp-server`) +
  painel Oxy com input texto. Prova "dirige qualquer thread" sem voz.
- **F2 — Voz entra.** `cpal` mic → Rustpotter wake "Oxy" → `SttEngine`
  (`WindowsStt`) → endpointing (§5) → input do Oxy. Máquina de estados + overlay.
- **F3 — Voz sai.** `TtsEngine`: `WindowsTts` → `KokoroTts` → `ElevenLabsTts`
  (pool rotativo). Barge-in cancela TTS.
- **F4 — MCP extra.** search + browser plugam como mais tools do Oxy (browser já
  tem plano CDP/WebView2).

## 8. Non-goals / abertos

- Não persistir áudio nem transcrição bruta como eventos (só o texto do turn).
- `whisper.cpp` e ElevenLabs são upgrades opt-in, não bloqueiam F1–F3.
- Aberto: onde mora a config default de engines/keys (provável reuso do
  mecanismo de `autopilot` defaults, tauri_commands/autopilot.rs).
