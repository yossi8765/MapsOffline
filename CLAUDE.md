# MapsOffline

Offline navigation and maps desktop app for Israel. Tauri 2 + Rust backend, React/TypeScript + MapLibre frontend.

- Spec (Hebrew, authoritative): `docs/SPEC.md`
- Project skill with architecture, IPC contract, commands and delegation policy: `.claude/skills/mapsoffline/SKILL.md` – load it before working on this repo.
- Quick commands: `pnpm tauri dev` (run), `cd src-tauri && cargo check` (Rust check), `pnpm tsc --noEmit` (TS check).
- UI is Hebrew / RTL by default. Coordinates are `[lon, lat]`.
- Runtime works fully offline. Network is used only in the data-update flow (`src-tauri/src/data/`) and in hybrid routing (`src-tauri/src/online/`: OSRM + Transitous tried first with short timeouts, local engines as fallback).
