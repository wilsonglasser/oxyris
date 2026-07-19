# Design — Um language server por projeto (multi-root), não por worktree

> Status: **slices 1–2 implementados** (2026-07-19). Slices 4–5 pendentes.
> Data: 2026-07-19.
> Encaixa como sprint próprio no [`PLAN.md`](../../PLAN.md) §7. Source of truth continua o PLAN.md.
> Pré-requisitos já entregues:
> - reaping determinístico + idle-reaper (commit `4ec9428`).
> - config lean do rust-analyzer (`cachePriming`/`lru`/`numThreads` + `check` com `--target-dir` isolado) + `IDLE_TTL` 5 min.
> Relaciona com [`wsl-agent-process-lifecycle.md`](./wsl-agent-process-lifecycle.md) (lifecycle nativo dentro do distro) — ortogonal: aquele muda **quem é o pai** do processo; este muda **quantos processos existem**.

## 1. Problema

Hoje o pool de LSP é chaveado por **worktree**. Cada worktree de um projeto sobe
o seu próprio `rust-analyzer`. Como worktrees no Oxyris são **git worktrees do
mesmo repo** (branches), N worktrees abertos do mesmo projeto = N rust-analyzers,
cada um indexando **as mesmas dependências** de forma independente.

Onde a RAM do rust-analyzer mora, aproximadamente:

| Parte | ~% do heap | Idêntica entre worktrees do mesmo repo? |
|---|---|---|
| Análise de **dependências** (serde, tokio, …) | ~50–60% | **Sim**, quando o `Cargo.lock` casa |
| sysroot / std | ~10% | Sim |
| Crates do **workspace** (código do usuário) | resto | Não |

Ou seja: a maior fatia do heap é **duplicada N vezes** sem necessidade. Abrir 4
branches do mesmo repo custa ~4× a análise de deps, quando 1× bastaria.

O idle-reaper e a config lean (pré-requisitos) **limitam o pico**, mas não atacam
a duplicação estrutural: enquanto os worktrees estão ativos, os N servers coexistem.

## 2. Arquitetura atual

- **Pool real:** `apps/desktop/src/infra/lsp.rs` → `LspManager`, campo
  `entries: HashMap<AggregateId /* worktree_id */, WorktreeEntry>`, onde
  `WorktreeEntry { workspace: PathBuf, detected: Vec<LspLanguage>, clients: HashMap<LspLanguage, Arc<LspClient>> }`.
  Um `LspClient` por `(worktree, linguagem)`.
- **`LspClient`** (`crates/oxyris-lsp/src/lib.rs`): tem **um único** `root: PathBuf`
  e manda no `initialize` **um único** `workspace_folder` (linha ~219). Não fala
  `linkedProjects` nem `workspace/didChangeWorkspaceFolders`.
- **MCP:** `LspBackend::Bridge` (`apps/mcp-server/src/lsp_backend.rs`) proxia toda
  chamada para o `LspManager` do desktop por TCP — já dedup **por sessão**, mas
  não por worktree. O `Local` é fallback (spawn próprio).
- **Lifecycle:** `worktree.rs:185` → `lsp.warm_primary(worktree_id, env, path)` no
  create; `worktree.rs:225` → `lsp.close(worktree_id)` no remove. O caller tem
  `project` em mãos (`data.project_id`, `project.environment`, `project.root_path`).
- **Roteamento de query:** o bridge chama `ensure_at(workspace_root, env, lang)`,
  que encontra a entry por `entry.workspace == workspace_root`. Query traz path
  absoluto do arquivo.

## 3. Proposta

Trocar a chave do pool de **worktree** para **`(project_id, environment, linguagem)`**.
Um `LspClient` por projeto+distro+linguagem, servindo **todos os worktrees**
daquele projeto como *workspace folders* / *linked projects* simultâneos.

rust-analyzer tem um **crate graph global**: dois workspaces que dependem de
`serde 1.0.x` da mesma origem (`~/.cargo/registry/...`, path compartilhado) são
vistos como a **mesma crate** e analisados **uma vez**. Portanto, servir N
worktrees num único processo dedup as deps automaticamente — só o código de cada
workspace fica N×.

### Ganho esperado

Para o caso comum (várias branches, `Cargo.lock` alinhado): a maioria do heap
(deps) passa a ser analisada 1×. 3–4 worktrees no custo de ~1,3× em vez de ~4×.
Quando os lockfiles divergem, o ganho degrada para "1 processo em vez de N"
(overhead de processo + proc-macro server + sysroot compartilhados), ainda positivo.

## 4. Mudança de modelo de dados

```
// antes
entries: HashMap<worktree_id, WorktreeEntry { workspace, detected, clients }>

// depois
servers: HashMap<(project_id, Environment, LspLanguage), Arc<LspClient>>
folders: HashMap<project_id, BTreeSet<PathBuf>>   // roots de worktree ativos por projeto
routes:  HashMap<PathBuf /* worktree root */, project_id>  // p/ roteamento por path
```

`LspClient` deixa de ter um único `root`. Ganha:

- construção com **conjunto** de roots (workspace folders + `linkedProjects`);
- `add_folder(root)` → `workspace/didChangeWorkspaceFolders` (added) +
  `workspace/didChangeConfiguration` com `linkedProjects` atualizado;
- `remove_folder(root)` → o inverso;
- `roots()` para o reaper decidir quando o server fica vazio.

## 5. linkedProjects vs workspaceFolders

Para Cargo, a via autoritativa é **`rust-analyzer.linkedProjects`** — lista de
paths de `Cargo.toml` (ou de `rust-project.json`). `workspace_folders` sozinho faz
o RA auto-descobrir Cargo.toml sob cada folder, mas o controle explícito via
`linkedProjects` é mais previsível para add/remove dinâmico.

Decisão: mandar **ambos** —
- `workspace_folders` no `initialize` e mutados via `didChangeWorkspaceFolders`
  (contrato LSP genérico, serve ts/php também);
- `linkedProjects` (só rust) no `initializationOptions` e re-emitido via
  `didChangeConfiguration` quando o conjunto muda.

Para **ts/php**: multi-root sai de graça por `workspace_folders`
(typescript-language-server e intelephense suportam). Sem `linkedProjects`.

## 6. Roteamento de query

O bridge continua mandando path absoluto do arquivo + o workspace da sessão.
O manager resolve:

1. `routes[worktree_root] → project_id` (worktree_root = o workspace da sessão,
   já confinado no MCP — ver §8);
2. `servers[(project_id, env, lang)]` → o client único;
3. o client já tem o folder daquele worktree registrado, então RA resolve o
   arquivo pelo path. Nenhuma mudança no protocolo de query (hover/references/
   diagnostics carregam URI absoluto).

Se o folder do worktree ainda não estiver registrado no client (primeiro query),
`ensure` faz `add_folder` antes de rotear.

## 7. Reaper — **implementado como server-level** (revisão da decisão 4)

Decisão original (§12.4): clock por folder no manager, remover folder ocioso do
server compartilhado. **Revisto na implementação para reaping server-level**:

- **Server ocioso:** um server compartilhado é reapado só quando **nenhum** dos
  seus worktrees teve query há `IDLE_TTL` (o `idle_for()` por-client já cobre —
  qualquer worktree ativo mantém o server quente). No reap, o server cai inteiro
  e o mapa de folders daquele lang é limpo; o próximo query respawna e
  re-anexa os folders lazy.
- **Server vazio no close:** quando o último folder sai em `close(worktree_id)`
  (worktree removido), o server sem folders é desligado na hora.

Por que não per-folder-idle: puxar um folder ocioso de um multi-root **vivo**
dispara reindex do conjunto no RA (churn) para devolver só a fatia de workspace
daquele worktree — as deps, que são a maioria do heap, continuam compartilhadas e
não voltam. Com uso típico de 3–4 worktrees, manter todos anexados num server é
muito melhor que N servers; a otimização per-folder é de segunda ordem e fica de
fora. `remove_folder` continua sendo chamado no **close explícito** de worktree
(não por idle), então não há churn especulativo.

Resultado: `LspClient::idle_for()` por-client basta; o manager **não** precisa do
mapa `root → Instant`. Se um dia o per-folder-idle valer a pena, o `add_folder`/
`remove_folder`/`roots()` do slice 1 já suportam — é só somar o clock por-folder.

## 8. Segurança (confinamento)

Não muda. O confinamento de path vive na **camada MCP** (`LspBackend::resolve_path`),
não no client: cada sessão MCP roda `bypassPermissions` confinada ao **seu**
worktree (workspace da sessão). O fato de um mesmo processo RA servir vários
worktrees é invisível para o confinamento — o bridge só entrega paths já validados
contra o workspace da sessão que chamou. Um RA multi-root não abre buraco: ele
sempre teve acesso de leitura ao filesystem de qualquer jeito; o gate é o
`resolve_path` antes da query.

## 9. WSL

Um RA por `(project, distro, lang)` **dentro do distro**. O spawn continua via
`wsl.exe -d <distro> -- bash -lc 'exec rust-analyzer …'` (ou, quando
[`wsl-agent-process-lifecycle.md`](./wsl-agent-process-lifecycle.md) entrar, via
agent). Folders são **paths POSIX** do distro — o mesmo `to_posix` de hoje. Menos
processos no distro = menos balão; combina diretamente com o objetivo daquele doc.

## 10. Riscos e mitigações

| Risco | Mitigação |
|---|---|
| **Blast radius** — RA crasha → cai LSP de todos os worktrees do projeto | Respawn lazy já existe (`ServerGone` → re-ensure). Custo: um cold-start compartilhado, não N. |
| **Reload churn** — abrir/fechar worktree reindexação do conjunto | Debounce no reaper (janela de graça); `add_folder` só no primeiro query real, não no warm especulativo. |
| **Lockfiles divergentes** entre branches → deps não dedup | Aceito. Degrada para "1 processo em vez de N", ainda melhor. |
| **check on-save por workspace** — N worktrees ainda rodam N `cargo check` | `--target-dir` isolado (já entregue) evita thrash; `check.invocationStrategy: "once_per_workspace"` é o default. Sem regressão vs hoje. |
| **Roteamento errado** — path cai no folder errado | RA resolve por path absoluto; folders são disjuntos (roots de worktree não se aninham). |

## 11. Slices de rollout

1. ✅ **`LspClient` multi-folder.** `roots: Mutex<Vec<PathBuf>>`; `add_folder`/
   `remove_folder`/`roots()`; `workspace_folders` + capability no init;
   `didChangeWorkspaceFolders` + `didChangeConfiguration`; helpers
   `workspace_folder()` e `rust_linked_projects_settings()`. Testes unitários dos
   helpers puros. **Feito.**
2. ✅ **`LspManager` re-chaveado** por `(project_id, lang)`. Struct `Pools`
   (`worktrees`/`by_path`/`projects`/`dedicated`); `register`/`ensure`/
   `warm_primary` recebem `project_id`; `ensure_at` resolve project via `by_path`
   (sintético + auto-register para path desconhecido); `close` desanexa folder e
   desliga server vazio; `FOLDER_CAP = 8` com fallback dedicado; `init_options_for`
   injeta `linkedProjects` no spawn. Reaper server-level (§7). `WorktreeContext`
   ganhou `project_id`; callers (`worktree.rs`, `indexing.rs`) atualizados.
   Gate: fmt + clippy `-D warnings` + 92 testes desktop + 6 lsp. **Feito.**
   ⚠️ **Não runtime-verificado** — sem app+worktrees reais nesta sessão. Validar
   com app aberto: abrir 2+ worktrees do mesmo repo Rust, confirmar **1** só
   rust-analyzer servindo os dois (via `ps`/`Get-Process` + logs
   `lsp worktree joined shared server`).
3. ⤳ **Reaper granular** — dobrado no slice 2 como **server-level** (§7). O
   per-folder-idle não foi implementado (segunda ordem); estrutura do slice 1
   suporta se reabrir.
4. ☐ **Bridge/MCP:** sem mudança de superfície (roteamento por path é o contrato;
   `ensure_at` já mapeia worktree_root → project). Falta **validar ao vivo** que
   uma query do bridge cai no server compartilhado certo.
5. ☐ **ts/php multi-root** via `workspace_folders` (opcional, depois do rust
   provar em produção).

## 12. Decisões (fechadas 2026-07-19)

- **Grão = projeto (= repo). Decidido.** Não existe caso de dois projetos
  apontando pro mesmo repo físico. Chave `(project_id, env, lang)` é segura; sem
  necessidade de desambiguar por path de repo.
- **Teto de folders: ~8 (folga).** Uso típico é 3–4 worktrees por projeto. O cap
  fica em **8** como folga — acima disso, cai para o comportamento antigo (server
  próprio pro worktree excedente) em vez de inchar o reindex de um único RA.
  Medir o custo real de reindex com 8 folders quando o slice 1 existir; ajustar a
  constante se preciso.
- **Toolchain divergente: escape hatch, prioridade baixa.** Não há dado de com que
  frequência branches fixam `rust-toolchain.toml` diferente. Um único RA assume um
  toolchain, então: no `add_folder`, se o `rust-toolchain.toml` do worktree
  divergir do toolchain do server, **não** adicionar ao server compartilhado — subir
  server próprio pra esse worktree (mesma via de fallback do teto de 8). Barato de
  guardar, raro de disparar; implementar junto com o slice 2 mas sem otimizar.
- **`idle_for()` por folder: clock no manager.** O `LspClient` fica burro sobre
  política de idle — o manager mantém `HashMap<PathBuf /* root */, Instant>` e
  decide reap. Mantém a separação atual (manager é dono do reaper); o client só
  expõe `add_folder`/`remove_folder`/`roots()`.

## 13. Consequência das decisões no rollout

O teto de 8 e o escape hatch de toolchain compartilham **a mesma via de fallback**:
"esse worktree não entra no server compartilhado → sobe/reusa um server dedicado".
Vale modelar isso como um único caminho no slice 2 (`ensure` decide compartilhado
vs dedicado) em vez de dois ramos separados. O map `root → Instant` do idle vive no
manager desde o slice 1 — o client nunca ganha noção de tempo.

## 14. Alternativa considerada (e por que não sozinha)

**LRU cap no nº de RA vivos** (ex.: 3, reap o menos-usado no spawn). Muito mais
barato (mudança contida no manager, sem tocar o `LspClient`), trava o **pico**.
Mas **não dedup deps**: se os 3 RA vivos são branches do mesmo repo, ainda é 3× a
análise de deps. Bom como rede de segurança de curto prazo, não como fix
estrutural. Pode entrar **antes** deste sprint sem conflito — o multi-root depois
torna o cap quase irrelevante (1 server por projeto já é o teto natural).
