# Plano: revisar o backend Android do quokka

> **Como usar este documento.** Ele é auto-contido: foi escrito para ser
> implementado por uma sessão do Claude **com contexto zerado**. Leia-o inteiro
> antes de codar.

---

## 0. Contexto do projeto (leia primeiro)

**quokka** é uma CLI Rust (Mac) que inspeciona/limpa um iPhone via USB sem
jailbreak. Foi recém-estendido para suportar **Android** de forma isolada. Há
também a intenção futura de uma GUI Tauri (repo privado, paga) que consome o
mesmo núcleo — ver `docs/vision.md`.

### Regras de arquitetura inegociáveis

1. **O `Device` trait (`src/device/mod.rs`) é a costura.** Toda operação no
   aparelho passa por ele. Comandos recebem `&dyn Device` e **nunca** sabem a
   plataforma. **Proibido** qualquer `if android`/`if ios` em `src/commands/`.
   Se um comando precisar saber a plataforma, o trait vazou — conserte o trait.
2. **Nenhum tipo de transporte vaza acima de `device/mod.rs`.** O iOS isola o
   crate `idevice` dentro de `mod real`. O Android isola tudo dentro de
   `src/device/android.rs`. Nenhum tipo `idevice` **nem** de adb pode aparecer
   na superfície pública de `device/mod.rs`.
3. **Dependências pré-1.0 são pinadas com `=`** (o `idevice` é `=0.1.61`). Se
   este plano adotar uma crate nova de adb, pine com `=x.y.z` pelo mesmo motivo.
4. **Testes na mesma mudança que o código.** Parsers são funções puras testadas
   com fixtures (strings de exemplo da saída real do comando).
5. **Hook automático:** um `PostToolUse` em `.claude/settings.json` roda
   `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test` após
   qualquer edição em `src/**/*.rs`, `tests/**/*.rs` ou `Cargo.toml`. Falhas
   voltam como erro bloqueante — corrija antes de seguir. Clippy é `-D warnings`
   (zero warnings). **Não** rode fmt/clippy/test manualmente após editar Rust; o
   hook já faz.
6. **Comandos de validação:** `cargo test` (unit+integração, sem aparelho),
   `cargo test --features e2e` (precisa de iPhone real), `cargo run --bin qk -- <cmd>`.

### Onde o Android vive

- `src/device/android.rs` — backend Android inteiro (struct `AndroidDevice`,
  `impl Device for AndroidDevice`, parsers puros + testes).
- `src/device/mod.rs` — declara `mod android;` e despacha em `connect()`:
  ```rust
  pub async fn connect(udid: Option<&str>, platform: Option<Platform>) -> Result<Box<dyn Device>> {
      match platform {
          None | Some(Platform::Ios) => Ok(Box::new(real::RealDevice::connect(udid).await?)),
          Some(Platform::Android) => Ok(Box::new(android::AndroidDevice::connect(udid).await?)),
      }
  }
  ```
  `--platform android` / `QK_PLATFORM=android` aciona o Android; ausência = iOS
  (sem custo de probing adb para quem só usa iPhone).

### O `Device` trait (assinatura atual)

```rust
#[async_trait]
pub trait Device: Send + Sync {
    async fn status(&self) -> Result<DeviceStatus>;
    async fn apps(&self) -> Result<Vec<App>>;                 // user apps, sizes "estáticos"
    async fn all_apps(&self) -> Result<Vec<App>>;             // + system apps
    async fn with_dynamic_sizes(&self, apps: Vec<App>, on_batch: BatchCallback) -> Result<Vec<App>>;
    async fn app(&self, bundle_id: &str) -> Result<Option<App>>;
    async fn uninstall_app(&self, bundle_id: &str) -> Result<()>;
    fn media_roots(&self) -> &'static [&'static str];
    async fn afc_walk(&self, roots: &[&str], on_progress: WalkCallback) -> Result<Vec<MediaFile>>;
    async fn afc_delete(&self, path: &str) -> Result<()>;
    async fn info(&self) -> Result<DeviceInfo>;
    async fn reboot(&self) -> Result<()>;
    async fn shutdown(&self) -> Result<()>;
    async fn stream_logs(&self) -> Result<tokio::sync::mpsc::Receiver<Result<LogEntry>>>;
    fn as_capture(&self) -> Option<&dyn CaptureCapable> { None } // pcapd: iOS-only; Android usa o default None
}
```

`App { bundle_id: String, name: String, size_bytes: u64, is_system: bool, install_date_unix: Option<i64> }`.
Os tipos do device derivam `Serialize`/`Deserialize` (`#[serde(rename_all = "camelCase")]`).

---

## 1. Estado atual do backend Android (o que já existe)

`src/device/android.rs` hoje **spawna o binário `adb`** (`tokio::process::Command`)
e parseia stdout. Funções existentes (todas com testes de fixture):

- `run_adb(args) -> Result<String>` / `spawn_error` — wrapper do CLI `adb`.
- `AndroidDevice { serial }` + `connect`, `shell`, `getprop`.
- `parse_adb_devices`, `select_serial` (single/target/unauthorized/multiple).
- `parse_battery` (dumpsys battery), `parse_df` (df /data).
- `parse_pm_packages` (`pm list packages`), `parse_find_output` (`find -printf`).
- `parse_logcat_line` (`logcat -v threadtime`), `shell_single_quote`.
- `app_from_id(id, is_system)` → **`size_bytes: 0`, `name: id`** ← os dois pontos
  degradados.

**Duas fraquezas que este plano resolve:**

1. **`size_bytes = 0`** — conservadorismo errado. Dá para ter tamanho real sem
   root (ver pesquisa abaixo). **Esta é a feature mais importante para o produto:
   o usuário quer ver o tamanho dos apps para deletar os gordos.**
2. **`afc_walk` usa `find -printf`** — o `-printf` do toybox pode não existir em
   algumas versões de Android. Frágil.

`name = id` (sem o label "Spotify") é uma degradação **aceitável** por enquanto
(polish opcional, Estágio 4).

---

## 2. Pesquisa que fundamenta o plano (com fontes)

### 2.1. Tamanho de app SEM root → `adb shell dumpsys diskstats`

O usuário `shell` do adb (uid 2000) roda `dumpsys diskstats`, que invoca o serviço
de disco do framework e devolve tamanhos por-app **sem root**, desde Android 8
(API 26). Formato: **arrays paralelos casados por índice**:

```
Package Names: [com.spotify.music, com.whatsapp, com.foo, ...]
App Sizes: [180000000, 95000000, ...]        ← código/APK
App Data Sizes: [1200000000, 800000000, ...] ← dados do usuário
Cache Sizes: [50000000, 30000000, ...]       ← cache
```

`size_bytes` por app = **`App Size + App Data Size`** (espelha "iPhone Storage" =
app + documentos&dados; cache é efêmero, fora). Caveats honestos:

- Os números vêm do serviço de diskstats, que recalcula periodicamente → podem
  ser um **snapshot levemente desatualizado**. Ótimo para "quais são grandes,
  deletar"; não é precisão ao byte no segundo.
- Só **ids de pacote**, não o label. (Label = Estágio 4, opcional.)
- API 26+ (cobre ~99% dos aparelhos).
- A confusão anterior: a API `StorageStatsManager` exige permissão
  `PACKAGE_USAGE_STATS` **quando um app** consulta outro app — não é o nosso
  caso. Nós somos o usuário `shell` do adb, que dumpa o serviço direto.

Fontes:

- https://blog.backslasher.net/android-app-sizes.html
- https://gist.github.com/ckhung/501bac5587ae5f5f1b7f80cca5b3b83f
- https://androidhiro.com/source/questions/answer/220442/obtaining-app-storage-details-via-adb/

### 2.2. Transporte: crate `adb_client` (em vez de spawnar o binário)

A crate `adb_client` implementa o **protocolo do adb** direto (smart sockets),
dando acesso estruturado a:

- **enumeração de devices** (server/USB/TCP/mDNS) — tipado, não parse de texto;
- **serviço SYNC de arquivos** (`stat`/`list`/`pull`/`push`) com **tamanho e mtime
  por arquivo** estruturados → **substitui o frágil `find -printf`** do `afc_walk`;
- **install/uninstall** (pm), **reboot** (`RebootType`), e **shell**;
- erros tipados (`RustADBError`).

Trait central: `ADBDeviceExt`. Structs relevantes: `AdbStatResponse`,
`ADBStatExtendedResponse`, `ADBListItem`. **Importante:** `shell(...)` ainda volta
**texto cru** — `dumpsys`/`pm`/`getprop`/`diskstats` continuam sendo parse de
string (os parsers atuais permanecem válidos). A crate melhora **transporte +
operações de arquivo**, não a forma da saída desses comandos.

Crate é **pré-1.0** → pine com `=`, mantenha 100% atrás do módulo `android`.

Fontes:

- https://crates.io/crates/adb_client · https://docs.rs/adb_client · https://lib.rs/crates/adb_client
- https://www.synacktiv.com/en/node/1042 (protocolo adb)
- alternativas: https://crates.io/crates/forensic-adb · https://docs.rs/mozdevice

### 2.3. O que NÃO dá sem root (fora de escopo, por simetria com o iOS)

- **Label de app** ("Spotify"): mora nos recursos do APK; precisa parsear o APK.
  Degradado para o id por enquanto (Estágio 4 opcional).
- Métricas avançadas de bateria (ciclos/saúde) — degradam para `None`.
- Per-app cache cleanup, root — fora de escopo (igual "impossível sem jailbreak"
  no iOS). Documentar como tal.

---

## 3. Decisões travadas (não reabrir sem motivo)

1. **Tamanho de app via `dumpsys diskstats`** (sem root). Total = app + data.
2. **Nome do app continua o id** por enquanto (label = Estágio 4 opcional).
3. **Transporte alvo = `adb_client` em modo "adb server"** (fala com o servidor
   adb local em `127.0.0.1:5037`), **não** USB-direto. Motivo: o modo USB-direto
   reivindica a interface USB e conflita com um servidor adb rodando + exige
   libusb/permissões; o modo server reaproveita o adb do sistema (USB, auth,
   multi-device) e é o mais compatível para um companion de Mac. (Ainda exige
   `adb` instalado — aceitável; já recomendamos platform-tools.)
4. **`capture` (pcapd) permanece iOS-only** — Android não implementa
   `CaptureCapable`; `as_capture` fica no default `None`.
5. **`--platform` ausente = iOS.** Autodetecção real (sondar adb) fica para depois.
6. **Parsers são transporte-agnósticos** (recebem `&str`) → o trabalho de
   parsing (Estágio 1) sobrevive intacto à migração de transporte (Estágio 3).
   Por isso a ordem dos estágios.

---

## 4. Plano em estágios

> Faça **um estágio por vez**, cada um deixando `cargo test` + hook verdes.
> Estágio 1 é o de maior valor e menor risco — entrega a feature que o produto
> quer. Estágios 3 e 4 são maiores/opcionais.

### Estágio 1 — Tamanho de app via `dumpsys diskstats` (MUST-HAVE, pequeno)

**Objetivo:** `qk apps` no Android passa a ranquear por tamanho real, igual ao
iPhone. Não adiciona dependência nova; funciona com o backend CLI atual.

**Arquivo:** `src/device/android.rs`

1. **Parser puro novo** `parse_diskstats(output: &str) -> std::collections::HashMap<String, u64>`:
   - Extrai os arrays `Package Names: [...]`, `App Sizes: [...]`,
     `App Data Sizes: [...]` (cache ignorado por ora).
   - Casa por índice; valor = `app_size + app_data_size`.
   - Defensivo: se os arrays tiverem comprimentos diferentes, use o menor; linhas
     ausentes → ignore. Tolere espaços e a forma `[a, b, c]`.
   - Retorna `pacote → bytes`.

2. **Helper** `async fn app_sizes(&self) -> HashMap<String, u64>`:
   - `self.shell(&["dumpsys", "diskstats"]).await` → `parse_diskstats(&out)`.
   - Em erro, retorne mapa vazio (degradação graciosa: tamanhos viram 0, não crash).

3. **Mudar `app_from_id`** para receber o tamanho:
   - `fn app_from_id(bundle_id: String, is_system: bool, size_bytes: u64) -> App`.
   - `name` continua `bundle_id.clone()`.

4. **Reescrever `apps()` e `all_apps()`** para buscar `app_sizes()` uma vez e
   preencher cada `App.size_bytes` pelo mapa (0 se ausente). `apps()` = user
   (`pm list packages -3`); `all_apps()` = todos + flag `is_system` via diff com
   `-3` (lógica atual preservada).

5. **`with_dynamic_sizes`**: como o `diskstats` já dá o tamanho real em `apps()`,
   mantenha como **passthrough** (dispara um batch sintético com a entrada
   inalterada — comportamento atual). Documente que no Android o tamanho já é
   "estático real", não há fase de enrichment separada.

6. **`app(bundle_id)`**: ao encontrar o pacote, preencha o tamanho a partir de
   `app_sizes()` também (ou 0 se ausente).

**Testes (fixtures) — adicionar ao `mod tests` de `android.rs`:**

- `parse_diskstats_matches_arrays_by_index`: fixture com os 4 rótulos e 2-3
  pacotes; asserta `map["com.spotify.music"] == app+data`.
- `parse_diskstats_tolerates_missing_or_ragged_arrays`: arrays de tamanhos
  diferentes / rótulo ausente → não entra em pânico, retorna o que dá.
- Atualizar o teste `app_from_id_*` para a nova assinatura.

**Fixture de exemplo (use no teste):**

```
Package Names: [com.spotify.music, com.whatsapp]
App Sizes: [180000000, 95000000]
App Data Sizes: [1200000000, 800000000]
Cache Sizes: [50000000, 30000000]
```

Esperado: `com.spotify.music → 1_380_000_000`, `com.whatsapp → 895_000_000`.

**Definição de pronto:** parser + testes verdes; `apps`/`all_apps`/`app`
preenchem `size_bytes`; hook verde. (Validação em device real = Estágio 5.)

---

### Estágio 2 — Robustez do walk de mídia (find -printf)

**Problema:** `afc_walk` depende de `find -printf`, que pode não existir no toybox.

**Decisão:** este fix é **melhor feito junto com o Estágio 3** (SYNC do
`adb_client` dá size+mtime estruturados, eliminando o `find`). **Se** for
implementar o Estágio 3, pule este. **Se não** for fazer o Estágio 3 agora,
mantenha `find -printf` mas adicione um caminho de fallback:

- Tente `find <root> -type f -printf "%s\t%T@\t%p\n"`.
- Se a saída vier vazia/erro, faça fallback para `find <root> -type f` (só
  paths, portável) com `size_bytes = 0` e `modified_unix = 0`, e **registre via
  `log`** que os tamanhos do walk não estão disponíveis nesta versão de Android
  (princípio UX: nunca truncar/silenciar capacidade sem avisar).

Marque com `// TODO(android): substituir por adb_client SYNC stat/list — <data>`.

---

### Estágio 3 — Migrar transporte para `adb_client` (RECOMENDADO, maior)

**Pré-requisito: SPIKE primeiro.** Antes de migrar, faça um spike curto para
validar a API real da crate (a doc abaixo é inferida de resumos; confirme):

- Adicione `adb_client = "=<versão atual>"` ao `Cargo.toml` (cheque a última no
  crates.io e pine com `=`). Habilite só as features necessárias (modo server).
- Escreva um throwaway que: conecta ao server, lista devices, roda
  `shell("getprop ro.product.model")`, faz `stat`/`list` de `/sdcard/DCIM`,
  e (com cuidado) testa `install`/`uninstall`/`reboot` na assinatura.
- Confirme nomes reais: trait `ADBDeviceExt`, `AdbStatResponse`/`ADBListItem`,
  como obter um handle de device do server, e os tipos de retorno.

**Migração (mantendo os parsers intactos):**

1. **`Cargo.toml`:** adicionar `adb_client` pinado com `=`. Comentar o porquê do
   `=` (mesmo motivo do `idevice`).
2. **`AndroidDevice`** passa a guardar o handle de conexão da crate (ou os dados
   para reabri-la) em vez de só `serial: String`. `connect()` usa a enumeração
   estruturada da crate em vez de `parse_adb_devices` (pode aposentar
   `parse_adb_devices`/`select_serial` ou adaptá-los aos tipos da crate —
   preserve a semântica single/target/unauthorized/multiple e os
   `DeviceError::{NoAndroidDevice, AndroidUnauthorized, AdbCommandFailed}`).
3. **`shell()`** passa a usar o `shell` da crate; **os parsers
   (`parse_battery`/`parse_df`/`parse_pm_packages`/`parse_diskstats`/`parse_logcat_line`)
   não mudam** — continuam recebendo `&str`.
4. **`afc_walk`** passa a usar SYNC `list`/`stat` (size+mtime estruturados),
   aposentando `find -printf` e `parse_find_output`. Mantenha o filtro aos
   `media_roots` e o `on_progress`.
5. **`afc_delete`** pode continuar via `shell rm` (ou SYNC se a crate expuser
   delete). **`uninstall`/`reboot`** passam para os métodos tipados da crate.
6. **`stream_logs`** (logcat): a crate expõe shell **streaming**? Se sim, use;
   se não, mantenha o spawn do binário `adb logcat` só para este caso (streaming
   contínuo) e documente. Os parsers de logcat não mudam.
7. **Remover** `run_adb`/spawn onde a crate substituir; manter `spawn_error`
   só se ainda houver spawn (logcat).
8. **`DeviceError`:** mapear `RustADBError` → variantes existentes
   (`AdbNotFound` quando o server/binário não existe, `AdbCommandFailed(...)`
   para o resto). Sem vazar tipos da crate acima de `android.rs`.

**Invariante a manter:** nenhum tipo de `adb_client` aparece fora de
`src/device/android.rs`. O `Device` trait e os tipos neutros não mudam.

**Definição de pronto:** todos os métodos do trait migrados (ou logcat
justificadamente via spawn), parsers reaproveitados, testes de parser verdes,
hook verde.

---

### Estágio 4 — Label de app (nome "Spotify") — OPCIONAL, polish

Só se valer o peso. Caminho: `pm list packages -f` dá o path do APK por pacote;
parsear o badging/manifest do APK com uma crate Rust de APK (avaliar
`apk`/`axmldecoder`/similar) para extrair `application-label`. Pull do APK via
SYNC. Em falha, manter o id como nome. Pinar a crate nova com `=`. Adicionar
testes de parsing do label com um APK-fixture pequeno (ou mockar o badging).

---

### Estágio 5 — Validação em device real: feature `e2e-android`

Espelha a feature `e2e` do iOS (compila no CI, **nunca** roda no CI).

1. `Cargo.toml`: adicionar `e2e-android = []` em `[features]`.
2. `tests/e2e_android.rs` (atrás de `#[cfg(feature = "e2e-android")]`): conectar
   com `device::connect(None, Some(Platform::Android))`, ler `status`/`info`,
   listar `apps` e **asserir que pelo menos um app tem `size_bytes > 0`**
   (prova que o `diskstats` funcionou no aparelho real), walk de mídia, logcat
   por alguns segundos. Testes tolerantes a "sem device" (skip com mensagem,
   como o `e2e_smoke.rs` do iOS faz).
3. Documentar no `README`/`CLAUDE.md` o comando:
   `cargo test --features e2e-android`.

**Matriz de aparelhos sugerida** (é onde os quirks de OEM aparecem): 1 Pixel
(AOSP puro), 1 Samsung (maior skin), idealmente 1 Xiaomi/MIUI (o mais
bagunçado). O `find -printf` e o formato do `dumpsys diskstats` são os primeiros
suspeitos de variação.

---

## 5. Ordem recomendada de execução

1. **Estágio 1** (sizes via diskstats) — entrega a feature, sem deps novas.
2. **Estágio 5** parcial — criar `e2e-android` já com o assert de `size_bytes>0`
   (mesmo que você não tenha device agora; fica pronto para rodar).
3. **Estágio 3** (migração `adb_client`) — só após o spike validar a API. Inclui
   o fix do walk (Estágio 2 some dentro dele).
4. **Estágio 4** (labels) — por último, se valer.

Se o tempo for curto, **Estágio 1 sozinho já entrega o valor de produto** (ver
tamanho dos apps para deletar). O resto é robustez/transporte/polish.

---

## 6. Itens que o implementador DEVE verificar (não assumir)

- **API real do `adb_client`** (nomes de método/trait, modo server, streaming de
  shell para logcat, delete via SYNC). A descrição aqui é inferida de docs —
  confirme em docs.rs/código da versão pinada antes de migrar.
- **Formato exato do `dumpsys diskstats`** no(s) aparelho(s) reais — os rótulos
  são AOSP, mas confirme separadores/colchetes e ajuste o parser/fixtures.
- **`reboot -p`** (shutdown) e `find -printf` em OEMs específicos — candidatos a
  variação; validar no Estágio 5.

## 7. Definição de pronto global

- `qk --platform android apps` lista apps **com tamanhos reais**, maior primeiro.
- Nenhum `if android`/`if ios` em `src/commands/`.
- Nenhum tipo de adb/`adb_client` acima de `src/device/android.rs`.
- Parsers puros com testes de fixture; hook (`fmt+clippy -D warnings+test`) verde.
- iOS intocado (rodar `cargo test --features e2e` com um iPhone continua passando).
- `e2e-android` existe e compila; documentado.
