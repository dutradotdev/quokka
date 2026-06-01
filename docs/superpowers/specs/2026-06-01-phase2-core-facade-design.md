# Fase 2 — `quokka-core`, facade de aplicação e `--json` total

> Design doc da Fase 2 da visão (`docs/vision.md`). Cobre a preparação do core
> open-source que destrava a GUI Tauri. Não cobre o app Tauri em si.

## Contexto

O quokka é hoje uma crate única. Tudo que toca o device passa pelo `Device`
trait (`src/device/mod.rs`), o seam que isola o resto do código das crates
`idevice` (iOS) e `forensic-adb` (Android). A Fase 0 já deixou quase todos os
tipos de saída com `Serialize` e a maior parte da lógica pura já está em funções
auxiliares dentro dos módulos de comando. A Fase 1 colocou o Android atrás do
mesmo trait sem ramificação de plataforma nos comandos.

A visão descreve três camadas que precisam ficar separadas para a CLI e a GUI
compartilharem o mesmo miolo: dados + trait + impls; lógica pura; apresentação.
A apresentação é descartável e específica de cada surface.

## Escopo

A Fase 2 da visão tem quatro peças. Este spec cobre as três que vivem neste
repositório open-source:

- **A — Split de workspace:** `quokka` vira `quokka-core` + `quokka-cli`.
- **B — Extração de lógica pura:** tirar o miolo de dentro dos `run()`
  interativos para funções puras testáveis em `quokka-core`.
- **C — Superfície serializável e `--json`:** `Serialize` em toda a saída e
  `--json` em todos os comandos cabíveis.

A quarta peça fica **fora deste spec**:

- **D — App GUI Tauri:** repositório privado, pago, separado, que depende de
  `quokka-core` (path/git). Vira projeto downstream com seu próprio spec depois
  que A+B+C estiverem prontos.

O objetivo norteador de A+B+C é deixar o `quokka-core` tão redondo que o app
Tauri seja quase só fiação: um `#[tauri::command]` de uma linha por função do
facade, mais a UI. Nenhuma lógica, nenhum DTO, nenhum parsing reescrito do lado
da GUI.

## Decisões

1. **Facade de aplicação (Nível 2).** Além dos ingredientes (trait + tipos +
   lógica pura), o `quokka-core` expõe um conjunto de funções de aplicação
   assíncronas e neutras de surface, cada uma devolvendo um DTO `Serialize` já
   projetado. A CLI consome essas funções (o `run()` vira renderização por cima
   do DTO), o `--json` imprime o DTO, e o Tauri envolve cada uma. O core **não**
   traz glue Tauri (sem feature `tauri`, sem `#[tauri::command]` no core) para
   não acoplar o open-source ao framework da GUI.

2. **Streaming exposto como `Receiver` de DTOs (opção a).** `logs` e `capture`
   continuam devolvendo um `Receiver`/`PacketStream` de tipos `Serialize`
   (`LogEntry`, `Packet`). A CLI renderiza a TUI por cima; o `--json` vira
   NDJSON ao vivo; a GUI faz a ponte `mpsc → evento Tauri`. O core não conhece
   TUI nem evento.

3. **`DeviceError` é o erro do facade (sem tipo paralelo).** `DeviceError` ganha
   `Serialize` na forma `{ kind, message }`. Ele já carrega as mensagens
   acionáveis. A GUI ramifica no `kind`, a CLI mostra o `message`. Erros
   `anyhow` das bordas internas caem em `DeviceError::Other`. As impls do device
   seguem usando `anyhow` por dentro; o facade converte na saída.

4. **Redação como função pura.** `redact` deixa de ser lógica enterrada no
   `info`/`card` e vira função pura no core, aplicada pelo facade quando a flag
   está ligada. CLI e GUI compartilham a mesma máscara.

5. **Ordem incremental, entrega em PR único.** A implementação segue a ordem
   serializável → facade → json → split (cada etapa compila e mantém o CI/hook
   verde), mas tudo é entregue num único PR.

## Arquitetura

### Layout do workspace (estado final)

```
quokka/                      (workspace root)
├── Cargo.toml               (workspace members)
├── rust-toolchain.toml      (inalterado)
├── crates/
│   ├── quokka-core/         (lib: dados + trait + lógica pura + facade)
│   └── quokka-cli/          (bins quokka/qk + toda a apresentação)
```

### O que vai para `quokka-core`

- **`device/` inteiro:** o `Device` trait, `CaptureCapable`, as impls `real`
  (iOS), `android` e `FakeDevice`, e todos os tipos de saída. O seam vai junto;
  é o que a GUI precisa. Os pins `=` de `idevice` e `forensic-adb` migram para o
  `Cargo.toml` do core.
- **Lógica pura hoje nos comandos:** heurísticos do `analyze`
  (`live_photo_motion`, `originals_with_edited`, `old_screenshots`,
  `exact_duplicates`), agregações do `media` (`classify_by_kind`,
  `bucket_by_month`, `find_duplicate_groups`), agregação de hosts e o `parser`
  do `capture`, e as cinco camadas puras do `card` (`data`, `badges`, `render`,
  `png`, `share`).
- **Formatadores puros do `ui.rs`:** `format_bytes`, `format_bar`,
  `format_percent`, `format_optional`, `civil_from_days`, `civil_from_unix`.
- **A camada `app::*` (facade) e os DTOs novos.**

### O que fica em `quokka-cli`

- Bins `quokka.rs`/`qk.rs` e o `lib.rs` (parse clap + dispatch).
- Toda a apresentação: TUIs ratatui (`apps`, `analyze`, `capture`, `logs`,
  `sidebar`), o renderer `dashboard`, `menu`, `device_action`, prompts
  `dialoguer`, spinners/progress.
- Pedaços terminal-coupled do `ui.rs`: detecção de TTY
  (`stdin_is_interactive`, `stdout_is_interactive`, `non_interactive_forced`),
  `wait_for_enter`, `spinner`, `progress_bar`, `terminal_width`, `now_unix`.
- I/O de borda: o `run()` do `card` que grava o PNG e abre o Preview; o
  `update` (GitHub/instalador).

### Regra de corte

Se usa `ratatui`/`indicatif`/`crossterm`/`dialoguer`/`std::process::Command`/
escrita em `std::fs` → fica na CLI. Se é dado, decisão lógica ou projeção → vai
para o core. O `card/mod.rs` se parte: as cinco camadas puras vão para o core; o
`run()` que grava PNG e abre o Preview fica na CLI.

### Facade (`quokka-core::app`)

Uma função de aplicação por operação:

```text
app::status(&dyn Device)                         -> Result<DeviceStatus, DeviceError>
app::info(&dyn Device, redact: bool)             -> Result<DeviceInfo, DeviceError>
app::apps(&dyn Device, on_batch)                 -> Result<Vec<App>, DeviceError>
app::analyze(&dyn Device, on_progress)           -> Result<AnalyzeReport, DeviceError>
app::media(&dyn Device, find_dupes, on_progress) -> Result<MediaReport, DeviceError>
app::delete_files(&dyn Device, &[paths])         -> Result<DeleteOutcome, DeviceError>
app::card(&dyn Device, now_unix, redact)         -> Result<RenderedCard, DeviceError>
app::reboot(&dyn Device)                         -> Result<(), DeviceError>
app::shutdown(&dyn Device)                       -> Result<(), DeviceError>
app::stream_logs(&dyn Device)                    -> Result<Receiver<Result<LogEntry>>, DeviceError>
// capture: via Device::as_capture() -> CaptureCapable::capture_packets() (inalterado)
// list_devices(): já existe no device layer
```

Cada função chama o device, aplica a lógica pura e devolve um DTO `Serialize` já
projetado. Os callbacks de progresso (`on_progress` do walk) e de enrich
(`on_batch` do `with_dynamic_sizes`) ficam na assinatura para a UI mostrar
atualização ao vivo; a GUI faz a ponte deles para eventos do mesmo jeito que o
streaming.

### DTOs novos

- `AnalyzeReport` — arquivos ordenados por tamanho + marcações do auto-mark.
- `MediaReport` — counts por kind, buckets por mês, top-N maiores, grupos
  duplicados.
- `DeleteOutcome` — resultado da deleção (sucesso/erro por caminho).
- `RenderedCard` — `{ data: CardData, svg: String, png: Vec<u8> }`.

Os tipos internos que alimentam essas projeções (`CardData` e os agregados de
media/analyze) ganham `Serialize` e viram públicos no core. Todo o resto
(`DeviceStatus`, `App`, `MediaFile`, `Packet`, `LogEntry`, `Battery`,
`Storage`, `StorageBreakdown`, `DeviceInfo`, `WalkProgress`, `Platform`,
`DeviceListing`) já é `Serialize`.

### Erros

`DeviceError` ganha `#[derive(Serialize)]` e serializa como `{ kind, message }`,
onde `kind` é o nome da variante e `message` é o `Display` atual (que já diz o
que aconteceu e o que fazer). O facade devolve `Result<T, DeviceError>`;
conversões de `anyhow::Error` das bordas internas viram `DeviceError::Other`.

### `--json`

- **One-shot** (`status`, `info`, `apps`, `analyze`, `media`, `devices`):
  `--json` suportado em todos. O dispatch no `lib.rs` vira genérico: chama o
  facade e, se `cli.json`, imprime `serde_json` do DTO; senão renderiza. O gate
  `command_supports_json` é removido.
- **`logs --json`:** NDJSON ao vivo, uma linha por `LogEntry` (implica
  `--no-tui`).
- **`capture --json`:** NDJSON de `Packet` no modo stream. Modos de agregação
  (`hosts`/`dns`/`sni`) ficam texto nesta fase; NDJSON deles é extensão futura,
  fora de escopo aqui.
- **`card`:** fora do `--json` (é comando de imagem). A GUI consome
  `RenderedCard` direto pelo facade.

## Testes

- **Lógica pura** → testes unitários em `quokka-core` (os existentes migram
  junto).
- **Facade** → testes de integração contra `FakeDevice`, chamando `app::*` e
  asserindo nos DTOs. Os testes de `tests/integration.rs` migram para esse
  formato.
- **Contrato serializável** → round-trip `serde` (serializa→desserializa) por
  DTO, fixando a forma que a GUI e o `--json` consomem.
- **NDJSON streaming** → facade de `logs`/`capture` com `FakeDevice` emitindo
  eventos, asserindo uma linha JSON por evento.
- **Apresentação** → permanece na CLI: renderer puro do `dashboard`, snapshots
  da TUI do `capture`, snapshots do SVG do `card` (migram para o core junto do
  render).
- **e2e** (`e2e`, `e2e-android`) → passam a exercitar o facade no core; a CLI
  mantém os testes de parser do clap.

## Ordem de implementação (PR único)

Quatro etapas, cada uma compilando e mantendo o CI/hook verde, entregues num só
PR:

1. **Fronteira serializável.** `Serialize` no `DeviceError` (`{ kind, message }`)
   e nos poucos tipos internos que faltam (`CardData` e cia.). Testes de
   round-trip. Zero mudança de comportamento.
2. **Facade + DTOs.** Cria `app::*`, os DTOs novos e a redação como função pura.
   Reescreve cada `run()` para chamar o facade e só renderizar por cima. Move a
   lógica pura para o layout de módulos que ela terá no core. Testes do facade
   via `FakeDevice`.
3. **`--json` genérico.** Dispatch genérico no `lib.rs` por cima do facade,
   remove o gate `command_supports_json`, liga NDJSON em `logs`/`capture`
   stream. Snapshots de JSON por comando.
4. **Split de workspace.** `git mv` para `crates/quokka-core` +
   `crates/quokka-cli`, `Cargo.toml` de workspace, ajuste de paths de `use`,
   pins migram para o core. Mecânico, porque 2 e 3 já organizaram os módulos por
   pureza. Atualiza `ARCHITECTURE.md` e `CLAUDE.md` para descrever o workspace e
   o facade.

O hook `PostToolUse` e o CI seguem rodando `fmt`/`clippy`/`test` da raiz do
workspace sem mudança. `rust-toolchain.toml` fica.

## Invariantes que não mudam

- O `Device` trait continua o contrato. Específico-de-plataforma fica atrás
  dele. Nenhum comando ramifica por plataforma.
- Nenhum tipo de `idevice`/`forensic-adb` vaza pela superfície pública do
  `device`.
- Sem overengineering: sem plugin system, sem message bus, sem camada de
  adapters genérica, sem glue Tauri no core. Trait + tipos `Serialize` + facade
  + workspace entregam as duas separações (CLI e GUI).
- iOS/Android open source (MIT); GUI privada e paga.

## Fora de escopo

- O app Tauri (Fase D) e qualquer código específico de Tauri.
- NDJSON para os modos de agregação do `capture` (`hosts`/`dns`/`sni`).
- `--json` para o `card`.
- Qualquer refactor não relacionado à separação core/cli.

## Resultado para a Fase D

O repositório Tauri privado adiciona `quokka-core` como dependência (path/git) e
escreve só um `#[tauri::command]` de uma linha por função `app::*`, mais a UI em
JS/HTML. Os DTOs, a lógica pura, o parsing e o render do card já vêm prontos e
testados do core.
