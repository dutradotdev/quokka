# Handoff — Fase 2, retomada do Passo 4 (split de workspace)

Date: 2026-06-02
Branch: `phase2-core-facade`
Spec: `docs/superpowers/specs/2026-06-01-phase2-core-facade-design.md`
Plano: `docs/superpowers/plans/2026-06-01-phase2-core-facade-plan.md`

Este doc é autocontido: leia-o (mais o spec e o plano) e dá para continuar o
Passo 4 do zero, sem o contexto da sessão anterior.

## Onde estamos

Fase 2 = preparar o core open-source que destrava a GUI Tauri (Fase D, repo
privado, fora deste repo). Escopo deste trabalho: **A** split de workspace, **B**
extração de lógica pura, **C** superfície `--json`. Entrega em **PR único** na
branch `phase2-core-facade`.

Commits na branch (todos verdes: `cargo fmt && clippy -D warnings && test`):

```
5cea72a docs: update Phase 2 plan — step 4a/4b done, 4c-4e remaining
c616ba1 refactor: move syslog parser into device::syslog (Phase 2, step 4b)
5f2c2b1 refactor: make device::connect presentation-free (Phase 2, step 4a)
77f4ce4 docs: record Phase 2 step 1-3 status and step-4 split blockers
8039e0f feat: route --json through the facade for all query commands (step 3)
fa6ecaa feat: add app facade and DTOs over the Device trait (step 2)
d30d2cc feat: make DeviceError and CardData serializable (step 1)
6f8e79a docs: add Phase 2 implementation plan
5ca75e6 docs: add Phase 2 design
```

## Concluído

- **Passo 1** — `DeviceError` serializa como `{kind,message}` (impl manual em
  `src/device/mod.rs`); família `CardData` deriva `Serialize` (camelCase).
- **Passo 2** — facade `quokka_cli::app` (`src/app/mod.rs`): uma fn por operação,
  devolve DTO serializável, erro `DeviceError`. DTOs: `AnalyzeReport`, `AutoMark`,
  `DeleteOutcome`, `RenderedCard`; reusa `MediaReport`. `app::redact`
  (`src/app/redact.rs`) é a redação pura compartilhada (`info.rs` depende dela).
  `status`/`media` `run()` já passam pelo facade.
- **Passo 3** — `--json` genérico no dispatch de `src/lib.rs` (chama o facade,
  serializa); `command_supports_json` lista status/info/apps/analyze/media/
  devices/logs; `logs --json` = NDJSON (campo `json` em `logs::Options`,
  emissão no `plain::run`). `print_json` + `silent_walk` helpers em `lib.rs`.
- **Passo 4a** — `device::connect` recebe `select: DeviceSelector<'_>`
  (`type DeviceSelector<'a> = &'a (dyn Fn(&[DeviceListing]) -> Result<Option<usize>> + Send + Sync)`).
  A CLI passa `&crate::ui::select_device` (dialoguer + `format_listing_row`, em
  `src/ui.rs`). `device/` não usa mais `dialoguer`. Callers atualizados:
  `lib.rs`, `menu.rs` (2), `sidebar.rs`, e os 3 testes e2e + o harness `bench`.
- **Passo 4b** — parser de syslog movido de `commands::logs::parser` para
  `src/device/syslog.rs` (`parse_syslog_line`, `is_continuation` + testes).
  `device/` **não referencia mais nenhum `crate::commands::*`** — confirmado com
  `grep -rn "crate::commands" src/device/` → vazio. Está autocontido.

## Decisões e desvios a respeitar (não regredir)

1. **`qk info --json` mudou de forma (quebra intencional):** agora é o DTO
   `DeviceInfo` plano em camelCase (o payload da GUI), não o objeto aninhado
   snake_case antigo. Snapshot atualizado em `tests/integration.rs`
   (`info_json_output_snapshot`).
2. **`capture --json` ficou fora** (TUI + modos hosts/dns/sni). É rejeitado com
   mensagem clara, igual `card --json`. Extensão futura, não regressão.
3. **`app::analyze` recebe `now_unix: i64`** (determinismo), diferente da
   assinatura "ideal" do spec — proposital.
4. **Tipos de projeção do card são `Serialize`-only** (`Badge` tem `&'static str`,
   não dá `Deserialize`). `RenderedCard` idem. Não tente derivar `Deserialize`
   neles.
5. **Picker → CLL via callback** (decisão do Lucas): o core nunca abre UI. Mantenha
   `DeviceSelector` injetado; não reintroduza `dialoguer` no `device/`.

## O que falta — Passo 4 (4c → 4d → 4e)

Objetivo: `quokka-core` **livre de apresentação** com `device` + `app` + a lógica
pura; `quokka-cli` depende dele e mantém bins/TUIs/renderers/`ui` terminal.

### Dependências cross-módulo que o `app` ainda puxa (o que força o 4c)

`src/app/mod.rs` importa:
- `crate::commands::card::data::{self, CardData}`, `crate::commands::card::{png, render}`
- `crate::commands::{analyze, media}` (usa `analyze::heuristics::detect_all`,
  `analyze::sort_by_size`; `media::build_report`)
- `pub use crate::commands::media::MediaReport`
- `crate::ui::now_unix`

`src/app/redact.rs` importa só `crate::device::DeviceInfo` (já core-friendly).

### 4c — extrair lógica pura para módulos do core (ainda no crate único)

Mover para um layout `core`-shaped (sugestão: `src/logic/` + `src/fmt.rs`, ou
mover `card/` puro para junto do `app`). Por módulo:

- **card/** (`src/commands/card/`): `data.rs`, `badges.rs`, `render.rs`,
  `png.rs`, `share.rs`, `emoji.rs` são **puros** → core. Só `mod.rs` (run que
  grava PNG + abre Preview + `prompt_for_star`) fica na CLI. `mod.rs` tem
  `default_output_path`, `write_png`, `print_success`, `open_in_preview`,
  `prompt_for_star` (presentation/IO) → CLI.
- **analyze** (`src/commands/analyze.rs`): puro → core: `pub mod heuristics`
  (com `Match`, `detect_all`, `live_photo_motion`, `originals_with_edited`,
  `old_screenshots`, `exact_duplicates`), `sort_by_size`, `ext_lower`,
  `kind_from_ext`. Fica na CLI: `run`, `pick_and_delete`, `walk`,
  `confirm_and_delete`, `render_file_list`, `build_confirm_prompt`, `mod tui`.
- **media** (`src/commands/media.rs`): puro → core: `Kind`, `YearMonth`,
  `MediaReport`, `DuplicateReport`, `DuplicateGroup`, `build_report`,
  `classify_by_kind`, `bucket_by_month`, `find_duplicate_groups`,
  `epoch_to_year_month`, `previous_month`, `roots_label`, `MonthBuckets`. Fica na
  CLI: `run`, `report` (usa `now_unix`), `render`, `bar_for`.
- **commands::top_n_by_size** (`src/commands/mod.rs`): puro → core.
- **ui.rs** (`src/ui.rs`): puro → core (`fmt`): `format_bytes`,
  `format_optional`, `format_percent`, `format_bar`, `civil_from_days`,
  `civil_from_unix`, `now_unix`. Fica na CLI: `stdin/stdout_is_interactive`,
  `non_interactive_forced`, `wait_for_enter`, `spinner`, `progress_bar`,
  `terminal_width`, `select_device`, `format_listing_row`, `DASH`.

Conforme mover, reaponte os imports do `app` para os novos caminhos do core.

### 4d — criar o workspace

```
quokka/
├── Cargo.toml            # [workspace] members = ["crates/quokka-core","crates/quokka-cli"]
├── crates/
│   ├── quokka-core/      # lib quokka_core: device, app, logic/fmt puros
│   └── quokka-cli/       # bins quokka/qk + lib quokka_cli (apresentação)
```

- Pins `=idevice` / `=forensic-adb` vão para `quokka-core/Cargo.toml`.
- **Truque de re-export para minimizar edits:** em `quokka-cli` (lib root), faça
  `pub use quokka_core::{device, app};` e, onde a CLI usava `crate::ui::format_bytes`
  etc., re-exporte os puros em `cli::ui` via `pub use quokka_core::fmt::*;`, e em
  `cli::commands::media`/`analyze` via `pub use quokka_core::logic::...::*;`. Assim
  os ~44 `crate::ui::*` e as refs `crate::commands::<puro>` espalhadas continuam
  resolvendo sem reescrever cada call site.
- `git mv` preserva histórico — prefira a editar+criar.

### 4e — testes e docs

- Testes que usam `FakeDevice`/facade (`tests/integration.rs` bloco `mod facade`,
  e os de `device`/`app`) → exercitam `quokka_core`. Os de parser do clap +
  snapshots de render ficam na CLI. e2e (`tests/e2e_*.rs`) passam a exercitar o
  facade no core.
- Atualizar `docs/ARCHITECTURE.md`, `CLAUDE.md` (seção de build/arquitetura) e o
  `README` (seção `--json` + estrutura do workspace).

## Como verificar (a cada etapa)

- O hook `PostToolUse` roda `cargo fmt && cargo clippy --all-targets -- -D warnings
  && cargo test` após cada edição em `src/**/*.rs`, `tests/**/*.rs`, `Cargo.toml`.
  **Ele NÃO usa as features e2e** — valide-as à mão:
  `cargo clippy --all-targets --features e2e,e2e-android`.
- Edições via `Bash` (sed, cat) **não** disparam o hook — rode os 3 checks
  manualmente depois.
- Baseline atual: 353 testes lib + 51 integração, verdes.

## Prompt de retomada (após /clear)

> "Continua o Passo 4 da Fase 2 a partir do sub-passo 4c, na branch
> `phase2-core-facade`. Lê primeiro
> `docs/superpowers/plans/2026-06-02-phase2-step4-handoff.md`,
> `docs/superpowers/specs/2026-06-01-phase2-core-facade-design.md` e
> `docs/superpowers/plans/2026-06-01-phase2-core-facade-plan.md`, e me resume o
> plano de 4c–4e antes de codar. Mantém commits verdes por sub-passo; entrega
> tudo num PR único. Não reintroduz dialoguer no device, não regride os desvios
> listados no handoff."
