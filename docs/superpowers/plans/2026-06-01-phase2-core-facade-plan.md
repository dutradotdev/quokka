# Implementation Plan — Fase 2 (quokka-core + facade + --json)

Date: 2026-06-01
Spec: `docs/superpowers/specs/2026-06-01-phase2-core-facade-design.md`
Status: Approved, ready to implement starting with Passo 1
Branch: `phase2-core-facade`

**Entrega em PR único.** Os quatro passos abaixo são commits na mesma branch,
na ordem serializável → facade → json → split. Cada commit compila e mantém
`cargo fmt && clippy -D warnings && test` verde (o hook `PostToolUse` já força
isso a cada edição em `.rs`/`Cargo.toml`). Não há merge intermediário; o split
de workspace é o último commit porque os passos 2 e 3 já deixam os módulos
organizados por pureza, tornando o recorte mecânico.

Testes fazem parte de cada passo, não são deixados para o fim.

## Passo 1 — Fronteira serializável (zero comportamento novo)

**Entregáveis:**
- `DeviceError` (`src/device/mod.rs:27`) passa a serializar como
  `{ "kind": "<Variant>", "message": "<Display>" }`. Como `thiserror` deriva o
  `Display` mas não o JSON desejado, implementar `Serialize` à mão
  (`serialize_struct` com `kind` = nome da variante via um `fn kind(&self) ->
  &'static str`, `message` = `self.to_string()`). Só `Serialize` — nada
  desserializa `DeviceError` em Rust (a GUI lê o JSON em JS).
- `Serialize` na família `CardData` em `src/commands/card/data.rs`: `CardData`,
  `StorageBreakdownRows`, `TopApp`, `StorageFallback`, `AppsJailbreakLabel`,
  `HealthTier`.

**Testes (unit):**
- `DeviceError` serializa para o `{ kind, message }` esperado, uma asserção por
  variante representativa (`NotPaired`, `AdbNotFound`, `Other`).
- `CardData` faz round-trip `serde` (serializa→desserializa→igual).

**Critérios de aceite:**
- `fmt && clippy -D warnings && test` verde.
- Nenhuma mudança de comportamento de runtime; diff é só derives + impl de
  `Serialize`.

**Anti-objetivos:** nada de facade nem `--json` ainda.

---

## Passo 2 — Facade `app::*` + DTOs + redação pura

**Entregáveis:**
- Novo módulo `src/app/` (vira `quokka-core::app` no Passo 4), com uma função
  por operação, todas devolvendo `Result<_, DeviceError>`:
  - `app::status`, `app::info(redact)`, `app::apps(on_batch)`,
    `app::analyze(on_progress)`, `app::media(find_dupes, on_progress)`,
    `app::delete_files(paths)`, `app::card(now_unix, redact)`, `app::reboot`,
    `app::shutdown`, `app::stream_logs`.
  - Captura segue por `Device::as_capture()` (inalterado).
- DTOs novos, todos `Serialize`/`Deserialize`:
  - `AnalyzeReport { files: Vec<MediaFile> ordenados, marks: AutoMarks }`
    (reaproveita `heuristics::detect_all`).
  - `MediaReport` (promove o report interno do `media.rs`: counts por kind,
    buckets por mês, top-N, grupos duplicados).
  - `DeleteOutcome { deleted: Vec<String>, failed: Vec<(String, String)> }`.
  - `RenderedCard { data: CardData, svg: String, png: Vec<u8> }`.
- Redação como função pura: `app::redact::device_info(DeviceInfo) -> DeviceInfo`
  e `app::redact::card_data(CardData) -> CardData`, aplicadas pelo facade quando
  a flag está ligada. Remover a lógica de máscara enterrada em `info.rs`/`card`.
- Reescrever cada `commands::*::run()` para: chamar `app::*`, depois só
  renderizar/interagir por cima do DTO. As TUIs (`apps`, `analyze`, `capture`,
  `logs`, `sidebar`), o renderer `dashboard` e os prompts continuam na camada de
  comando.
- Mover a lógica pura para o layout que ela terá no core (já em `src/`, mas na
  forma final): heurísticos do `analyze`, agregações do `media`, agregação de
  hosts + `parser` do `capture`, e as cinco camadas puras do `card`.

**Testes (integração via `FakeDevice`):**
- Um teste por função `app::*` asserindo o DTO retornado (não a saída de texto).
- Round-trip `serde` de `AnalyzeReport`, `MediaReport`, `DeleteOutcome`,
  `RenderedCard`.
- `app::info(redact: true)` mascara serial/UDID/IMEI/MAC; `redact: false` não.
- Migrar `tests/integration.rs` para asserir nos DTOs do facade.

**Critérios de aceite:**
- `fmt && clippy && test` verde.
- Saída de runtime da CLI idêntica à de hoje (os renderers consomem o DTO; o
  texto não muda).

---

## Passo 3 — `--json` genérico

**Entregáveis:**
- Dispatch genérico no `src/lib.rs`: para cada one-shot, chamar `app::*` e, se
  `cli.json`, imprimir `serde_json::to_string_pretty(&dto)`; senão renderizar.
- Remover o gate `command_supports_json` (`lib.rs:259`) — `status`, `info`,
  `apps`, `analyze`, `media`, `devices` passam todos a aceitar `--json`.
- `logs --json` → NDJSON ao vivo (uma linha por `LogEntry`); implica caminho
  `--no-tui`.
- `capture --json` → NDJSON de `Packet` no modo stream. Modos `hosts`/`dns`/`sni`
  seguem texto (anotado como extensão futura).
- `card` permanece fora do `--json`.

**Testes:**
- Snapshot de JSON por comando one-shot (saída estável dado um `FakeDevice`
  fixo).
- `logs --json`/`capture --json`: N eventos do fake → N linhas JSON válidas,
  uma por evento.
- Atualizar os testes de parser do clap se a superfície de flags mudar.

**Critérios de aceite:**
- `fmt && clippy && test` verde.
- `qk status --json`, `qk media --json`, etc. emitem o DTO; `qk card --json`
  ainda é rejeitado com mensagem clara.

---

## Passo 4 — Split de workspace (mecânico)

**Entregáveis:**
- Estrutura `crates/quokka-core/` + `crates/quokka-cli/`; `Cargo.toml` de
  workspace na raiz com os dois members.
- `git mv` para o core: `device/` inteiro (trait, `real`, `android`,
  `FakeDevice`, tipos), `app/`, a lógica pura movida no Passo 2, os formatadores
  puros do `ui.rs` (`format_bytes`, `format_bar`, `format_percent`,
  `format_optional`, `civil_from_days`, `civil_from_unix`).
- `git mv` para a CLI: bins, `lib.rs`, as TUIs/`dashboard`/`menu`/
  `device_action`, os pedaços terminal-coupled do `ui.rs`, o `run()` do `card`
  (escrita de PNG + `open`), `update`.
- Pins `=idevice` / `=forensic-adb` migram para o `Cargo.toml` do core.
- Ajustar paths de `use` (`crate::` → `quokka_core::` na CLI).
- Mover os testes: integração com `FakeDevice` + facade → `crates/quokka-core/`;
  parser do clap + snapshots de render → `crates/quokka-cli/`; `e2e`/
  `e2e-android` passam a exercitar o facade no core.
- `examples/chaos_cards.rs` → exemplo do core (usa render puro).
- Atualizar `docs/ARCHITECTURE.md` e `CLAUDE.md` descrevendo o workspace e o
  facade.

**Critérios de aceite:**
- `fmt && clippy && test` verde da raiz do workspace.
- `cargo test --features e2e` e `--features e2e-android` compilam.
- Diff é puramente movimentação + paths + `Cargo.toml`; sem mudança de lógica.
- O hook `PostToolUse` e o CI seguem rodando da raiz sem ajuste.
- `rust-toolchain.toml` inalterado.

---

## Ordem & dependências

```
1 (serializável) → 2 (facade + DTOs) → 3 (--json) → 4 (split de workspace)
```

Tudo na branch `phase2-core-facade`, um commit por passo, PR único no fim. Após
cada passo, pausa para confirmação antes do próximo.

## Quando re-entrar (e.g. após /clear)

Prompt sugerido para retomar:

> "Implementa o Passo 1 do plano em
> `docs/superpowers/plans/2026-06-01-phase2-core-facade-plan.md`, baseado na
> spec em `docs/superpowers/specs/2026-06-01-phase2-core-facade-design.md`.
> Antes de começar, lê os dois arquivos e me apresenta um resumo do que vai
> fazer."

---

## Status de execução (2026-06-01)

- **Passo 1** — concluído (`d30d2cc`). `DeviceError`/`CardData` serializáveis.
- **Passo 2** — concluído (`fa6ecaa`). Facade `app::*`, DTOs, `app::redact`.
- **Passo 3** — concluído (`8039e0f`). `--json` genérico + NDJSON em `logs`.
- **Passo 4** — **concluído**. Sub-passos:
  - **4a** (`5f2c2b1`) — `device::connect` livre de apresentação: picker extraído
    para a CLI via callback `DeviceSelector` (`ui::select_device`). `dialoguer`
    saiu do `device/`.
  - **4b** (`c616ba1`) — parser de syslog movido para `device::syslog`. O
    `device/` não referencia mais nenhum módulo de comando — autocontido.
  - **4c** (`8f35cd9`) — lógica pura fatiada em módulos core-shaped no crate
    único: `src/fmt.rs` (formatadores + `now_unix`), `src/logic/{mod,analyze,
    media}.rs` (`top_n_by_size`, heurísticos, agregações + `MediaReport`),
    `src/card/` (as 6 camadas puras). Os comandos mantêm `run`/render/TUI e
    re-exportam os símbolos puros nos caminhos antigos; `app`/`card::data`
    repontados para `crate::{logic,card,fmt}`. `device`+`app`+`fmt`+`logic`+
    `card` viram uma ilha sem `crate::commands`/`crate::ui`.
  - **4d** (`eeb91a0`) — workspace `crates/quokka-core` + `crates/quokka-cli`.
    Core = device+app+fmt+logic+card+assets (+ pins `=idevice`/`=forensic-adb`
    + stack SVG→PNG); CLI = bins+lib+commands+ui, depende do core e re-exporta
    `quokka_core::{app,card,device,fmt,logic}` na raiz (diff = só git mv +
    swap de re-export no lib.rs + Cargo.tomls). Features `e2e`/`e2e-android`
    encaminham CLI→core (`device::bench`).
  - **4e** — testes do facade movidos para `crates/quokka-core/tests/facade.rs`
    (sem duplicar fixtures); `tests/llm` voltou para a raiz (não é alvo cargo);
    docs (`ARCHITECTURE.md`, `CLAUDE.md`, `README`) atualizadas para workspace +
    facade + `--json`. Total verde: 353 unit (218 cli + 135 core) + 51
    integração (43 cli + 8 core facade) = 404; clippy limpo incl.
    `--features e2e,e2e-android`.

  Pendência fora do meu alcance: o matcher do hook `PostToolUse` em
  `.claude/settings.json` ainda casa `src/**`/`tests/**`; precisa virar
  `crates/**` (a edição foi bloqueada pelo classifier — decisão do Lucas).

Desvios assumidos nos Passos 1–3 (sinalizados para revisão):

- **Relocação física da lógica pura adiada para o Passo 4.** O facade reusa a
  lógica pura *no lugar* (em `commands::*`) por enquanto. Isso é o que torna o
  Passo 4 mais pesado do que o "mecânico" originalmente previsto.
- **`qk info --json` mudou de forma** — agora é o DTO `DeviceInfo` plano em
  camelCase (o payload que a GUI consome), não mais o objeto aninhado
  snake_case artesanal. Quebra para quem fazia parsing do formato antigo.
- **`app::analyze` recebe `now_unix`** como parâmetro (determinismo/teste), em
  vez de chamar `now_unix()` por dentro.
- **`capture --json` ficou de fora** (os modos `hosts`/`dns`/`sni` + a TUI
  tornam o NDJSON de stream uma mudança maior e arriscada). `capture --json`
  hoje é rejeitado com mensagem clara, igual ao `card --json`.

## Descobertas do Passo 4 (bloqueadores do split limpo)

Um `quokka-core` **livre de apresentação** (o que a Fase D precisa) exige
resolver, antes do `git mv`, três acoplamentos que hoje cruzam a fronteira:

1. **`device::connect` tem um picker `dialoguer`** embutido
   (`src/device/mod.rs:664` e `:1189`) — apresentação interativa dentro do
   módulo que deveria virar core. Decisão de design pendente: extrair o picker
   para a CLI (o `connect` retorna uma lista e a CLI escolhe) ou aceitar uma
   dependência de `dialoguer` no core. **Precisa de decisão do Lucas.**
2. **`device/` depende de `commands::logs::parser`** (`parse_syslog_line`,
   `is_continuation`) — o parser de syslog é puro e precisa migrar para o core
   (ex.: `core::logic::syslog`) antes de `device/` poder ir junto.
3. **`app/` depende de lógica pura ainda dentro de módulos de comando** que
   misturam puro + apresentação: `commands::media` (agregações), 
   `commands::analyze::heuristics` (+ `sort_by_size`/`ext_lower`/`kind_from_ext`),
   `commands::card::{data,badges,render,png,share,emoji}` (puros), e os
   formatadores puros de `ui.rs` (`format_bytes`, `civil_from_unix`,
   `now_unix`, …) + `commands::top_n_by_size`. Cada um precisa ser fatiado em
   parte-pura (core) e parte-apresentação (CLI).

### Sub-plano sugerido para o Passo 4 (quando retomado)

Truque de baixo atrito: a CLI **re-exporta** os símbolos do core nos caminhos
antigos (`pub use quokka_core::fmt::*` em `cli::ui`; `pub use
quokka_core::logic::media::*` em `cli::commands::media`; etc.), evitando reescrever
as ~44 chamadas `crate::ui::*` e as referências `crate::commands::<puro>` espalhadas.

1. Decidir o destino do picker do `connect` (pergunta 1 acima).
2. Extrair para `quokka-core`: `device/` (após mover o parser de syslog para
   `core::logic::syslog` e resolver o picker), `app/`, `core::fmt` (formatadores
   puros + `now_unix`), `core::logic::{media,analyze}`, `core::card` (camadas
   puras), `core::top_n_by_size`.
3. Criar o `Cargo.toml` de workspace + `crates/quokka-core` + `crates/quokka-cli`;
   mover os pins `=idevice`/`=forensic-adb` para o core.
4. Na CLI: re-exportar os símbolos do core nos caminhos antigos; manter
   bins/`lib.rs`/TUIs/renderers/`ui` terminal-coupled.
5. Mover os testes (facade+`FakeDevice` → core; parser do clap + snapshots → CLI;
   `e2e`/`e2e-android` → exercitar o facade no core).
6. Atualizar `ARCHITECTURE.md`, `CLAUDE.md` e o `README` (seção `--json`).
