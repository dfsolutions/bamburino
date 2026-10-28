<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# MVP acceptance: Bamburino service against the real Bambu Studio driver

Outcome vocabulary: `verified | failed | not run | manual`. A step is `verified` only after it was executed and its
result checked. Nothing in this file has been run yet: every row is `not run` (or `manual` for the steps that need
hands on Studio) until the owner runs it.

Steps 2, 3, 5, 6, 7 and 8 are driven by `pwsh tools/demo.ps1`, which rewrites the matching row (observed, result,
date). The other rows are filled by hand. Step numbers follow section 9 of the design; `4b` is Ctrl+Z in the GUI after
`studio_transform_model` (the previous rotation comes back), `4c` is Ctrl+Z in the GUI after `studio_set_supports`
with scope `object` (the supports go away).

Run it on the build PC: patched Studio with "Local MCP server" on (driver port 27601 there, see
`docs/acceptance-mvp.md`), `driver_url = "http://127.0.0.1:27601/mcp"` in `%ProgramData%\Bamburino\config.toml`,
the service installed and the driver token saved with `bamburino config set-driver-token`.

| step | action | expected | observed | result | date |
|---|---|---|---|---|---|
| 1 | Open Studio, Local MCP server on, load the test cube | Studio ready, project with one object | not run | not run | - |
| 2 | `studio_get_state` | project, plate, model, presets and a revision | not run | not run | - |
| 3 | `studio_set_settings`: infill 35 %, gyroid, 4 walls, layer 0.20 | four values applied and visible in the process panel, new revision | not run | not run | - |
| 4 | Ctrl+Z in the GUI after step 3 | the values go back; `studio_get_state` shows a new revision | not run | manual | - |
| 4b | Ctrl+Z in the GUI after `studio_transform_model` | the previous rotation comes back | not run | manual | - |
| 4c | Ctrl+Z in the GUI after `studio_set_supports` (scope `object`) | the supports go away | not run | manual | - |
| 5 | Change infill to 20 % by hand, retry with the old revision | 409 REVISION_CONFLICT with the current state; retry with the new revision succeeds | not run | not run | - |
| 6 | `studio_transform_model`: rotate 90 degrees about Z | the model rotates on the plate; Z reads back +90 (normalized) | not run | not run | - |
| 7 | `studio_set_supports`: tree supports on | tree supports visible in the panel | not run | not run | - |
| 8 | `studio_slice`, poll `studio_slice_status`, `studio_get_slice_summary` | job succeeds, preview updates, summary has print time and grams | not run | not run | - |
| 9 | Close Studio, call every GUI tool, `bamburino_capabilities`, `catalog_search` | structured errors for the GUI tools; the other two work | not run | not run | - |
| 10 | Open a modal dialog in Studio, call `studio_set_settings` | `busy` within the timeout, the call does not hang | not run | manual | - |
