# Implementation Plan — Phase 6 TUI

Date: 2026-05-26
Spec: `docs/superpowers/specs/2026-05-26-capture-tui-design.md`
Status: Approved, ready to implement starting with 6.0

Each phase is mergeable on its own. Each ends with explicit user
confirmation before the next phase begins. Tests are part of every
phase — not bolted on at the end.

## Fase 6.0 — Module split (preparatório, zero comportamento novo)

**Entregáveis:**
- `src/commands/capture.rs` → diretório `src/commands/capture/`
- Mover `parse_summary` + DNS/SNI parsers → `parser.rs`
- Mover `CaptureFile` + `SaveFormat` → `pcap_io.rs`
- Mover `HostAggregator` + `HostStats` → `hosts.rs`
- `mod.rs` re-exporta tudo via `pub use` para que
  `crate::commands::capture::Foo` continue funcionando

**Critérios de aceite:**
- Todos os 200+ testes existentes passam sem mudança de import
- `cargo fmt && clippy -D warnings && test` verde
- Diff é puramente movimentação de código (sem mudança de lógica)

**Anti-objetivos:** nenhuma feature nova.

---

## Fase 6.1 — Core App + ingest (sem render, sem input)

**Entregáveis:**
- `src/commands/capture/tui.rs` com:
  - `enum View { Stream, Hosts }`
  - `struct DisplayRow { pkt: Packet, parsed: Option<ParsedPacket> }`
  - `enum FilterField { App, Pid, Port, Proto, Interface }`
  - `struct PromptState { field, buffer, error }`
  - `struct App { view, rows, aggregator, filter, stream_state,
    hosts_state, prompt, stats }`
  - `App::ingest(pkt)` (parse + filter check + push + overflow + aggregator)
  - `App::apply_filter(new_filter)` (retain + reset aggregator + rebuild)
- Variant `Mode::Headless` em `Options` (ou similar) que entra pelo path
  do TUI sem render

**Testes (unit, ~10 novos):**
- `ingest` aceita pacotes que casam, dropa os que não casam
- Ring buffer overflow descarta o mais antigo
- `apply_filter` esconde rows não-matching E re-popula aggregator
- Filter vazio aceita tudo
- DisplayRow cacheia parse (não re-parseia em getter)
- `stats.count` reflete só pacotes aceitos (não os dropped pelo filtro)

**Critérios:** lib testes verdes, sem feature externa visível ainda.

---

## Fase 6.2 — Stream view rendering (sem input ainda)

**Entregáveis:**
- `App::draw(frame)` que dispatcha para `draw_stream(frame, &app)`
  quando view == Stream
- Layout: top bar (stats), filter row, table (header + rows),
  detail pane (selected packet), hotkey footer
- Cores: ↑ verde, ↓ vermelho, drops em amarelo no top bar
- `Style` constants em um único módulo `style.rs`
- `StreamViewState { selected: usize, scroll_offset: usize }`

**Testes:**
- Snapshot via `ratatui::backend::TestBackend(100, 30)`:
  stream com 3 rows
- Snapshot empty state ("no packets yet")
- Snapshot prompt aberto com input parcial
- Snapshot terminal too small (40×20)
- Snapshot com filtro ativo no top bar

**Critérios:** snapshots aprovados, dirty flag funciona (não re-render
em idle).

---

## Fase 6.3 — Hosts view rendering

**Entregáveis:**
- `draw_hosts(frame, &app)` — tree-table com
  `HostsViewState { selected_process: usize, selected_host: Option<usize>,
  collapsed: HashSet<u32> }`
- Detail pane mostra histórico do host selecionado (precisa anotar
  timestamp por host no aggregator — pequena extensão de
  `HostStats { first_seen, last_seen, recent: Vec<(time, dir, bytes)> }`
  capped em ~20 entradas)

**Testes:**
- Snapshot hosts view com 2 processos × 3 hosts cada
- Snapshot com 1 processo colapsado
- Snapshot detail pane do host selecionado
- Unit: collapse/expand state toggles

**Critérios:** snapshots aprovados.

---

## Fase 6.4 — Event loop + hotkeys

**Entregáveis:**
- `tui::run(device, opts)` — substitui o `run()` atual de `capture.rs`
- TTY check no entry (falha cedo com mensagem clara)
- RAII guard para restaurar terminal em panic/exit
- `tokio::select!` { recv pacote | crossterm event | redraw tick 30Hz |
  ctrl_c }
- `App::handle_key(key) -> KeyOutcome { Continue, Quit, Dirty }`
- State machine completa: idle (a/p/P/i/d/c/Tab/↑↓/Enter/q) +
  prompt (chars/Enter/Esc/Ctrl-C)

**Testes:**
- State machine 100% coberto: cada (prompt_state, key) → expected
  mutation, sem ratatui
- Erro inline de validação (pid=abc → erro permanece, prompt continua
  aberto)

**Critérios:**
- cargo test verde
- **Validação manual obrigatória com iPhone real:** 1 min de captura,
  exercitar cada hotkey, redimensionar terminal, sair com q, sair com
  Ctrl-C
- Validação manual: `qk capture | cat` falha com mensagem clara

---

## Fase 6.5 — Wire CLI flags + integração

**Entregáveis:**
- Pre-popular `App::filter` a partir das flags CLI (`--app`, `--proto`,
  etc.)
- `qk capture --hosts` vira "abrir TUI já em view=Hosts"
- `--save` continua funcionando em paralelo (writer plumbing por baixo
  do TUI)
- `--max` conta pacotes capturados (não renderizados) e fecha TUI
  quando atinge
- Integration tests existentes adaptados para `Mode::Headless`
- Confirmar que `--dns` e `--sni` continuam intactos (renderizam
  linha-a-linha como hoje)

**Testes:**
- Integration: cada CLI flag pre-popula filter corretamente
- Integration: `--hosts` flag faz view inicial = Hosts
- Integration: `--max N` encerra após N pacotes (com `Mode::Headless`)

**Critérios:**
- Validação manual final: cobrir todos os fluxos do roteiro (captura
  limpa, com filtros CLI pre-aplicados, com `--save`, com `--hosts`
  direto)
- Atualizar README seção `qk capture` com screenshot da TUI (opcional
  mas recomendado)

---

## Ordem & dependências

```
6.0 (split) → 6.1 (core) → 6.2 (stream draw) → 6.3 (hosts draw)
            → 6.4 (events) → 6.5 (wire CLI)
```

Cada fase é commit separado, mergeable em isolamento. Após cada fase,
pausa para confirmação igual fizemos no roteiro original (Phases 1–5).

## Quando re-entrar (e.g. após /clear)

Prompt sugerido para retomar:

> "Implementa Fase 6.0 do plano em
> `docs/superpowers/plans/2026-05-26-capture-tui-plan.md`,
> baseado na spec em
> `docs/superpowers/specs/2026-05-26-capture-tui-design.md`.
> Antes de começar, lê os dois arquivos e me apresenta um resumo
> do que vai fazer."
