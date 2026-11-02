# MapsOffline – size budget

Goal: the smallest installer / executable we can get **without slowing routing, search
or features**. Measured on Windows 11 x64, 2026-11-01, Tauri 2.12, Vite 8.3.5 (rolldown + oxc),
rustc stable. Map/routing/geocode/transit data is downloaded separately and is not part of
the installer.

## Result

| Artifact | Before | After | Change |
|---|---:|---:|---:|
| NSIS installer (`bundle/nsis/*-setup.exe`) | 11.58 MB | **4.87 MB** | −58 % |
| MSI installer | built by default (failed here: WiX) | not built | – |
| `mapsoffline.exe` (production, assets embedded) | 17.29 MB | **10.24 MB** | −41 % |
| `mapsoffline-data.exe` shipped inside installer | 7.20 MB | **not shipped** | −100 % |
| `dist/` total | 13.58 MB | **5.84 MB** | −57 % |
| `dist/fonts` | 11.48 MB (4 stacks × 256 ranges) | 4.01 MB (3 stacks × 15 ranges) | −65 % |
| `dist/sprites` | 150 KB (v3 + v4, 5 flavors) | 104 KB (v4 light + dark) | −31 % |
| `dist/rtl-text.js` | 427 KB | 207 KB (minified build) | −52 % |
| JS (all chunks) | 1 414 KB (1 chunk) | 1 420 KB (6 chunks) | ≈ 0 |

"Before" exe/installer were built from the original `Cargo.toml` / `tauri.conf.json` / `dist`
via `tauri build`; "after" is the real `pnpm tauri build` from the repo.
Note: a plain `cargo build --release` is a *dev-mode* binary (no `tauri/custom-protocol`, no
embedded assets) – always compare `tauri build` output.

## Changes and their effect

### 1. Data CLI no longer bundled (7.2 MB uncompressed removed from the installer payload)
The Tauri bundler packs **every `[[bin]]`** of the crate. `mapsoffline-data.exe` (7.2 MB) was
being installed next to the app. It is now gated:

```toml
[[bin]]
name = "mapsoffline-data"
required-features = ["cli"]
[features]
cli = ["dep:clap"]       # clap is now optional, only compiled for the CLI
```

`tauri build` runs `cargo build --bins --features tauri/custom-protocol --release`, which skips
the CLI. **Developer commands change:**

```
cargo run --release --features cli --bin mapsoffline-data -- build --all
cargo run --release --features cli --bin mapsoffline-data -- bench
cargo check --all-targets --features cli      # to also type-check the CLI
```
(Without `--features cli` cargo prints "requires the features: `cli`".)

### 2. Release profile (exe −1.8 MB at equal speed)
```toml
[profile.release]
codegen-units = 1
lto = "fat"
opt-level = "s"
panic = "abort"
strip = true
debug = false
incremental = false
[profile.release.package.libsqlite3-sys]  opt-level = 3
[profile.release.package.rusqlite]        opt-level = 3
```

Measured on a frozen source snapshot (same code for all variants), trimmed deps, production
build. Bench = `mapsoffline-data bench --iters 5`, 3 runs each, medians:

| Variant | app exe | CLI exe | car TLV–JLM ×3 alt | car Eilat–KS ×3 | search worst ("ב") | search "חיפה" | graph build (PBF→graph.bin) |
|---|---:|---:|---:|---:|---:|---:|---:|
| opt 3 (old) | 12.07 MB | 5.97 MB | 36.9 ms | 60.2 ms | 3.04 ms | 1.27 ms | 4.25 s |
| opt "s" everywhere | 9.69 MB | 4.75 MB | 36.0 ms | 58.3 ms | **4.47 ms (+47 %)** | 1.57 ms (+24 %) | – |
| opt "z" everywhere | 8.91 MB | 4.45 MB | **47.6 ms (+29 %)** | – | 4.51 ms | 1.62 ms | – |
| deps "s", app + hot crates 3 | 11.63 MB | 5.67 MB | 37.2 ms | 60.7 ms | 3.07 ms | 1.31 ms | – |
| **"s" + SQLite at 3 (chosen)** | **10.25 MB** | 5.30 MB | 36.1 ms | 58.9 ms | 3.15 ms (+3 %) | 1.36 ms | 4.2–4.7 s (noise) |

* Routing (A*, alternatives, graph load ≈ 31 ms) is not slower at `-Os`; `"z"` is (rejected).
* The FTS5 search slowdown at `"s"` came entirely from SQLite's C code; keeping
  `libsqlite3-sys` + `rusqlite` at 3 restores it (within noise).
* Keeping only the "hot" Rust deps at 3 saves almost nothing, because generic code is
  monomorphised into our own crate.

### 3. Dependency trimming (`src-tauri/Cargo.toml`)
| Change | Why |
|---|---|
| removed `regex` | not used in `src-tauri/src` (tauri-utils still pulls it in transitively) |
| removed `thiserror` | not used |
| removed `futures-util` | not used (downloads use `Response::chunk()`) |
| `reqwest`: dropped `stream` feature | `bytes_stream`/`wrap_stream` unused |
| `tokio`: `full` → `rt`, `time` | only `Builder::new_current_thread().enable_all()` + `time::timeout` |
| `chrono`: no default features → `std`, `clock`, `serde` | drops `oldtime`, `wasmbind` |
| `env_logger`: no defaults → `humantime` | drops `regex` filters and ANSI auto-colour (`RUST_LOG=info`, `module=debug` still work) |
| `clap`: optional (CLI only), `std, derive, help, usage, error-context` | not compiled into the app at all |
| `zip`: kept `deflate` only | `deflate-flate2` alone does not enable the flate2 dep in zip 2.6; zopfli is only reachable from `ZipWriter` (tests) and is removed by LTO |

Kept (used): `rayon` (GTFS build, transit), `rstar`, `smallvec`, `rustc-hash`, `parking_lot`,
`osmpbf`, `csv`, `rusqlite/bundled`, `unicode-normalization`, `chrono-tz`, `dirs`, `bincode`.
Tauri default features are all needed on Windows (`compression` = brotli-compressed embedded
assets, `common-controls-v6`, `dynamic-acl`); `protocol-asset` kept.

### 4. Map assets (`public/`)
* **Font stacks**: the Protomaps basemap (`light`, `dark`, `grayscale` flavors, he/en/ar) only
  references `Noto Sans Regular`, `Noto Sans Medium`, `Noto Sans Italic`; app layers use
  `Noto Sans Regular`. `Noto Sans Devanagari Regular v1` removed.
* **Glyph ranges** kept per stack (15 of 256):
  `0-255, 256-511, 512-767, 768-1023` (Latin, Latin-ext, IPA, Greek),
  `1024-1279` (Cyrillic), `1280-1535` (Hebrew), `1536-1791` (Arabic),
  `7680-7935` (Latin Extended Additional – transliterations),
  `8192-8447, 8448-8703` (punctuation, currency, letter-like, arrows, ₪),
  `64256-64511` (Hebrew presentation forms), `64512-64767, 64768-65023, 65024-65279`
  (Arabic presentation forms A/B – what the RTL shaper emits), `65280-65535` (half/full-width forms).
  Safe because MapLibre 5.24 **draws a glyph locally with TinySDF (system font) when its range
  fails to load** (`glyph_manager.ts` → `_drawGlyph`, warns once) – a rare Thai/Ge'ez/etc.
  character still renders, it is never dropped and the tile still loads. CJK is always drawn
  locally by MapLibre, so CJK ranges were never fetched anyway.
* **Sprites**: v3 removed (style uses `sprites/v4`); in v4 only `light` and `dark` are referenced
  (`mapStyle.ts`: dark → `dark`, everything else → `light`), so `black`, `white`, `grayscale` removed.
* **`rtl-text.js`** replaced by the package's own minified build
  (`@mapbox/mapbox-gl-rtl-text@0.2.3/mapbox-gl-rtl-text.min.js`, identical code, registers via
  `self.registerRTLTextPlugin`). Re-sync with `pnpm rtl:sync`.
* `public/vite.svg` removed (unused; `index.html` uses `tauri.svg`).

To add a font range back: copy `<range>.pbf` from the Protomaps `basemaps-assets` repo into each
of the three `public/fonts/Noto Sans */` folders.

### 5. Frontend build (`vite.config.ts`)
* `target: "chrome105"` always (WebView2), `sourcemap` only for debug builds,
  `minify`/`cssMinify` (oxc / lightningcss), `modulePreload.polyfill: false`,
  `reportCompressedSize: false` (faster builds).
* `rolldownOptions.output.codeSplitting.groups`: `maplibre` (1 028 KB), `map-vendor`
  (pmtiles + @protomaps/basemaps, 57 KB), `react` (react, react-dom, scheduler, zustand, 219 KB),
  app `index` (≈ 116 KB). No size change (all assets are embedded either way); the app chunk is
  now small and the vendor chunks are stable between releases.
* `lucide-react` uses named imports only (tree-shaken); `@protomaps/basemaps` (inside the 57 KB map-vendor chunk) carries all
  flavors – flavors are plain objects, not worth splitting.

### 6. Bundling (`tauri.conf.json`)
* `bundle.targets`: `["nsis"]` (MSI dropped – needs WiX, and duplicates the installer).
* `nsis.compression: "lzma"` (explicit; best ratio).
* `windows.webviewInstallMode: downloadBootstrapper` (silent) – WebView2 ships with Windows 11;
  nothing embedded.
* Icon list trimmed to what Windows uses (`icon.ico` + PNGs); `icon.icns` (macOS) no longer listed.
* `app.withGlobalTauri: false` (explicit). No `bundle.resources`.

## Recommendations that need source edits (not done – other owners)

1. **chrono-tz: compile only `Asia/Jerusalem`** (est. −150…250 KB exe). The code only uses
   `chrono_tz::Asia::Jerusalem` / `Tz`. Enable `chrono-tz = { version = "0.10", features =
   ["filter-by-regex"] }` and add `src-tauri/.cargo/config.toml`:
   ```toml
   [env]
   CHRONO_TZ_TIMEZONE_FILTER = "(Asia/Jerusalem|UTC)"
   ```
   (Check no code parses timezone names at runtime – currently none does.)
2. **Docs/commands**: `.claude/skills/mapsoffline/SKILL.md` and any agent prompts must use
   `--features cli` for `mapsoffline-data` (see §1).
3. **NSIS Hebrew strings**: Tauri has no Hebrew translation of its custom installer strings
   (makensis warns `LangString ... not set in language table of language Hebrew`; they fall back
   to English). Provide `bundle.windows.nsis.customLanguageFiles` with a Hebrew `.nsh` if the
   installer UI should be fully Hebrew.
4. `dayjs` + `relativeTime` (≈ 8 KB) could be replaced by `Intl.RelativeTimeFormat` – tiny gain,
   optional.
5. `src-tauri/icons/android`, `icons/ios`, `Square*Logo.png`, `icon.icns` are not used by the
   Windows bundle (repo size only, not installer size); can be deleted if mobile/macOS/Store
   targets are not planned.

## How to re-measure
```
pnpm tauri build                       # installer: src-tauri/target/release/bundle/nsis/*.exe
cd src-tauri
cargo build --release --features tauri/custom-protocol,cli --bins
target/release/mapsoffline-data.exe bench --iters 5
```
