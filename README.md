# Bamburino

An MCP bridge that lets AI agents (Claude Code, Codex, any MCP client) prepare prints inside a live Bambu Studio window on Windows. The user watches every change appear in the panels and on the plate, and can intervene with mouse and keyboard at any time.

Status: pre-alpha. See [Verification status](#verification-status) before relying on anything.

## What it is

You ask an agent: "infill 35 %, gyroid, 4 walls, 0.20 mm layers". The agent calls Bamburino, and the four values change in the Bambu Studio process panel that is already open. Studio takes an undo snapshot before each change (whether Ctrl+Z reverts a process-setting edit is `manual`, not yet checked; see Verification status). The same goes for rotating a model, switching supports on, slicing a plate and reading the time and filament summary.

What it does:

- Reads the open project: plates, objects, active presets, effective settings, slice state.
- Writes a fixed list of print settings, transforms objects, sets supports (project, plate or object scope).
- Slices a plate in Studio, reports progress, cancels, and reads the summary.
- Validates every setting key against a versioned catalog before writing, and detects conflicts with your manual edits through a global `revision`.

What it does not do:

- No simulated clicks, no editing of 3MF files behind Studio's back, no headless mode: it works in the window you have open.
- No printing and no printer actions of any kind. Starting a physical print stays with you.
- It does not silently change your global presets; changes go to the open project and saving is your decision.
- It does not start Bambu Studio for you.

## Why two pieces

```
 MCP client (Claude Code, Codex, ...)
        |  Streamable HTTP, bearer token (client token)
        v
 127.0.0.1:27600/mcp   (default listen_port)
 +--------------------------------------------+
 |  bamburino.exe  (Rust, Windows service)    |   the product
 |  12 tools, catalog validation, revision    |
 |  conflict check, timeouts, structured      |
 |  errors, version check                     |
 +--------------------------------------------+
        |  loopback HTTP, bearer token (driver token)
        v
 127.0.0.1:<driver port>   (Preferences > Local MCP server, default 27183)
 +--------------------------------------------+
 |  Bambu Studio v02.08.04.57 + driver patch  |   the driver
 |  (Local MCP server inside the Studio       |
 |  process; edits go through Studio's own    |
 |  undo stack and GUI)                       |
 +--------------------------------------------+
        |
        v   what you see: panels, plate, preview
```

**The driver lives inside Studio.** Only code running inside the Studio process can change the open project the way the GUI does (same undo stack, same panels). Studio is C++, so the driver is C++. It is the upstream pull request #12485 (`otarkhan/BambuStudio`, a native MCP server for Studio), frozen at commit `c31e971` and applied as `driver/patches/0001-pr12485-native-mcp-server.diff`, plus a handful of small patches (build identity, undo snapshot exposure, MSVC fixes). The driver is kept as small as possible on purpose: a fork with its own life is a non-goal.

**The service is the product.** Everything that can live outside Studio does: argument validation against the catalog (type, enum, range, scope), the global revision and conflict model, timeouts that never hang on a modal dialog, structured errors, the version check, authentication and the Windows service. It is a single Rust executable with no Python or Node runtime, and it keeps answering (status, catalog search) when Studio is closed.

See [docs/architecture.md](docs/architecture.md) for the details.

## Requirements

- Windows 10 or 11, x64.
- Disk space for the Studio build (a few hundred MB, the zip is about 260 MB) and the service. Building Studio from source is a different matter (about 20 GB and hours, see [docs/ci-runner.md](docs/ci-runner.md)); you do not need it to use Bamburino.
- No Python, Node or other runtime.
- An MCP client that speaks Streamable HTTP with custom headers.

## Install the Bambu Studio build

Bamburino needs its own build of Bambu Studio `v02.08.04.57` with the driver patch. The official build does not contain the driver.

1. Download the Studio zip from the GitHub release, `BambuStudio-bamburino-02.08.04.57-<patch_id>-win64.zip`, and its `.sha256` file. Check the hash (`Get-FileHash` in PowerShell, compare with the `.sha256` file).
2. Extract the zip anywhere (for example your Downloads folder). Open **Windows PowerShell** (`powershell`, version 5.1, present on every Windows 10/11; `pwsh` 7 also works) in the extracted folder and run the installer script that ships inside the zip, no administrator rights needed:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\install-studio.ps1
   ```

   `-ExecutionPolicy Bypass` is there because a script downloaded from the internet is otherwise blocked by default. Run from its own folder, the script installs from that folder to `%LOCALAPPDATA%\Programs\BambuStudio-Bamburino` and creates the Start Menu shortcut "Bambu Studio (Bamburino)". `-Uninstall` removes the folder and the shortcut. (The script also has a `-Zip <path>` option that extracts and checks the `.sha256` file next to the zip itself; it needs the script from outside the zip, so the path above is the one to follow.) The script parses cleanly under Windows PowerShell 5.1; a full install run under 5.1 is `not run`.
3. **SmartScreen.** The build is not code-signed, so Windows SmartScreen warns about an unknown publisher. This is expected: choose "More info", then "Run anyway". Code signing is outside the MVP.
4. **Shared profile.** Your settings, presets and login live in `%APPDATA%\BambuStudio`, shared with the official Bambu Studio (it is the same code). The installer never touches that folder.
5. **Do not keep the official Studio and the Bamburino build open at the same time.** Studio's single-instance check can make the two instances interfere.
6. Start "Bambu Studio (Bamburino)", open **Preferences > Local MCP server**, and switch it on. The panel shows the **port** (default 27183) and the **token**. You paste the token into the service in the next step. Treat it like a password.
7. If the server does not start on the default port, see the FAQ: some PCs reserve that TCP range (Hyper-V / WinNAT). Use another port, such as 27601 (an example, not a requirement), in the same Preferences panel.

## Install the Bamburino service

1. Download `bamburino-<version>-win64.zip` (and its `.sha256`) from the GitHub release and extract it. It contains `bamburino.exe`, `install.ps1`, `LICENSE`, `NOTICE.md` and `README.md`.
2. Open PowerShell **as Administrator** in the extracted folder and run:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\install.ps1
   ```

   It runs `bamburino.exe --install`: the exe is copied to `%ProgramFiles%\Bamburino\`, a Windows service `Bamburino` (display name "Bamburino MCP bridge", automatic delayed start) is registered under `NT AUTHORITY\LocalService`, and the configuration is created in `%ProgramData%\Bamburino\` (a client token is generated there; Documents and Downloads of the installing user go into `allowed_dirs`). The command is idempotent. If you elevate with an account other than your own, that account becomes the owner of the configuration.
3. As your **normal user** (not elevated), give the service the driver token shown in Studio. The token is read from standard input, so it never appears on a command line:

   ```powershell
   & "$env:ProgramFiles\Bamburino\bamburino.exe" config set-driver-token
   ```

   Paste the token and press Enter. The service picks it up without a restart.
4. If your Studio driver does not listen on the default port, set `driver_url` in `%ProgramData%\Bamburino\config.toml` (default `http://127.0.0.1:27183/mcp`) and restart the service.

Without a service (foreground, for a try-out): `bamburino.exe --run --config-dir <dir>` creates the config in `<dir>` if it is missing and stops with Ctrl+C. To remove the service, from an elevated shell: `bamburino.exe --uninstall` (add `--purge` to delete the configuration too). `bamburino.exe --version` prints the service version, the catalog version and the expected driver patch. Run `bamburino.exe --help` to see the commands of your version.

Keys in `config.toml`: `listen_port` (default 27600), `driver_url`, `read_timeout_ms`, `mutation_timeout_ms` (default and minimum 35000), `poll_interval_ms`, `allow_version_mismatch`, `allowed_dirs`, `log_retention_days`.

## Connect an MCP client

The endpoint is `http://127.0.0.1:27600/mcp` (if you kept `listen_port`), with the header `Authorization: Bearer <client token>`. The client token is a different secret from the driver token. The service prints a ready-made registration; this is the only command that prints the client token:

```powershell
& "$env:ProgramFiles\Bamburino\bamburino.exe" config client-snippet --client claude-code
```

For Claude Code the output is a line of this form:

```
claude mcp add --transport http bamburino http://127.0.0.1:27600/mcp --header "Authorization: Bearer <client token>"
```

Run that line once. For Codex and any other client use `--client generic`, which prints the URL and the header to enter in the client's MCP configuration. The Codex configuration is `not run`: it has not been tried yet.

The server also publishes the setting catalog as the resource `bamburino://catalog`.

## Tools

Twelve tools; input fields, result fields and errors per tool are in [docs/tools.md](docs/tools.md). Read tools never change anything. Mutation tools change the open project (or start a slice) and need Studio open with the right build.

| Tool | Kind | What it does |
|---|---|---|
| `bamburino_capabilities` | read | Bamburino, catalog and driver status. Works with Studio closed. |
| `studio_get_state` | read | The open project (plates, models, presets, slice state) and the `revision`. |
| `studio_get_settings` | read | Effective settings of the project, a plate or an object. |
| `studio_set_settings` | mutation | Sets settings by key; validated; `dry_run` available; needs `expected_revision`. |
| `studio_list_models` | read | Objects and instances of the project. |
| `studio_transform_model` | mutation | Rotates, translates or scales one object instance; needs `expected_revision`. |
| `studio_set_supports` | mutation | Support settings for the project, a plate or an object; needs `expected_revision`. |
| `studio_slice` | mutation | Starts slicing a plate; returns a `job_id`. Does not modify the project. |
| `studio_slice_status` | read | Progress of a slice job. |
| `studio_cancel_slice` | mutation | Cancels a running slice job. |
| `studio_get_slice_summary` | read | Time, filament and layers of the current slice result of a plate. |
| `catalog_search` | read | Finds setting keys by words of key, label or description. |

An example per tool. The JSON is the tool's input (the agent builds it from your sentence); outputs are described in prose and are not reproduced as verified samples.

**`bamburino_capabilities`** - "Is Bamburino connected to Studio?" Input: `{}`. Returns the service, catalog and driver status (Studio running or not, version match, token configured).

**`studio_get_state`** - "What is in the project?" Input: `{}`. Returns plates, models, active presets, slice state and `revision`, the number to pass as `expected_revision` in the next mutation.

**`studio_get_settings`** - "What is the current infill?" Input:
```json
{ "scope": "project" }
```
For `plate` add `"target": {"plate_id": 1}`; for `object` add `"target": {"object_id": "<id from studio_list_models>"}`. Returns the effective values for that scope.

**`studio_set_settings`** - "Infill 35 %, gyroid, 4 walls, 0.20 mm layers." Input:
```json
{
  "expected_revision": 123456,
  "changes": [
    {"key": "sparse_infill_density", "value": "35%"},
    {"key": "sparse_infill_pattern", "value": "gyroid"},
    {"key": "wall_loops", "value": 4},
    {"key": "layer_height", "value": 0.2}
  ]
}
```
`changes` holds 1 to 100 entries; each may carry `scope` (default `project`) and `target`. Add `"dry_run": true` to validate and see the diff without writing. Returns the values actually applied and the new `revision`.

**`studio_list_models`** - "Which objects are on the plate?" Input: `{}`. Returns objects and instances with ids, names, dimensions and transforms.

**`studio_transform_model`** - "Rotate the part 90 degrees around Z." Input:
```json
{
  "expected_revision": 123456,
  "object_id": "<id>",
  "transform": { "rotate_deg": [0, 0, 90], "mode": "relative" }
}
```
`transform` takes at least one of `rotate_deg` (degrees), `translate_mm`, `scale` (each `[x, y, z]`); `mode` is `relative` (default) or `absolute`; `instance_id` defaults to the first instance. Rotation angles read back are normalized (Z 270 reads as -90).

**`studio_set_supports`** - "Tree supports on this object." Input:
```json
{
  "expected_revision": 123456,
  "target": {"scope": "object", "object_id": "<id>"},
  "settings": {"enabled": true, "type": "tree", "style": "auto"}
}
```
`settings` also takes `threshold_angle` and `on_build_plate_only`. Scope `plate` needs `plate_id`.

**`studio_slice`** - "Slice the plate." Input: `{}` or `{"plate_id": 1}`. Returns a `job_id`; the Studio preview updates.

**`studio_slice_status`** - "How far is it?" Input: `{"job_id": "<id>"}`. Returns the job status and progress.

**`studio_cancel_slice`** - "Stop slicing." Input: `{"job_id": "<id>"}`.

**`studio_get_slice_summary`** - "How long, and how much filament?" Input: `{}` or `{"plate_id": 1}`. Returns time, filament and layers of the current slice result (the selected plate is the only one that can hold one).

**`catalog_search`** - "What is the key for the seam position?" Input:
```json
{ "query": "seam position", "limit": 5 }
```
`limit` defaults to 20, maximum 50. Returns matching keys with type, enum values, range, default and scopes.

### The revision flow

Every mutation carries the `revision` of the last read. If the project changed since (you edited by hand, pressed Ctrl+Z, saved), the call writes nothing and fails with `REVISION_CONFLICT` (status 409) and the current state in `details`. A typical exchange:

1. Agent: `studio_get_state` returns `revision` 100.
2. Agent: `studio_set_settings` with `expected_revision: 100` is applied and returns the new `revision`, say 101.
3. You change the infill to 20 % by hand in Studio. The revision moves on.
4. Agent retries with the old `expected_revision: 101`: `REVISION_CONFLICT`, current state attached. The agent re-reads, sees your 20 %, and decides again.

The numbers are illustrative; real revisions are large opaque integers. The conflict model and its known gaps are in [docs/architecture.md](docs/architecture.md).

### Errors

Every failure is `{"error": {"code", "message", "http_status", "details"}}`.

| Code | Meaning / what to do |
|---|---|
| `STUDIO_NOT_RUNNING` | Studio is not running. Start the Bamburino build of Bambu Studio. |
| `DRIVER_DISABLED` | Studio is open but Preferences > Local MCP server is off. Switch it on. |
| `DRIVER_UNAUTHORIZED` | The driver rejected the token. Run `config set-driver-token` with the token shown in Studio. |
| `DRIVER_TOKEN_MISSING` | No driver token configured yet. Run `config set-driver-token`. |
| `VERSION_MISMATCH` | The running Studio is not the build Bamburino expects; `details` has expected and found versions. |
| `BUSY` | Studio is busy (for example a modal dialog is open). Close the dialog and retry. |
| `REVISION_CONFLICT` | The project changed since your last read. Re-read and retry. |
| `UNKNOWN_KEY` | The key is not in the catalog. Use `catalog_search`. |
| `INVALID_VALUE` | The value fails the catalog's type, enum or range. |
| `SCOPE_NOT_ALLOWED` | The key cannot be set at that scope. |
| `NOT_WRITABLE` | The key is known but not writable. |
| `NOT_WRITABLE_BY_DRIVER` | The catalog knows the key, but the driver accepts only its fixed list (see Limits). |
| `OBJECT_NOT_FOUND`, `PLATE_NOT_FOUND`, `JOB_NOT_FOUND` | The id does not exist. Re-read with `studio_get_state` or `studio_list_models`. |
| `PARTIALLY_APPLIED` | Some changes were applied and a later one failed; `details` lists both. The applied part is in Studio's undo history. |
| `INVALID_ARGUMENT` | The input does not match the tool's schema. |
| `DRIVER_ERROR` | The driver returned an error or is unreachable; the message carries the cause. |

## Limits

The full list, with reasons, is in [docs/limitations.md](docs/limitations.md). In short:

- Windows x64 only; only Bambu Studio `v02.08.04.57`.
- **Live writes are limited to the 20 keys the driver accepts** (19 of them at plate or object scope; `print_sequence` is project only), even though the catalog describes 603 settings: `layer_height`, `initial_layer_print_height`, `wall_loops`, `top_shell_layers`, `bottom_shell_layers`, `sparse_infill_density`, `sparse_infill_pattern`, `enable_support`, `support_type`, `support_threshold_angle`, `support_on_build_plate_only`, `brim_type`, `brim_width`, `skirt_loops`, `outer_wall_speed`, `inner_wall_speed`, `sparse_infill_speed`, `travel_speed`, `print_sequence`, `seam_position`.
- No physical or printer actions. Bamburino does not start Studio.
- The revision is a fingerprint of the state, not a driver counter: a single manual edit during the write window of an agent mutation is indistinguishable from an undo jump and is not detected.
- Supports per plate are implemented as per-object overrides. No support painting, no lay-on-face, no variants, no filament presets, no multi-plate workflows.
- No import or export tool: the path rules exist in the service but no tool takes a path yet.
- Print time comes from Studio's estimate string; filament grams and meters come from parsing the header of a temporary exported G-code.
- Unsigned installers; unofficial build of Bambu Studio.

## Verification status

Vocabulary: `verified | failed | not run | manual`. Nothing is called verified unless it was executed and its result recorded.

<!-- verification:start -->
Not generated yet — see docs/verification.md after the first verification run (Task 5).
<!-- verification:end -->

## Security

- **Loopback only.** The service binds `127.0.0.1`; the driver also listens on loopback only.
- **Bearer tokens on both layers.** The client token (generated at install) and the driver token (from Studio) are separate. The client token is compared in constant time. Tokens are not logged and not passed as command-line arguments; the driver token is read from stdin.
- **Host and Origin checks** against DNS rebinding and browser cross-origin requests.
- **ACLs.** `%ProgramData%\Bamburino\` has a protected DACL: SYSTEM and Administrators full control, the service account read (modify on `logs\`), the installing user read or modify.
- **Path allowlist.** `allowed_dirs` defaults to Documents and Downloads of the installing user; the path rules reject traversal, UNC, device names, alternate data streams, symlinks and reparse points.
- **No shell, no physical actions.** No command is exposed, and no physical action exists in this version.
- The service runs as `NT AUTHORITY\LocalService`. The CI self-hosted runner is on demand and takes maintainer pushes only: workflows never trigger on pull requests; a fork PR run must be approved by a maintainer — never approve a run that touches `.github/` ([docs/ci-runner.md](docs/ci-runner.md)).

## License and attributions

Bamburino is licensed under the GNU Affero General Public License v3.0 (see [LICENSE](LICENSE)). [NOTICE.md](NOTICE.md) lists the reused components and their copyrights:

- Bambu Studio (Bambu Lab) and PrusaSlicer (Prusa Research), upstream of the driver.
- Pull request #12485 by otarkhan (`otarkhan/BambuStudio`), the source of the driver patch.
- `subhashdasyam/bambustudio-mcp`, source of the catalog generator.
- `Ding-Ding-Projects/BambuStudio`, source of the path-handling rules (rewritten in Rust).

No code from `DMontgomery40/bambu-printer-mcp` (GPL-2.0) is included.

## Contributing and upstream

The driver is the upstream pull request #12485, not a fork with its own life. Our extra patches are small, and each one states whether we propose it upstream. Each patch is listed with its reason and with whether we propose it upstream in `docs/driver-divergences.md` (on the driver branch; links into `driver/` and `catalog/` resolve after that branch is merged). Issues and pull requests are welcome; run `pwsh ci/check-repo.ps1` before sending one.

## FAQ and troubleshooting

**Error 10013 when the service or the driver binds a port.** Windows (Hyper-V / WinNAT) can reserve whole TCP ranges. List them with `netsh int ipv4 show excludedportrange protocol=tcp`. If your port is inside a range, change it: `listen_port` in `%ProgramData%\Bamburino\config.toml` for the service, the port in Studio's Preferences > Local MCP server for the driver (27601 is only an example; any free port outside the reserved ranges will do), then update `driver_url` to match and restart the service. `--install` and `config init` check this and say so.

**SmartScreen blocks the installer.** The build is unsigned. "More info", then "Run anyway".

**`STUDIO_NOT_RUNNING`.** Start "Bambu Studio (Bamburino)". Bamburino never starts Studio itself, and the official Studio does not contain the driver.

**`DRIVER_DISABLED`.** Switch on Preferences > Local MCP server.

**`DRIVER_UNAUTHORIZED` / `DRIVER_TOKEN_MISSING`.** Copy the token from Studio's Preferences and run `config set-driver-token` again. No restart is needed.

**`VERSION_MISMATCH`.** The Studio you run is not the build this Bamburino expects (`v02.08.04.57` plus the matching patch). Install the Studio zip from the same release as the service. `allow_version_mismatch` in `config.toml` is an override at your own risk.

**`BUSY`.** A modal dialog is open in Studio, or another operation is running. Close it and retry; calls return `BUSY` within the timeout instead of hanging.

**Two Studios open.** Close the official one; the single-instance check can make them interfere.

**Where are the logs?** `%ProgramData%\Bamburino\logs`.
