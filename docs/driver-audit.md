# Driver code audit: PR #12485 native MCP server

Audited code: `bambulab/BambuStudio` PR #12485 by otarkhan, head `c31e97108e4d1ea137f90a075502e4d4536535fe`, diffed against
`da8b44ee34dd349f2ae0df3f1cbae366df482354` and frozen in `driver/patches/0001-pr12485-native-mcp-server.diff`
(41 files, +12995/-108, `patch_id` `1c5c9e30ce52`). The patch applies cleanly on tag `v02.08.04.57` (`f977235e`).
Line numbers refer to the PR files at `c31e971`. Unless a path is given, `McpTools.cpp`, `McpAdvanced.inc`, `McpBatch.inc`,
`McpDevices.inc`, `McpPreview.inc`, `Plater.cpp` and `GUI_ObjectList.cpp` are in `src/slic3r/GUI/`, and `McpServer.cpp` and `UndoRedo.cpp` are in `src/slic3r/Utils/`.

## Tool table

One row for each of the 90 tools in `src/slic3r/GUI/McpToolsManifest.json`. The first column is the MCP tool name; `method` is the native method the tool dispatches to.

How the columns were filled:
- **live GUI**: `yes` means the change shows up in the open Bambu Studio window without a reload. Either the handler calls `refresh`, `advanced_refresh`, `on_config_change`, `load_config`, `set_plater_dirty`, `update_project_dirty_from_presets` or `notify_instance_update`, or it calls a native Plater or ObjectList entry point that updates the window (`update()`, `object_list_changed()`, a canvas render, `update_after_undo_redo`). `no` means nothing visible changes in the project view. Every read is `no`.
- **take_snapshot**: `yes` means an undo snapshot is taken before the first change to the model or presets, either by the handler or by the native function it calls. `no` means the handler changes the project without taking an undo snapshot. `n/a` means the tool does not change the project model or presets: reads, selection, view, slicing, exports, devices, account and preferences. `project_new` and `project_open` show `no` because they only take a `ProjectSeparator` snapshot (`Plater.cpp:20320`, `:20515`), which clears the undo history and gives the user nothing to undo. `history_undo` and `history_redo` show `no` because they are the undo and redo themselves.
- **revision**: `yes` means the tool checks a revision the caller supplied. That is `expectedPlateRevision`, through `check_plate_revision`, `current_slice_plate`, `mcp_preview_result`, `mcp_upload_plan` or `mcp_print_plan`, or `meshRevision`, through `advanced_check_mesh`. A revision that is issued but never checked counts as `no`. Two checks are conditional: `settings_effective` checks only when `plateIndex` is passed (`McpTools.cpp:389`), and `settings_override` checks only for `scope=plate` (`:810`).
- **Windows**: allowed values are `not run`, `verified (<yyyy-mm-dd>)` and `failed (<yyyy-mm-dd>)`. The column stays `not run` until Task 9 runs the tools on Windows.
- **Bamburino tool**: the Bamburino MCP tool that uses this driver tool, plus the plan B `DriverClient` method in brackets. `-` means the tool is outside the plan B allowlist, so Bamburino does not call it.

Provenance: two extractions were made independently, then merged. Of the 270 cells in the live GUI, take_snapshot and revision columns:
- 229 had the same value in both extractions and were kept.
- 21 were settled by reading the handler or its native callee. For these rows, the citation is appended to `file/function`.
- 17 were differences of convention only (`n/a` against `no`), settled by the definitions above.
- 1 cell had the same value in both extractions but was wrong according to the code, and was corrected: `plate_select` take_snapshot, see Findings 14.
- 2 could not be settled and are marked `(unverified)`. `object_duplicate` calls `Selection::clone`, and `device_select` calls `DeviceManager::set_selected_machine`; the source of neither is in the PR.

| driver tool | method | file/function | live GUI | take_snapshot | revision | Windows | Bamburino tool |
|---|---|---|---|---|---|---|---|
| app_get_capabilities | app.get_capabilities | McpTools.cpp:1467 execute() (app.get_capabilities branch) | no | n/a | no | not run | bamburino_capabilities (DriverClient::capabilities) |
| app_get_state | app.get_state | McpTools.cpp:1005 -> app_state (McpTools.cpp:459) | no | n/a | no | not run | studio_get_state (DriverClient::app_state) |
| printer_get_state | printer.get_state | McpTools.cpp:1006 -> printer_state (McpTools.cpp:250) | no | n/a | no | not run | - |
| printer_select_nozzle | printer.select_nozzle | McpTools.cpp:1009 -> select_nozzle (McpTools.cpp:348) -> select_preset (McpTools.cpp:291) | yes | yes | no | not run | - |
| preset_list | preset.list | McpTools.cpp:1007 -> preset_list (McpTools.cpp:271) | no | n/a | no | not run | studio_get_state (DriverClient::active_presets) |
| preset_select | preset.select | McpTools.cpp:1008 -> select_preset (McpTools.cpp:291) | yes | yes | no | not run | - |
| model_list | model.list | McpTools.cpp:1012 -> model_state (McpTools.cpp:206) | no | n/a | no | not run | studio_list_models (DriverClient::model_list) |
| object_inspect | object.inspect | McpTools.cpp:1013 -> object_inspection (McpTools.cpp:230) | no | n/a | no | not run | - |
| plate_list | plate.list | McpTools.cpp:1014 -> plate_state (McpTools.cpp:433) | no | n/a | no | not run | studio_get_state, studio_slice (DriverClient::plate_list) |
| project_new | project.new | McpTools.cpp:1015 -> allow_replace (McpTools.cpp:559), Plater::new_project; Plater.cpp:20265 new_project | yes | no | no | not run | - |
| project_open | project.open | McpTools.cpp:1020 -> allow_replace (McpTools.cpp:559), Plater::load_project; Plater.cpp:20425 load_project -> load_model_objects update()/object_list_changed() 10123-10129 | yes | no | no | not run | - |
| project_save | project.save | McpTools.cpp:1031 -> Plater::export_3mf, set_project_filename, reset_project_dirty_after_save; export_3mf + set_project_filename only, no model change | no | n/a | no | not run | - |
| model_import | model.import | McpTools.cpp:1042 -> Plater::load_files; load_model_objects update()/object_list_changed() Plater.cpp:10123-10129 | yes | no | no | not run | - |
| model_import_multipart | model.import_multipart | McpAdvanced.inc:100 (register_advanced_handlers); snapshot 161, refresh 174/180 | yes | yes | yes | not run | - |
| object_rename | object.rename | McpTools.cpp:1059 (snapshot 1062, refresh 1065) | yes | yes | no | not run | - |
| object_select | object.select | McpTools.cpp:1068 -> obj_list()->select_item; ObjectList::select_item, the sidebar list widget itself | yes | n/a | no | not run | - |
| object_transform | object.transform | McpTools.cpp:1073 (snapshot 1082, refresh 1086) | yes | yes | no | not run | studio_transform_model (DriverClient::object_transform) |
| object_center | object.center | McpTools.cpp:1089 (snapshot 1103, refresh 1107) | yes | yes | no | not run | - |
| object_drop_to_bed | object.drop_to_bed | McpTools.cpp:1110 (snapshot 1123, refresh 1127) | yes | yes | no | not run | - |
| object_mirror | object.mirror | McpTools.cpp:1130 (snapshot 1144, refresh 1148) | yes | yes | no | not run | - |
| object_delete | object.delete | McpTools.cpp:1170 (snapshot 1172) -> Plater::remove; Plater::priv::remove update()+object_list_changed() Plater.cpp:10775-10791 | yes | yes | no | not run | - |
| object_duplicate | object.duplicate | McpTools.cpp:1176 -> selection.clone | no (unverified) | no | no | not run | - |
| object_reorder | object.reorder | McpTools.cpp:1188 -> obj_list()->mcp_reorder_object (refresh 1193) | yes | no | no | not run | - |
| selection_all | selection.all | McpTools.cpp:1196 -> Plater::select_all / select_curr_plate_all; Plater::priv::select_all Plater.cpp:10759 | yes | n/a | no | not run | - |
| selection_clear | selection.clear | McpTools.cpp:1200 -> Plater::deselect_all; Plater::priv::deselect_all Plater.cpp:10765 | yes | n/a | no | not run | - |
| plate_select | plate.select | McpTools.cpp:1201 (check 1202) -> Plater::select_plate; Plater::select_plate render + preview update Plater.cpp:26527-26545; Plater::select_plate take_snapshot("select partplate!") Plater.cpp:26531 | yes | yes | yes | not run | studio_slice, studio_get_slice_summary (DriverClient::plate_select) |
| plate_create | plate.create | McpTools.cpp:1207 (snapshot 1208, refresh 1211) | yes | yes | no | not run | - |
| plate_delete | plate.delete | McpTools.cpp:1214 (check 1215, snapshot 1245, set_plater_dirty 1265) | yes | yes | yes | not run | - |
| plate_rename | plate.rename | McpTools.cpp:1273 (check 1274, snapshot 1277, refresh 1279) | yes | yes | yes | not run | - |
| plate_reorder | plate.reorder | McpTools.cpp:1282 (check 1283, set_plater_dirty 1288) | yes | no | yes | not run | - |
| plate_lock | plate.lock | McpTools.cpp:1291 (check 1292, snapshot 1295, refresh 1297) | yes | yes | yes | not run | - |
| plate_move_instance | plate.move_instance | McpTools.cpp:1300 (check 1301, snapshot 1312, refresh 1315) | yes | yes | yes | not run | - |
| plate_set_print_sequence | plate.set_print_sequence | McpTools.cpp:1348 (check 1349, snapshot 1359, refresh 1361) | yes | yes | yes | not run | - |
| plate_clone | plate.clone | McpTools.cpp:1319 (check 1320, snapshot 1328, refresh 1345) | yes | yes | yes | not run | - |
| plate_set_bed_type | plate.set_bed_type | McpTools.cpp:1364 (check 1365, snapshot 1375, refresh 1377) | yes | yes | yes | not run | - |
| arrange_start | arrange.start | McpTools.cpp:1380 -> Plater::mcp_start_arrange; same ArrangeJob as the GUI button, Plater.cpp:6810-6815; take_snapshot("Arrange") Plater.cpp:6812 | yes | yes | no | not run | - |
| orient_start | orient.start | McpTools.cpp:1389 -> Plater::mcp_start_orient; same OrientJob as the GUI button, Plater.cpp:6817-6822; take_snapshot("Orient") Plater.cpp:6819 | yes | yes | no | not run | - |
| history_undo | history.undo | McpTools.cpp:1398 -> Plater::undo; update_after_undo_redo Plater.cpp:19881-19900 | yes | no | no | not run | - |
| history_redo | history.redo | McpTools.cpp:1399 -> Plater::redo; update_after_undo_redo Plater.cpp:19881-19900 | yes | no | no | not run | - |
| view_set | view.set | McpTools.cpp:1400 -> Plater::select_view; Plater::priv::select_view Plater.cpp:8089 | yes | n/a | no | not run | - |
| view_capture | view.capture | McpAdvanced.inc:184 (check 186); writes PNG only | no | n/a | yes | not run | - |
| settings_get | settings.get | McpTools.cpp:1407 | no | n/a | no | not run | studio_get_settings (DriverClient::writable_keys) |
| settings_effective | settings.effective | McpTools.cpp:1010 -> effective_settings (McpTools.cpp:381), check at 389 | no | n/a | yes | not run | studio_get_settings (DriverClient::settings_effective) |
| settings_update | settings.update | McpTools.cpp:1413 (snapshot 1428, load_config 1429, on_config_change 1430) | yes | yes | no | not run | studio_set_settings, studio_set_supports (DriverClient::settings_update) |
| settings_override | settings.override | McpTools.cpp:1011 -> update_overrides (McpTools.cpp:802), check 810, snapshot 879, refresh 882 | yes | yes | yes | not run | studio_set_settings, studio_set_supports (DriverClient::settings_override) |
| filament_assign | filament.assign | McpTools.cpp:1151 (snapshot 1164, refresh 1166) | yes | yes | no | not run | - |
| geometry_faces | geometry.faces | McpAdvanced.inc:231 (issues meshRevision, does not check it) | no | n/a | no | not run | - |
| geometry_analyze | geometry.analyze | McpAdvanced.inc:261 (check 263) | no | n/a | yes | not run | - |
| object_lay_on_face | object.lay_on_face | McpAdvanced.inc:322 (mesh check 328, snapshot 357, refresh 361) | yes | yes | yes | not run | - |
| object_cut | object.cut | McpAdvanced.inc:621 (snapshot 662, refresh 665) | yes | yes | no | not run | - |
| object_split | object.split | McpAdvanced.inc:589 (snapshot 615, refresh 618) | yes | yes | no | not run | - |
| object_merge | object.merge | McpAdvanced.inc:560 (merge_objects 583, refresh 586); merge(true) -> TakeSnapshot "Assemble" GUI_ObjectList.cpp:3523, 3294 | yes | yes | no | not run | - |
| volume_add | volume.add | McpAdvanced.inc:364 (snapshot 386, advanced_refresh 391) | yes | yes | no | not run | - |
| volume_set_type | volume.set_type | McpAdvanced.inc:394 (snapshot 406, advanced_refresh 409) | yes | yes | no | not run | - |
| painting_set | painting.set | McpAdvanced.inc:412 (mesh check 418, snapshot 438, advanced_refresh 440) | yes | yes | yes | not run | - |
| layers_get | layers.get | McpAdvanced.inc:443 | no | n/a | no | not run | - |
| layers_set | layers.set | McpAdvanced.inc:451 (snapshot 481, advanced_refresh 483) | yes | yes | no | not run | - |
| mesh_repair | mesh.repair | McpAdvanced.inc:486 (mesh check 491, snapshot 514, advanced_refresh 522) | yes | yes | yes | not run | - |
| mesh_simplify | mesh.simplify | McpAdvanced.inc:528 (mesh check 533, snapshot 548, advanced_refresh 556) | yes | yes | yes | not run | - |
| export_geometry | export.geometry | McpAdvanced.inc:668 (file write only) | no | n/a | no | not run | - |
| slice_get_state | slice.get_state | McpTools.cpp:949 -> slicing_state (McpTools.cpp:699) | no | n/a | no | not run | studio_get_state, studio_get_slice_summary (DriverClient::slice_state) |
| preview_get_summary | preview.get_summary | McpPreview.inc:99 -> mcp_preview_summary (McpPreview.inc:31); check at McpPreview.inc:2 | no | n/a | yes | not run | studio_get_slice_summary (DriverClient::preview_summary) |
| preview_get_layers | preview.get_layers | McpPreview.inc:100 -> mcp_preview_layers (McpPreview.inc:51); check at McpPreview.inc:2 | no | n/a | yes | not run | - |
| preview_get_toolpaths | preview.get_toolpaths | McpPreview.inc:101 -> mcp_preview_toolpaths (McpPreview.inc:76); check at McpPreview.inc:2 | no | n/a | yes | not run | - |
| slice_validate | slice.validate | McpTools.cpp:950 -> validate_slice (McpTools.cpp:682), check via current_slice_plate (McpTools.cpp:666); validate_slice makes no GUI call | no | n/a | yes | not run | - |
| slice_start | slice.start | McpTools.cpp:951 -> start_slice (McpTools.cpp:720) -> Plater::mcp_start_slice (McpTools.cpp:736); Plater::mcp_start_slice preview reload Plater.cpp:24263-24282 | yes | n/a | yes | not run | studio_slice (DriverClient::slice_start) |
| slice_start_batch | slice.start_batch | McpTools.cpp:952 -> start_batch (McpBatch.inc:126, check 127) -> advance_batch (McpBatch.inc:42, select_plate 65); select_plate McpBatch.inc:65 + start_slice; only when the plate changes: Plater::select_plate take_snapshot Plater.cpp:26531 | yes | yes | yes | not run | - |
| job_get | job.get | McpTools.cpp:953 -> job_state (McpTools.cpp:653) | no | n/a | no | not run | studio_slice_status (DriverClient::job_get) |
| job_cancel | job.cancel | McpTools.cpp:962 -> mcp_request_cancel / mcp_cancel_ui_job (read set at McpTools.cpp:1482) | no | n/a | no | not run | studio_cancel_slice (DriverClient::job_cancel) |
| export_gcode | export.gcode | McpTools.cpp:1003 -> export_gcode (McpTools.cpp:748), check via current_slice_plate (McpTools.cpp:749) | no | n/a | yes | not run | - |
| export_sliced_3mf | export.sliced_3mf | McpTools.cpp:1004 -> mcp_export_sliced_3mf (McpTools.cpp:916) -> mcp_stage_sliced_plate (McpTools.cpp:886) | no | n/a | yes | not run | - |
| device_list | device.list | McpDevices.inc:365 | no | n/a | no | not run | - |
| device_get_state | device.get_state | McpDevices.inc:375 | no | n/a | no | not run | - |
| device_select | device.select | McpDevices.inc:384 (set_selected_machine at 388) | no (unverified) | n/a | no | not run | - |
| device_pause | device.pause | McpDevices.inc:393 -> mcp_device_command (McpDevices.inc:67) | no | n/a | no | not run | - |
| device_resume | device.resume | McpDevices.inc:395 -> mcp_device_command (McpDevices.inc:67) | no | n/a | no | not run | - |
| device_stop | device.stop | McpDevices.inc:397 -> mcp_device_command (McpDevices.inc:67) | no | n/a | no | not run | - |
| device_upload_plan | device.upload.plan | McpDevices.inc:473 -> mcp_upload_plan (McpDevices.inc:131), current_slice_plate at 141 | no | n/a | yes | not run | - |
| device_upload_start | device.upload.start | McpDevices.inc:481 -> mcp_upload_plan (McpDevices.inc:131), current_slice_plate at 141 | no | n/a | yes | not run | - |
| device_print_plan | device.print.plan | McpDevices.inc:526 -> mcp_print_plan (McpDevices.inc:297), current_slice_plate at 315 | no | n/a | yes | not run | - |
| device_print_start | device.print.start | McpDevices.inc:550 -> mcp_print_plan (555) and mcp_stage_sliced_plate (McpTools.cpp:886) | no | n/a | yes | not run | - |
| ams_inventory | ams.inventory | McpDevices.inc:537 -> mcp_ams_inventory (McpDevices.inc:181) | no | n/a | no | not run | - |
| ams_mapping_plan | ams.mapping.plan | McpDevices.inc:541 -> mcp_print_plan (McpDevices.inc:297) | no | n/a | yes | not run | - |
| calibration_get_state | calibration.get_state | McpDevices.inc:399 -> mcp_calibration_state (McpDevices.inc:92) | no | n/a | no | not run | - |
| calibration_start | calibration.start | McpDevices.inc:403 (operation guard 407, command_start_calibration 415) | no | n/a | no | not run | - |
| account_get_state | account.get_state | McpDevices.inc:420 | no | n/a | no | not run | - |
| account_tasks_list | account.tasks.list | McpDevices.inc:427 | no | n/a | no | not run | - |
| account_presets_list | account.presets.list | McpDevices.inc:457 | no | n/a | no | not run | - |
| preferences_get | preferences.get | McpDevices.inc:589 | no | n/a | no | not run | - |
| preferences_set | preferences.set | McpDevices.inc:598 (app_config save at 609) | no | n/a | no | not run | - |

## Mapping Bamburino

This section checks the draft table "`DriverClient` method → driver tool" in plan B Task 5 against the manifest input schemas and the handler return values. Status `confirmed` means the draft is correct. Status `corrected` means plan B must apply the note.

| `DriverClient` method | driver tool | driver arguments (manifest) | status | note |
|---|---|---|---|---|
| `app_state` | `app_get_state` | `{}` | corrected | The response has `sessionId, appVersion, projectName, projectPath, dirty, presetsDirty, uiJobRunning, slicingRunning, modalOpen, objectCount, plateCount, selectedPlateIndex, platerView` (`McpTools.cpp:459-466`). It has **no undo-snapshot field**, so `AppState.undo_snapshot` is `None` until plan A Task 7 adds one (`Plater::get_active_snapshot_time()` exists, see Findings 12). |
| `capabilities` | `app_get_capabilities` | `{}` | corrected | The response has `sessionId, protocolVersion, appVersion, methods, features` (`McpTools.cpp:1467-1472`). It has no `buildSuffix` or `patchId` today; plan A Task 5 adds them, and until then the `Option`s are `None`. `methods` holds dotted native names, not tool names. |
| `plate_list` | `plate_list` | `{}` | confirmed | Returns `plates[{index,name,locked,selected,printable}]`, `selectedIndex`, `plateRevision` (`McpTools.cpp:433-448`). A new revision is issued on every call. |
| `plate_select` | `plate_select` | `{plateIndex, expectedPlateRevision}` (both required) | confirmed | Returns a fresh `plate_state`. `Plater::select_plate` takes an undo snapshot (`Plater.cpp:26531`), so every earlier revision becomes stale. Use the `plateRevision` returned by this call. |
| `model_list` | `model_list` | `{}` | confirmed | Returns `objects[{index,id,name,dimensions,instances[{index,id,position,rotation,scale,plateIndex}],volumes}]`, with rotation in degrees (`McpTools.cpp:206-227`). The JSON names are `dimensions` and `position`, not `*_mm`. |
| `active_presets` | `preset_list` x3 | `{kind: "printer"\|"process"\|"filament"}` (required) | corrected | The selected preset is the string field `selected`. The filaments are the array field `slots`, returned only for `kind=filament` (`McpTools.cpp:271-289`). `printer_get_state` would answer in one call (`selected{printer,process,filaments}`, `:268`), but it is outside the plan B allowlist (`printer_` prefix), so the three calls stay. |
| `writable_keys` | `settings_get` | `{}` | confirmed | The field is `writableKeys`, holding the 20 keys (`McpTools.cpp:1407-1411`). |
| `settings_effective` | `settings_effective` | `{keys?, plateIndex?, expectedPlateRevision?, objectId?, volumeId?}` | corrected | `plateIndex` needs `expectedPlateRevision`, and `volumeId` needs `objectId`. The response is `values{<key>:{value,source}}`, an object keyed by setting name rather than an array. `source` is one of `global\|plate\|object\|volume` (`McpTools.cpp:381-428`); map `global` to `SettingSource::Project`. An unknown key returns `INVALID_PARAMS`. |
| `settings_update` | `settings_update` | `{values: {key: string}}` (1-100 entries) | confirmed | Accepts only the 20 `writable_settings()` keys, all values as strings. See Findings 4 for what undo does and does not revert. |
| `settings_override` | `settings_override` | `{scope: "plate"\|"object"\|"volume", values, plateIndex+expectedPlateRevision \| objectId[+volumeId]}` | corrected | `scope` also accepts `volume`. Accepts the 19 `override_settings()` keys. A locked plate or locked object is rejected (`McpTools.cpp:802-830`). The response has a new `plates.plateRevision` (`:884`). |
| `object_transform` | `object_transform` | `{objectId, instanceId, position?, rotation?, scale?}` | corrected | **`instanceId` is required** by the manifest, so `instance_id` cannot be `Option`: pass the first instance's id from `model_list`. Rotation is in degrees and absolute, and at least one of position, rotation or scale must be given. The tool returns the whole `model_state`, not a single object (`McpTools.cpp:1073-1088`). |
| `slice_start` | `slice_start` | `{plateIndex, expectedPlateRevision}` | corrected | Works only on the **selected** plate: call `plate_select` first (Findings 9). The tool runs validation, returns `VALIDATION_FAILED` on errors, and can return a job that is already `succeeded` with `reused:true` (`McpTools.cpp:720-747`). |
| `job_get` / `job_cancel` | `job_get` / `job_cancel` | `{jobId}` | corrected | `status` is one of `running\|cancelling\|succeeded\|failed\|cancelled\|stale`. There is no `queued`, so `JobStatus::Queued` is never returned; map `cancelling` and `stale` explicitly. The other fields are `progressPercent, message, error{code,message}, result{sliceResultId,reused}` (`McpTools.cpp:653-663`). |
| `slice_state` | `slice_get_state` | `{}` | corrected | The fields are `busy` (not `running`), `valid`, `readyForExport`, `plateIndex`, `plateRevision`, `progressPercent`, `sliceResultId`, `currentJobId` and `statistics{estimatedNormalTime,estimatedSilentTime,totalToolchanges}` (`McpTools.cpp:699-718`). |
| `preview_summary` | `preview_get_summary` | `{plateIndex, expectedPlateRevision, sliceResultId}` | corrected | Returns `moveCount, extrusionMoveCount, travelMoveCount, supportMoveCount, filamentCount, toolpathOutside, warnings[{level,code,message}]` (`McpPreview.inc:31-49`). It has **no print time, no grams or meters per filament, and no support volume**. Take the print time from `slice_get_state.statistics`; filament usage and support volume are not exposed by the driver today. |

`DriverTool::PlateSelect.is_mutation()` must return `true`: `plate_select` takes an undo snapshot and invalidates plate revisions (Findings 14).

## Findings

### 1. take_snapshot before mutation

take_snapshot settings.update: present - `GUI/McpTools.cpp:1428`, snapshot name `"MCP update process settings"` (called after validation at :1426-1427, before `tab->load_config(candidate)` at :1429).
take_snapshot object.transform: present - `GUI/McpTools.cpp:1082`, snapshot name `"MCP transform instance"` (called after the new values are computed at :1078-1080, before `set_offset/set_rotation/set_scaling_factor` at :1083).
take_snapshot settings.override: present - `GUI/McpTools.cpp:879`, snapshot name `"MCP update scoped settings"` (called after the whole validation block :826-878, before the config assignment at :880-881).

Notes:
- `Plater::take_snapshot(const std::string&)` is declared at `GUI/Plater.hpp:577` and delegates to `Plater::priv::take_snapshot` (`GUI/Plater.cpp:24980`, `:19569`). That function returns silently without a snapshot if `m_prevent_snapshots > 0` (`GUI/Plater.cpp:19571-19572`) or if the `m_single` filter rejects it (`GUI/Plater.cpp:19595-19596`). The MCP code does not check that a snapshot was actually taken.
- What the snapshot stores (`Utils/UndoRedo.cpp:940-956`): Model, Selection, GLGizmosManager, PartPlateList. The process preset config (the Tab config edited by `settings.update`) is not among the saved objects, so the "MCP update process settings" snapshot is at most a timeline marker. Whether undo reverts the preset edit was not verified (see Open questions).
- The generic busy/modal gate runs before every mutating handler (`GUI/McpTools.cpp:1482-1491`), so snapshots are never taken when the call is rejected as BUSY.

### 2. app.get_capabilities returned fields

Built inline in `execute()` at `GUI/McpTools.cpp:1467-1472`. Top-level JSON keys:
- `sessionId` (`session_id()`, a per-process random UUID, `GUI/McpTools.cpp:93-96`)
- `protocolVersion` (integer constant `1`)
- `appVersion` (`SLIC3R_VERSION`)
- `methods` (array: `"app.get_capabilities"` first, then every key of `handlers()`; these are internal method names such as `settings.update`, not manifest tool names such as `settings_update`)
- `features` (object) with keys: `jobCompletionTracking` (true), `jobTrackingScope` (string "FFF plate slicing, sequential plate queue, UI jobs and external device jobs"), `parameterIndices` ("zero-based"), `objectReferences` ("stable IDs"), `plateReferences` ("index with required expectedPlateRevision"), `securityPolicyVersion` (1), `deviceActionsEnabled` (`device_actions_enabled()`, `GUI/McpTools.cpp:1456-1458`), `accountReadsEnabled` (`account_reads_enabled()`, `:1460-1462`).

Not present: any `limits` key (the manifest description for `app_get_capabilities` says "versions and limits"), any undo/snapshot key, any `modalOpen`. `app.get_capabilities` is handled before the busy gate and before the handler lookup, so it is not itself a key in `handlers()` (it is added by hand to `methods`).

### 3. model_import

- Manifest tool name: `model_import` (`GUI/McpToolsManifest.json:304`), method `model.import` (:305). Description: "Add STL/OBJ/STEP/3MF geometry. 3MF import loads geometry; use project_open to load project settings."
- Argument: `paths`, array of strings, each `minLength 1` / `maxLength 32768`, `minItems 1`, `maxItems 100`; `required: ["paths"]`; `additionalProperties: false` (manifest :307ff).
- Handler: `"model.import"` lambda at `GUI/McpTools.cpp:1042-1058`.
- Path rules: the transport-level refinement `validate_refinements` (`GUI/McpTools.cpp:1677`) requires every `paths` entry to be absolute with no NUL (block at :1690-1699). The handler then calls `input_path()` (`GUI/McpTools.cpp:198-204`), which requires non-empty, <=32768 chars, no NUL, `path.is_absolute()` ("File path must be absolute") and `is_regular_file` ("Input file does not exist"). Relative paths are rejected with `INVALID_PARAMS` (surfaced as JSON-RPC -32602, see item 7). There is no relative-path resolution and no base directory.
- Accepted formats: extension (lowercased) in `{".stl", ".obj", ".3mf", ".step", ".stp"}` (`GUI/McpTools.cpp:1044`); anything else raises `INVALID_PARAMS` "Unsupported geometry import format" (:1050).
- Execution: inside `ImportScope` (sets `noninteractive_import=true`, `GUI/McpTools.cpp:102-106`), calls `p.load_files(paths, LoadStrategy::LoadModel | LoadStrategy::Silence, false)` (:1054-1055). Empty result raises `NATIVE_ERROR` "Native import did not load any objects" (:1056).
- Return: `{importedIndices, model}` where `model` is `model_state(p)` (:1057).
- No `take_snapshot` call in this handler (`GUI/McpTools.cpp:1042-1058`); any snapshot would come from inside `Plater::load_files`, which was not traced.
- 3MF via `model.import` loads geometry only; project settings need `project.open` (`.3mf` only, `GUI/McpTools.cpp:1020-1030`).

### 4. settings.update: Tab config, dirty flag, no preset save

Handler: `GUI/McpTools.cpp:1413-1433`. Sequence:
1. Gets the process Tab: `wxGetApp().get_tab(Preset::TYPE_PRINT)` (:1415); missing -> `BUSY` "Process settings are unavailable" (:1416).
2. `DynamicPrintConfig candidate(*tab->get_config())` (:1417) and `full = preset_bundle->full_config_secure()` (:1418); each value goes through `set_deserialize_strict` on both (:1424-1425); `full.validate()` (:1426) -> `INVALID_PARAMS` on error (:1427).
3. `p.take_snapshot("MCP update process settings")` (:1428).
4. `tab->load_config(candidate)` (:1429). Upstream `Tab::load_config` (`GUI/Tab.cpp:1619-1630`) copies differing keys into `m_config` (the edited preset), then calls `update_dirty()`, `reload_config()`, `update()`. It does not save.
5. `p.on_config_change(wxGetApp().preset_bundle->full_config_secure())` (:1430; `GUI/Plater.cpp:25238`).
6. `p.update_project_dirty_from_presets()` (:1431; `GUI/Plater.cpp:20188`).
7. Returns `{updated, scope:"process"}` (:1432).

Search results (grep over `McpTools.cpp`, `McpAdvanced.inc`, `McpBatch.inc`): `save_preset` not found, `save_current_preset` not found, `update_dirty` not found directly (it runs only inside `Tab::load_config`), `on_value_change` not found. `load_config` appears only at :1429. So the preset is modified in memory (edited preset, "modified" state) and never saved to disk by this path.

Consequences visible in the code:
- `app.get_state` reports `presetsDirty = plater.is_presets_dirty()` (`GUI/McpTools.cpp:462`) and `dirty = plater.is_project_dirty()` (:461).
- A subsequent `preset.select` is refused with `INTERACTION_REQUIRED` "Save or discard edited presets before selecting another profile" (:296-297).
- A subsequent `project.open` is refused with `DIRTY_PROJECT` unless `discardChanges=true` (`allow_replace`, :559-562; used at :1023).
- `settings.update` takes an undo snapshot, so it bumps the active snapshot time (item 12) and every previously issued `plateRevision` becomes stale (item 10).
- It edits only the Tab's edited preset (global process settings); per-plate/object/volume overrides are `settings.override` (item 5).

- Undo caveat: `settings.update` writes the edited process preset through `Tab::load_config` (`GUI/Tab.cpp:1619-1630`), while the undo snapshot stores Model, Selection, Gizmos and PartPlateList (`Utils/UndoRedo.cpp:940-956`). The snapshot may therefore not carry the preset config, and Ctrl+Z may not revert an agent settings change. This is **to verify manually in Task 9 step 4**.

### 5. Whitelists

`writable_settings()` - `GUI/McpTools.cpp:776-782` - 20 keys:
1. layer_height
2. initial_layer_print_height
3. wall_loops
4. top_shell_layers
5. bottom_shell_layers
6. sparse_infill_density
7. sparse_infill_pattern
8. enable_support
9. support_type
10. support_threshold_angle
11. support_on_build_plate_only
12. brim_type
13. brim_width
14. skirt_loops
15. outer_wall_speed
16. inner_wall_speed
17. sparse_infill_speed
18. travel_speed
19. print_sequence
20. seam_position

`override_settings()` - `GUI/McpTools.cpp:784-792` - 19 keys: the same list without `print_sequence`.

Count confirmed: 20 / 19. Both are enforced with `.count(key)` plus `has(key)` plus "value must be a string" (<=1024 chars): `settings.update` at :1420, `settings.override` at :827. `settings.get` returns only keys of `writable_settings()` (:1409-1411) and echoes `writableKeys`. `values` must contain 1-100 entries (`validate_refinements`, :1717-1718).

### 6. app.get_state modalOpen

Yes. `app_state()` (`GUI/McpTools.cpp:459-466`) returns the keys: `sessionId`, `appVersion`, `projectName`, `projectPath`, `dirty`, `presetsDirty`, `uiJobRunning`, `slicingRunning`, `modalOpen`, `objectCount`, `plateCount`, `selectedPlateIndex`, `platerView` ("preview" or "prepare").
`modalOpen` = `modal_open()` (`GUI/McpTools.cpp:450-457`): iterates `wxTopLevelWindows`, `dynamic_cast<wxDialog*>`, returns true if any `dialog->IsModal()`. It is not `wxGetActiveWindow` and not a counter.
`modal_open()` is also part of the mutation gate (`GUI/McpTools.cpp:1490`). `app.get_state` is a read method (`GUI/McpTools.cpp:1482`), so it is answered even when a dialog is open.

### 7. RpcError codes

Definition: `struct RpcError : std::runtime_error { std::string code; ... }` at `GUI/McpTools.cpp:53-56`. `require(cond, msg)` throws `INVALID_PARAMS` (`:109-111`). The code is a free string; there is no enum. Conversion happens in `McpTools::call` (`GUI/McpTools.cpp:1753`, `catch (const RpcError&)` at :1769): `INVALID_PARAMS` is rethrown as `std::invalid_argument` (:1770), which `GUI/GUI_App.cpp:6801-6802` turns into a JSON-RPC error -32602 (HTTP 400, `Utils/McpServer.cpp:224-229`). Every other code returns a normal tool result `{content:[{type:text,text:<json {code,message}>}], structuredContent:{code,message}, isError:true}`.

Codes (line numbers from `grep -n 'RpcError("'` over `GUI/McpTools.cpp` unless a file is named):
- `INVALID_PARAMS` - `:110` (via `require`, used all over), `:278`, `:303`, `:306`, `:367`, `:375`, `:428`, `:824`, `:835`, `:1139`, `:1356`, `:1427`. Bad or out-of-range parameter, wrong scope, unknown/unsupported setting or non-string value, native config validation failure, ambiguous or incompatible preset, relative path. Surfaces as -32602, not as `isError`.
- `STALE_REFERENCE` - `:149` (object id), `:156` (instance id), `:163` (volume id), `:189` (`check_plate_revision`: revision missing from the 32-entry store or fingerprint changed), `:314`, `:335`, `:352`, `:752`, `:890`, `:908`, `:960` and `:1001` (unknown/expired job), `:1518` and `GUI/McpJobs.hpp:85` (slice job invalidated, appears as `job.error.code`), and `GUI/McpAdvanced.inc:56` (`advanced_check_mesh`: mesh changed or observation expired). Any id or revision that no longer matches the current state.
- `BUSY` - `:310` (preset Tab unavailable), `:531` (external job capacity full, 32), `:670` (native export/upload already scheduled), `:1416` (process Tab unavailable), `:1490` (generic gate: modal open, interactive edit/drag, project loading, any UI job / background process / slicing / batch / external job running), `GUI/McpBatch.inc:149` (plate queue history full), `GUI/McpDevices.inc:409` (device printing or calibrating).
- `INTERACTION_REQUIRED` - `:297`, `:320`, `:337`, `:678`, `:694`, `:1556` (`mcp_require_interactive_import`, thrown from native import code when a dialog would be needed), `GUI/McpDevices.inc:80`, `:225`. The action needs a human decision in the GUI (edited presets, post-process scripts, PVA/nozzle warning, printer switch repositioning objects, AMS mapping).
- `NATIVE_ERROR` - `:328`, `:340`, `:499`, `:738`, `:904`, `:906`, `:1028`, `:1037`, `:1056`, `:1185`, `:1204`, `:1210`, `:1256`, `:1264`, `:1287`, `:1314`, `:1330`, `:1343`; also written as `job.error.code` at `:737`, `:742`, `:771`, `:1522`, `:1528`, `:1538`. A native call returned failure.
- `UNSUPPORTED` - `:308`, `:671`, `:673`, `:1474` (method not in `handlers()`: "Method is not implemented in this native build"), `GUI/McpDevices.inc:138`, `:261`, `:306`, `:312`, `:321`, `:333`, `:335`, `:410`.
- `PERMISSION_DENIED` - `:1478` (device actions disabled in Preferences), `:1480` (account reads disabled).
- `DIRTY_PROJECT` - `:561` (project or presets dirty and `discardChanges` not true).
- `VALIDATION_FAILED` - `:722` (`start_slice`: `slice.validate` returned errors; message = JSON array of errors).
- `UNAVAILABLE` - `GUI/McpDevices.inc:40`, `:59`, `:61`, `:102`, `:136`, `:300`, `:304`, `:360`, `:592`, `:601`.
- `INVALID_STATE` - `GUI/McpDevices.inc:76`, `:79`, `:84`, `:231`, `:234`, `:272`, `:311`, `:314`.
- `DEVICE_OFFLINE` - `GUI/McpDevices.inc:62`.
- `ACCOUNT_REQUIRED` - `GUI/McpDevices.inc:103`, `:307`.
- `RESOURCE_LIMIT` - `GUI/McpAdvanced.inc:543` (simplification exceeded 8 s).

Not present: a `TIMEOUT` or `CANCELLED` RpcError code. A request timeout is a transport HTTP 504 with JSON-RPC code -32603 (`Utils/McpServer.cpp:379`). Transport-level JSON-RPC codes: -32700, -32600, -32601, -32602, -32603, -32020, -32022 (`Utils/McpServer.cpp:256-379`). Schema failures (`validate_schema`, `GUI/McpTools.cpp:1617`) throw `std::invalid_argument` -> -32602. Also non-coded failures: "Another MCP operation is active" and "The editor is rebuilding" are `isError:true` results without a `code` (`GUI/GUI_App.cpp:6790`, `:6794`).

### 8. Queue and timeout

- Default request timeout: `explicit McpServer(std::chrono::milliseconds request_timeout = std::chrono::seconds(30))` at `Utils/McpServer.hpp:40`. `GUI_App::mcp_start_on_port` creates it with `std::make_unique<McpServer>()` (`GUI/GUI_App.cpp:6771`), so the effective timeout is 30 s. It is stored in `m_request_timeout` (`Utils/McpServer.cpp:423`) and passed to `Impl` (`:437`).
- Timer: in `Connection::dispatch`, `ticket = std::make_shared<McpRequestTicket>()` (`Utils/McpServer.cpp:374`), `deadline.expires_after(p->request_timeout)` (:375), `async_wait` (:376-380). On expiry (connection still open and not yet writing) it calls `McpServerAccess::cancel(ticket)` (:378) and replies HTTP 504 / -32603 "Request timed out; inspect operation state before retrying" (:379). The 30 s covers GUI-queue wait plus execution, not only queue wait.
- Cancellation token: `McpRequestTicket` (`Utils/McpServer.hpp:18-29`) with `m_state` (Pending / Started / Canceled) and `m_abandoned`. `McpServerAccess::cancel` (`Utils/McpServer.cpp:23-29`) sets `m_abandoned=true`, then CAS Pending -> Canceled. `try_start()` (`:32-37`) returns false if abandoned, else CAS Pending -> Started and re-checks `m_abandoned`. The same `cancel` runs on connection close (`:186`) and on dispatch exception (`:392`).
- Use in the GUI path: the handler does `CallAfter(...)` and the first statement of the lambda is `if (lifetime.expired() || is_closing() || generation != m_mcp_generation || !ticket->try_start()) return;` (`GUI/GUI_App.cpp:6787-6788`). A mutation still sitting in the wx event queue when the timer fires, or when the client disconnects, finds the ticket canceled and returns without executing and without calling `completion`.
- Guarantee assessment: real for the queued (Pending) case, and race-safe by construction (cancel sets `m_abandoned` before the CAS; `try_start` re-checks `m_abandoned` after winning the CAS). It does not cover a call that already started: once `try_start()` returned true the tool runs to completion on the GUI thread (nothing interrupts it), and its response is dropped because `Connection::result` returns when `ticket->canceled()` (`Utils/McpServer.cpp:222`). A late-finishing mutation is therefore committed while the client saw a 504. The 504 text ("inspect operation state before retrying") acknowledges this.
- Concurrency: a second call reaching the lambda while `m_mcp_call_active` is set is rejected, not queued (`GUI/GUI_App.cpp:6793-6796`).
- Server stop: `mcp_stop()` increments `m_mcp_generation` (`GUI/GUI_App.cpp:6756`); the lambda checks `generation != m_mcp_generation` (:6788), so queued calls from a stopped server also never execute.

### 9. slice.start

- Selected plate only. `current_slice_plate()` (`GUI/McpTools.cpp:664-682`) calls `check_plate_revision`, then `index(params,"plateIndex",count)`, then `require(plate == get_curr_plate_index(), "Only the selected plate is supported; select it explicitly first")` (:668). The caller must pass `plateIndex` equal to the currently selected plate and a fresh `expectedPlateRevision` (both required in the manifest). The server never selects a plate for the caller; `plate.select` is a separate tool (`GUI/McpTools.cpp:1201-1204`). Sequential multi-plate slicing is `slice.start_batch` (`GUI/McpBatch.inc`).
- Other gates in the same function: export/upload already scheduled -> `BUSY` (:669-670); not FFF -> `UNSUPPORTED` (:671); G-code-only / gcode.3mf / empty model / calib pattern -> `UNSUPPORTED` (:672-673); non-empty `post_process` script -> `INTERACTION_REQUIRED` (:674-679). Then `start_slice` (`GUI/McpTools.cpp:720-747`) validates (`VALIDATION_FAILED` at :722), reuses a still-current result (job created already `succeeded`, `reused:true`, :723-729) or starts via `Plater::mcp_start_slice(run)` (`GUI/Plater.cpp:24263-24282`).
- `job.status` values (field `status` in `job_state`, `GUI/McpTools.cpp:660-661`, taken from `McpSliceJob::state`, `GUI/McpJobs.hpp:26`, default `"running"`): plain strings, no enum.
  - `running` (initial, `McpJobs.hpp:26`)
  - `cancelling` (`GUI/McpTools.cpp:968`, `:1514`)
  - `succeeded` (`GUI/McpTools.cpp:728`, `:1535`; `McpJobs.hpp:81` sets progress 100)
  - `failed` (`GUI/McpTools.cpp:737`, `:742`, `:771`, `:1522`, `:1528`, `:1538`)
  - `cancelled` (`GUI/McpTools.cpp:1521`)
  - `stale` (`McpJobs.hpp:83-88` `invalidate`; `GUI/McpTools.cpp:1518`)
  Terminal states: succeeded, failed, cancelled, stale (`finish()` sets `worker_pending=false`, `McpJobs.hpp:73-82`). `job_state` also returns `progressPercent`, `error{code,message}`, `result{sliceResultId,reused}`, `completionObserved`, `workerPending`, `cancelRequested` (:660-662). Progress is capped at 99 until success (`McpJobs.hpp:68-70`).
- Other job families use similar words: UI jobs (`GUI/McpTools.cpp:64`, `:487`, `:1547`: running/cancelling/succeeded/failed/cancelled), external jobs (`:75`, `:501`: running/cancelling/succeeded/failed), batch jobs (`GUI/McpBatch.inc:2`: running/cancelling/succeeded/failed/cancelled/stale).
- `succeeded` is set only after `mcp_slice_completed` verified that the plate result is valid, exportable, has moves, and a non-empty tmp G-code file exists (`GUI/McpTools.cpp:1525-1531`). The manifest says submission or progress 100 does not establish success.

### 10. Revisions

- `plateRevision`: opaque string `"<sessionUUID>:<counter>"` produced by `plate_state()` (`GUI/McpTools.cpp:430-448`): `reference_id(++plate_observation_counter)` (:441). It is issued on every call of `plate_state()`, so it changes on every read: `plate.list` (:1014), `slice.get_state` (:715 via `slicing_state`), batch setup (`GUI/McpBatch.inc:67`), and the `plates` key returned by mutating handlers (e.g. `settings.override`, :884). The server stores a fingerprint per revision in `plate_observations` (FIFO of 32, `GUI/McpTools.cpp:441-446`); a revision is valid while the fingerprint recorded then equals `plate_fingerprint(plater)` now (`check_plate_revision`, :186-190). An old revision therefore stays valid as long as the fingerprint is unchanged and it has not been pushed out of the 32-entry store.
- Fingerprint contents (`plate_fingerprint`, `GUI/McpTools.cpp:172-185`): `model().id()`, `plater.get_active_snapshot_time()` (:173), per plate: pointer, name, locked, bed type, real print sequence, build volume min/max; and object ids. Consequence: any `take_snapshot` (every MCP mutation, plus GUI edits, undo and redo) invalidates all outstanding plate revisions.
- `meshRevision`: not in `McpTools.cpp`; defined in `GUI/McpAdvanced.inc:40-49` (`advanced_revision`) and returned by geometry tools (`McpAdvanced.inc:258`, `:308`, `:441`, `:526`, `:558`). It is `"<session>:mesh:<random uuid>"`, cached in `mcp_mesh_observations` (deque, max 64, `McpAdvanced.inc:6`, :46-47) keyed by (volume id, `weak_ptr` of the volume's shared mesh). Repeated reads of an unchanged mesh return the same revision; a new revision appears only when the mesh object is replaced or the entry was evicted. `advanced_check_mesh` (`McpAdvanced.inc:51-57`) throws `STALE_REFERENCE`.
- Stable IDs for objects/instances/volumes: `reference_id(model id)` = `"<sessionUUID>:<id>"` (`GUI/McpTools.cpp:98-100`); the session UUID is regenerated at every process start (:93-96).

### 11. Threading

- HTTP side: a dedicated thread runs the `io_context` (`Utils/McpServer.cpp:452-455`). `Connection::dispatch` calls `p->handler(method, params, ticket, completion)` on that thread (`:384`).
- To the GUI thread: the handler lambda in `GUI_App::mcp_start_on_port` (`GUI/GUI_App.cpp:6774-6811`) answers `tools/list` and invalid calls directly on the HTTP thread (:6777-6784, after `ticket->try_start()`), and for `tools/call` copies name and arguments and calls `CallAfter(...)` on the `GUI_App` object itself (:6787; not `wxTheApp->CallAfter`, not a custom event). Inside the lambda it runs `McpTools::call(*plater(), name, arguments)` (:6800), with `m_mcp_call_active` set around it (:6797, :6808).
- Back to HTTP: the result is passed to `completion(std::move(result))` (:6809). `completion` is the lambda built at `Utils/McpServer.cpp:384-390`, which does `net::post(owner->io, ...)` and runs `Connection::result` on the `io_context` thread (:387-389), where the response is serialized and written (`send`, :195-213). It is skipped if the server stopped or the ticket was canceled (`:386`, `:222`).
- No blocking wait: the HTTP thread does not block on the GUI; the 30 s deadline timer (item 8) is the only bound. Schema validation and semantic checks both run on the GUI thread (inside `McpTools::call`).
- Calls are serialized by the GUI event loop plus `m_mcp_call_active`. Long-running work (slicing, external/device jobs) is asynchronous and tracked as jobs (`job.get`).

### 12. UndoRedo::Stack active snapshot identifier

Yes. A one-line accessor exists and the MCP code already uses it:
- `Plater::get_active_snapshot_time()` declared at `GUI/Plater.hpp:581` (`size_t get_active_snapshot_time();`), defined at `GUI/Plater.cpp:24983` as `return p->undo_redo_stack().active_snapshot_time();`.
- Underlying: `size_t UndoRedo::Stack::active_snapshot_time() const` at `Utils/UndoRedo.hpp:168` (also `active_snapshot()` returning `const Snapshot&` at :169, `snapshots()` at :162, `snapshot(size_t time)` at :163). Implementation member `m_active_snapshot_time` (`Utils/UndoRedo.cpp:600`, `:689`).
- Semantics (`Utils/UndoRedo.cpp`): `take_snapshot` sets `m_active_snapshot_time = ++m_current_time` (:958), so it increases with each snapshot; undo/redo set it to the target timestamp (:1005, :1018, :1031); `clear` resets it to 0 (:556). The active snapshot can be the "topmost" placeholder when no undo has happened (`UndoRedo.hpp:165-167`).
- Existing use: `plate_fingerprint` reads it (`GUI/McpTools.cpp:173`), so it is already part of the plate revision logic.
- Also available: `Plater::undo_redo_stack_main()` (`GUI/Plater.hpp:592`) returning `const UndoRedo::Stack&`.

### 13. Transport and auth

- Bind address: loopback only, `net::ip::make_address_v4("127.0.0.1")` (`Utils/McpServer.cpp:438`); Windows sets `SO_EXCLUSIVEADDRUSE` (:440-444), other platforms `reuse_address(true)` (:446). `listen(16)` (:449); max 16 connections (`max_connections`, :44; check at :402).
- Port: default `27183` (`GUI/GUI_App.cpp:6731-6741` `mcp_port()`; also seeded in `libslic3r/AppConfig.cpp:137-138` as `mcp_server_port`). Valid range 1024-65535 (`Utils/McpServer.cpp:431`). Server is off by default (`libslic3r/AppConfig.cpp:135-136`, `mcp_server_enabled=false`); enabled in Preferences or autostarted at launch if the setting is true (`GUI/GUI_App.cpp:1498-1500`, `:4500-4502`).
- Endpoint: `POST /mcp` only (`Utils/McpServer.cpp:280-281`); address shown as `http://127.0.0.1:<port>/mcp` (`GUI/GUI_App.cpp:6743-6747`).
- Token generation: 32 bytes from OpenSSL `RAND_bytes`, hex-encoded to 64 lowercase hex chars (`GUI/McpCredentials.cpp:25-26`, `:33-45`). Persisted in `<data_dir()>/mcp.token` (`McpCredentials.cpp:28-31`, `:199-217`; called with `data_dir()` from `GUI/GUI_App.cpp:6847`, `:6883`), created with an owner-only DACL on Windows (`McpCredentials.cpp:57-96`, `:124-140`) and mode 0600 with `O_EXCL|O_NOFOLLOW` on POSIX (`:173-185`); the file is re-secured on every read (`:98-122`, `:154-171`). Created on first enable and reused afterwards; regeneration writes a temp file then atomically replaces it (`:219-229`) and restarts the server (`GUI/GUI_App.cpp:6887-6921`).
- Token display: Preferences shows it masked as eight bullet characters, with Show/Hide token, Copy token (clipboard) and Regenerate buttons (`GUI/Preferences.cpp:1400-1409` label logic, `:1615` copy button, `:1640-1660` handlers). The token is read from the file on demand via `mcp_copy_token` (`GUI/GUI_App.cpp:6882-6885`). The status label shows address and errors (`GUI/GUI_App.cpp:6822`), not the token.
- Required request headers (all checked in `Connection::validate_http`, `Utils/McpServer.cpp:262-291`, before the body is processed):
  - `Authorization: Bearer <token>` - exactly one header, scheme case-insensitive, length must equal `token.size()+7`, compared with constant-time `CRYPTO_memcmp` (:275-279). Failure -> HTTP 401 with `WWW-Authenticate: Bearer realm="BambuStudio MCP"` (:205, :279). This is the auth check.
  - `Host` must be exactly `127.0.0.1:<port>` or `localhost:<port>`, single header (:266-270) -> 403 otherwise.
  - `Origin`, if present, must be `http://127.0.0.1:<port>` or `http://localhost:<port>` (:271-274) -> 403.
  - `Content-Type: application/json` (single header; parameters allowed) (:282-284) -> 415.
  - `Accept` must list both `application/json` and `text/event-stream` with q>0 (:285-287) -> 406.
  - No `Content-Encoding`, no `Expect` (:288-289).
  - Modern protocol (`2026-07-28`) requests also need `MCP-Protocol-Version`, `Mcp-Method`, and for `tools/call` `Mcp-Name` (optionally `=?base64?...?=`), all matching the body `_meta` (`modern_headers`, :293-320). The legacy path (`2025-11-25`) uses `initialize`, then `MCP-Protocol-Version: 2025-11-25` on later requests (:342-346).
  - Limits: body 1 MiB, header 8 KiB, JSON nesting 64, 10 s read timeout, keep-alive disabled (`Utils/McpServer.cpp:44-46`, `:56-70`, `:203`, `:241`).
- Order of checks: Host, Origin, then Authorization, then path and method, then content headers (so a bad Host gets 403 even without a token; a bad token on a good Host gets 401).

### 14. Audit additions (merge of the two extractions)

- `plate.select` takes an undo snapshot: `Plater::select_plate` calls `take_snapshot("select partplate!")` (`Plater.cpp:26531`). Both extractions had recorded `n/a`. The snapshot bumps `get_active_snapshot_time()` and therefore invalidates every outstanding `plateRevision` (Findings 10); only the revision returned by `plate_select` is valid afterwards. `slice.start_batch` reaches the same call when it switches plate (`McpBatch.inc:65`).
- `arrange.start` and `orient.start` take a snapshot inside `m_ui_jobs.arrange/orient` (`Plater.cpp:6812`, `:6819`) before the job starts.
- `object.merge` takes a snapshot through `ObjectList::merge_objects` -> `merge(true)` -> `TakeSnapshot "Assemble"` (`GUI_ObjectList.cpp:3523`, `:3294`).
- `model.import` (open question 2) takes no snapshot on its path. A grep finds no `take_snapshot`/`TakeSnapshot` in `Plater::priv::load_files` (`Plater.cpp:8490-9963`) or `load_model_objects` (`:9964-10130`); the "Load Files" snapshot of the interactive GUI path is not on this route. An import therefore cannot be undone as a single step, which matters for the smoke test in plan C Task 4.

### Open questions

1. Whether undoing the `"MCP update process settings"` snapshot reverts the process-preset edit. The snapshot stores Model/Selection/Gizmos/PartPlateList (`Utils/UndoRedo.cpp:940-956`), none of which holds the Tab's edited preset, so it probably does not; the undo path (`Plater::priv::undo` / snapshot load) was not traced. A driver should not rely on undo to roll back `settings.update`.
2. (Answered in Findings 14: no snapshot on the `model.import` path.) Whether `Plater::load_files` (called by `model.import`, `GUI/McpTools.cpp:1055`) takes its own undo snapshot. `Plater.cpp` is about 28k lines; `load_files` was not traced.
3. Whether `Plater::priv::take_snapshot` can silently skip in the three mutating paths (`m_prevent_snapshots > 0`, `m_single` filter, `GUI/Plater.cpp:19571-19596`). Under normal entry from `CallAfter` it is expected not to, but this was not verified at runtime, and the MCP code does not detect a skipped snapshot.
4. Secondary line numbers in item 7 come from a single `grep -n 'RpcError("'` pass over `McpTools.cpp` and the `.inc` files. `McpGcodeExport.hpp`, `McpPreview.inc` and `McpJobs.hpp` contain no `RpcError("` literal per that grep.
5. `tests/mcp/test_mcp_server.cpp` (352 lines) and `tests/mcp/native_acceptance.py` were not read; they may show intended timeout/cancel behavior beyond what the code establishes.
6. Behavior of `CallAfter` while a native modal dialog runs a nested event loop (whether the tool lambda runs inside it and then hits the `modal_open()` BUSY gate) was inferred from code structure, not observed.
7. `ProjectSeparator` snapshots call `undo_redo_stack().clear()` first (`GUI/Plater.cpp:19637-19638`), resetting the active snapshot time to 0; a revision fingerprint taken before a project replacement could in theory collide after it, but the fingerprint also includes `model().id()` and plate pointers, so a false match looks unlikely. Not verified.
