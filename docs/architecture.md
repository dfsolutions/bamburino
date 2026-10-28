# Architecture

Bamburino has two pieces: the **driver** (a patched Bambu Studio, C++, runs inside the Studio process) and the **service** (`bamburino.exe`, Rust, a Windows service). An MCP client talks only to the service.

```
 MCP client --(HTTP /mcp, client bearer)--> bamburino.exe --(loopback HTTP, driver bearer)--> Bambu Studio v02.08.04.57 + driver patch
                                            127.0.0.1:27600                                    127.0.0.1:<driver port, default 27183>
```

## The driver

- Source: upstream pull request #12485 (`otarkhan/BambuStudio`), frozen at commit `c31e971` and stored as `driver/patches/0001-pr12485-native-mcp-server.diff`. The pull request can be rewritten by its author; our copy cannot.
- Applied on the pinned tag in `driver/upstream.toml` (Bambu Studio `v02.08.04.57`) by `driver/build.ps1 -Step patch`, in ordinal order of `driver/patches/*.diff`. The set is identified by a `patch_id` (also in `driver/upstream.toml`), recomputed at build time; a mismatch stops the build.
- Our additional patches, each with its reason: `0100` build identity (`app.get_capabilities` reports `buildSuffix` and `patchId`, so the service can tell this build from any other), `0120` undo snapshot exposure (`app.get_state` reports `undoSnapshot`), `0200`, `0210`, `0220` Windows/MSVC build fixes for code that upstream only built on macOS.
- The driver listens on `127.0.0.1` on the port set in Preferences > Local MCP server (default 27183), protected by a bearer token shown in the same panel.
- Build steps (`fetch`, `patch`, `deps`, `app`, `test`, `package`, `all`) and their exit codes are documented in the `.DESCRIPTION` of `driver/build.ps1`.

## The service

- One executable, Rust, no external runtime. Runs as the service `Bamburino` under `NT AUTHORITY\LocalService`, or in the foreground with `--run`.
- Endpoint: Streamable HTTP at `/mcp` on `127.0.0.1:<listen_port>` (default 27600), plus `/healthz`. Every request passes a Host/Origin check and a constant-time bearer check.
- Exposes 12 tools and the catalog resource `bamburino://catalog`.
- Configuration: `%ProgramData%\Bamburino\config.toml`, the client token and the driver token in the same folder (ACL-protected), logs in `logs\`, temporary files in `tmp\`.

### Catalog and validation

`catalog/catalog.json` describes the print settings of the pinned Studio version (603 entries at the time of writing: type, scopes, label, default, enum values). It is generated from Studio's sources by `catalog/build_catalog.py`, never edited by hand. Before any write, the service validates the key (exists, writable, allowed at the requested scope), the value (type, enum, range). Validation happens before the driver is called, so a bad request never touches Studio.

The catalog describes more than the driver can write. The driver accepts only a fixed list: 20 keys for the project, 19 of them (all but `print_sequence`) for plate or object overrides. A key that is in the catalog but not on that list is rejected with `NOT_WRITABLE_BY_DRIVER`.

### Driver mapping (summary)

| Service operation | Driver tool |
|---|---|
| state | `app_get_state`, `plate_list`, `model_list`, `preset_list` (printer, process, filament) |
| capabilities / identity | `app_get_capabilities` |
| read settings | `settings_effective` |
| write project settings | `settings_update` |
| write plate or object settings (and plate supports) | `settings_override` |
| transform | `object_transform` |
| plate selection | `plate_select` |
| slice | `slice_start`, `slice_get_state`, `job_get`, `job_cancel` |
| summary | `slice_get_state` (estimated time), `preview_get_summary` (move counts, warnings), `export_gcode` to a temporary file whose header gives grams, meters and volume |

The full table with arguments is in the module documentation of `service/crates/bamburino-core/src/driver/map.rs`.

## Revision and conflict model

Goal: never overwrite what you just did by hand.

- The service keeps one global **revision** of the project state. It is bumped when a **fingerprint** of what it reads from the driver changes. The fingerprint hashes the app state (including the dirty flag and project path), active presets, models and instance transforms, plates, and the effective settings of the project, of every object and of every plate.
- When the driver reports `undoSnapshot` (patch `0120`), it is hashed in and is the primary signal: every user edit that takes an undo snapshot, and every undo and redo, changes it. When the field is absent, the content hash alone is used.
- A revision value is built from the service start time and a sequence number, so a restart never reuses revisions and a stale `expected_revision` from before the restart cannot match.
- While MCP clients are connected, the service polls the driver (default every 1500 ms, `poll_interval_ms`) so that manual edits are noticed between agent calls.
- Cost: a refresh makes a fixed number of reads plus one `settings_effective` call per object and per plate, so it grows with their number.

### Mutation pipeline

Every mutating tool (`studio_set_settings`, `studio_transform_model`, `studio_set_supports`) runs under one mutex, in this order:

1. Studio is ready, and for writes it is the pinned build (else `VERSION_MISMATCH`).
2. No modal dialog, UI job or slice is running (else `BUSY`, with no write attempted).
3. Refresh the revision and compare with `expected_revision` (else `REVISION_CONFLICT` with the current state).
4. Run the mutation against the driver.
5. Refresh immediately, so the agent's own change is not mistaken for a user edit later.
6. **Snapshot rule:** when the driver reports `undoSnapshot`, it must have advanced by exactly one snapshot per successful mutating driver call. Otherwise something else edited the project while the mutation ran and the call returns `REVISION_CONFLICT`.

The mutation timeout (`mutation_timeout_ms`, default and minimum 35000) is longer than the driver's own 30 s queue, so a modal dialog yields a structured `BUSY` instead of a hung call.

### Known gaps (accepted for the MVP)

- A manual edit made between the pre-mutation refresh and the mutation itself is not seen, so the agent may overwrite it. Polling narrows this gap, it cannot close it.
- A user edit landing between the agent's mutation and the post-mutation refresh is absorbed into the agent's own revision bump.
- A single manual edit inside a write window is indistinguishable from an undo jump, and is not detected by the snapshot rule.
- Without `undoSnapshot`, an edit that leaves every hashed value identical is invisible.

## Version handling

The service is built for one Studio version. `bamburino_capabilities` and the write path compare the base version and the `patchId` reported by the driver with the ones compiled into the service. A mismatch stops writes (`VERSION_MISMATCH`) unless `allow_version_mismatch` is set. Updating Studio is a deliberate act: change the tag in `driver/upstream.toml`, rebuild, regenerate the catalog (the generator command is in `catalog/README.md`), run the acceptance checks.
