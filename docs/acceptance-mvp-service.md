<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
# MVP acceptance: Bamburino service against the real Bambu Studio driver

Outcome vocabulary: `verified | failed | not run | manual`. A step is `verified` only after it was executed and its
result checked.

Steps 2, 3, 5, 6, 7 and 8 are driven by `pwsh tools/demo.ps1`, which rewrites the matching row (observed, result,
date). The other rows are filled by hand. Step numbers follow section 9 of the design; `4b` is Ctrl+Z in the GUI after
`studio_transform_model` (the previous rotation comes back), `4c` is Ctrl+Z in the GUI after `studio_set_supports`
with scope `object` (the supports go away).

Run it on the build PC: patched Studio with "Local MCP server" on (driver port 27601 there, see
`docs/acceptance-mvp.md`), `driver_url = "http://127.0.0.1:27601/mcp"` in `%ProgramData%\Bamburino\config.toml`,
the service installed and the driver token saved with `bamburino config set-driver-token`.

Run of 2026-10-28 (elevated Claude session, `docs/superpowers/handoff-elevated-session.md`): Studio from the published
`v0.1.0-rc2` zip, installed with its `install-studio.ps1`, launched from the elevated prompt with a 20 mm cube STL;
`mcp_server_enabled` and `mcp_server_port = 27601` were written into `BambuStudio.conf` (MD5 line recomputed) instead
of through Preferences; service `bamburino.exe` 0.1.0 from `C:\cargo-target\bamburino-main\release`, installed with
elease`, installed with
`--install`; `bamburino_capabilities` reported `ready`, `version_match = true`, patch `2703638ef313`. The steps were
run by a runner that reuses `tools/Demo.psm1`; the keyboard steps (4, 4b, 4c, 10) were sent with `SendKeys` to the
Studio window, nobody touched the GUI. Rows marked `manual` still need the hands-on check.

| step | action | expected | observed | result | date |
|---|---|---|---|---|---|
| 1 | Open Studio, Local MCP server on, load the test cube | Studio ready, project with one object | Studio 02.08.04.57 `bamburino.2703638ef313` launched with `test-cube-20mm.stl`; port 27601 listening, `mcp.token` created; `studio_get_state` saw 1 plate, 1 object | verified | 2026-10-28 |
| 2 | `studio_get_state` | project, plate, model, presets and a revision | project 'Untitled', 1 plate(s), 1 object(s), process '0.12mm High Quality @BBL X1C', revision 1880294686720001 | verified | 2026-10-28 |
| 3 | `studio_set_settings`: infill 35 %, gyroid, 4 walls, layer 0.20 | four values applied and visible in the process panel, new revision | infill 35%, pattern gyroid, walls 4, layer 0.2, new revision 1880294686720002 | verified | 2026-10-28 |
| 4 | Ctrl+Z in the GUI after step 3 | the values go back; `studio_get_state` shows a new revision | Ctrl+Z sent with SendKeys after step 3: infill 35%, gyroid, walls 4, layer 0.2 stayed as they were (before step 3: 15%, gyroid, 2, 0.12), revision unchanged 1880294686720002. Keystrokes do reach Studio (Ctrl+P opened Preferences in step 10), but the canvas focus is not guaranteed, so this does not separate 'no undo snapshot for the preset config' from 'Ctrl+Z went to another control': check by hand on the canvas | manual | 2026-10-28 |
| 4b | Ctrl+Z in the GUI after `studio_transform_model` | the previous rotation comes back | Ctrl+Z sent with SendKeys after step 6: rotation_deg stayed [0,-0,90], revision unchanged 1880294686720005 (same caveat as step 4). Minor: the driver reports `-0` for the Y angle | manual | 2026-10-28 |
| 4c | Ctrl+Z in the GUI after `studio_set_supports` (scope `object`) | the supports go away | Ctrl+Z sent with SendKeys after step 7 (scope `project`, the `object` variant was not run): enable_support stayed 1, support_type tree(auto), revision unchanged 1880294686720006 (same caveat as step 4) | manual | 2026-10-28 |
| 5 | Change infill to 20 % by hand, retry with the old revision | 409 REVISION_CONFLICT with the current state; retry with the new revision succeeds | API variant (infill set to 20 % through studio_set_settings, not by hand; the by-hand variant was not run: no GUI control in this session): old revision 1880294686720002 -> 409 REVISION_CONFLICT (current 1880294686720003); retry ok, infill before the write 20%, applied 35%, revision 1880294686720004 | manual | 2026-10-28 |
| 6 | `studio_transform_model`: rotate 90 degrees about Z | the model rotates on the plate; Z reads back +90 (normalized) | object 74f92376-89ce-40c7-b9a4-92347b0e55c6:62: Z 0 -> 90 (re-read 90; angles are normalized to -180..180), revision 1880294686720005 | verified | 2026-10-28 |
| 7 | `studio_set_supports`: tree supports on | tree supports visible in the panel | enable_support 1, support_type tree(auto), revision 1880294686720006 | verified | 2026-10-28 |
| 8 | `studio_slice`, poll `studio_slice_status`, `studio_get_slice_summary` | job succeeds, preview updates, summary has print time and grams | job 74f92376-89ce-40c7-b9a4-92347b0e55c6:slice:1 succeeded after 2 poll(s); print time 25.9 min (1554 s), grams 6.07 | verified | 2026-10-28 |
| 9 | Close Studio, call every GUI tool, `bamburino_capabilities`, `catalog_search` | structured errors for the GUI tools; the other two work | Studio closed (Stop-Process). GUI tools: studio_get_state=STUDIO_NOT_RUNNING, studio_get_settings=STUDIO_NOT_RUNNING, studio_set_settings=STUDIO_NOT_RUNNING, studio_list_models=STUDIO_NOT_RUNNING, studio_transform_model=STUDIO_NOT_RUNNING, studio_set_supports=STUDIO_NOT_RUNNING, studio_slice=STUDIO_NOT_RUNNING, studio_slice_status=STUDIO_NOT_RUNNING, studio_cancel_slice=STUDIO_NOT_RUNNING, studio_get_slice_summary=STUDIO_NOT_RUNNING; capabilities driver.status=not_running; catalog_search 'infill' -> 20 hit(s) | verified | 2026-10-28 |
| 10 | Open a modal dialog in Studio, call `studio_set_settings` | `busy` within the timeout, the call does not hang | Preferences opened with Ctrl+P (SendKeys); get_state busy.modal_open=True; studio_set_settings -> error BUSY: Bambu Studio is busy (modal_open); nothing was changed. in 0 s; Esc sent | verified | 2026-10-28 |
