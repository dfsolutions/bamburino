# Tool reference

The 12 tools in the order of `Tools::NAMES` in the service. Input fields come from `service/crates/bamburino-core/src/tools/args.rs` (unknown fields are rejected), result fields from `read.rs`, `mutate.rs` and `slice.rs` in the same folder. Where no real captured output exists, the example says "example pending verification" instead of showing numbers; none of the outputs below has been recorded as `verified` yet (see the README, Verification status).

## Common behavior

- Every tool returns a JSON object. A failure returns `{"error": {"code", "message", "http_status", "details"}}` (`errors.rs`).
- Tools that talk to Studio (everything except `bamburino_capabilities` and `catalog_search`) can return these codes whenever Studio is not ready: `STUDIO_NOT_RUNNING`, `DRIVER_DISABLED`, `DRIVER_UNAUTHORIZED`, `DRIVER_TOKEN_MISSING`, `DRIVER_ERROR`. Mutations can also return `VERSION_MISMATCH`, `BUSY` and `REVISION_CONFLICT`. A `BUSY` caused by an unresponsive driver carries `details.reason` `driver_timeout` (no answer in time) or `driver_busy` (the driver reported itself busy). Bad input returns `INVALID_ARGUMENT`.
- Mutations (`studio_set_settings`, `studio_transform_model`, `studio_set_supports`) take `expected_revision`, the `revision` of your last read. On a mismatch nothing is written and `REVISION_CONFLICT` carries `details.reason`, `details.expected`, `details.current` and `details.state`. Slicing tools do not take a revision.
- `scope` is `project`, `plate` or `object`. `plate` needs `plate_id`, `object` needs `object_id` (ids as returned by `studio_get_state` and `studio_list_models`).
- Meaning of every error code: README, Errors table.

## bamburino_capabilities

Purpose: status of Bamburino, the catalog and the driver. Works with Studio closed.

Input: none (`{}`).

Result fields: `bamburino_version`; `catalog` (`studio_version`, `source_revision`, `keys`); `driver` (`status`, which is one of `ready`, `not_running`, `driver_off`, `unauthorized`, `token_missing`, `busy`, `driver_error`; when `ready` also `app_version`, `build_suffix`, `patch_id`, `version_match`; always `expected_studio_version` and `expected_patch_id`; `message` on `busy` and `driver_error`; `busy` means the driver did not answer in time or reported itself busy, typically because a modal dialog is open in Studio); `tools` (the 12 names); `physical_tools` (always `false`).

Errors: none specific; it reports problems in `driver.status` instead of failing.

Example request: `{}`. Example output: example pending verification.

## studio_get_state

Purpose: the open project and the `revision` that mutations need.

Input: none (`{}`).

Result fields: `instance` (`session_id`, `app_version`; `session_id` changes when Studio restarts); `project` (`name`, `path`, `dirty`, `presets_dirty`); `plates[]` (`plate_id`, `name`, `selected`, `locked`); `models` (objects with instances); `presets` (`printer`, `process`, `filaments`); `slice` (`running`, `valid`, `slice_result_id`); `busy` (`modal_open`, `ui_job_running`); `revision`. The revision is an integer of at most 2^53 - 1 (JavaScript-safe), it only grows while the service runs, and it takes new values after a service restart: a revision read before a restart does not match afterwards.

Errors: the Studio-not-ready set.

Example request: `{}`. Example output: example pending verification.

## studio_get_settings

Purpose: effective settings of the project, a plate or an object.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `scope` | `project` \| `plate` \| `object` | yes | Which level to read. |
| `target.plate_id` | integer | for `plate` | Plate number from `studio_get_state`. |
| `target.object_id` | string | for `object` | Object id from `studio_list_models`. |

Result fields: `scope`, `target`, `revision`, `settings[]`, each with `key`, `value`, `type`, `units`, `enum`, `min`, `max`, `origin`, `writable_by_driver` (type, units and range come from the catalog and may be null).

Errors: `INVALID_ARGUMENT` (missing target), `PLATE_NOT_FOUND`, `OBJECT_NOT_FOUND`, plus the Studio-not-ready set.

Example request:
```json
{ "scope": "object", "target": { "object_id": "<id from studio_list_models>" } }
```
Example output: example pending verification.

## studio_set_settings

Purpose: change settings by key. All changes are validated against the catalog before any write; `dry_run` stops after validation.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `changes` | array of 1 to 100 | yes | Each: `key` (string, required), `value` (number, string, bool or array, required), `scope` (default `project`), `target` (`plate_id` / `object_id`, needed for `plate` / `object`). |
| `expected_revision` | integer | yes | `revision` from the last read. |
| `dry_run` | boolean | no | Validate and return the diff without writing. |

Result fields: `applied[]` (`key`, `scope`, `target`, `requested`, `applied`), `diff[]` (`key`, `scope`, `target`, `from`, `to`, `changed`), `revision`, `dry_run`.

Errors: `UNKNOWN_KEY`, `INVALID_VALUE`, `SCOPE_NOT_ALLOWED`, `NOT_WRITABLE`, `NOT_WRITABLE_BY_DRIVER` (these list each failing change under `details.errors` with its `index`), `REVISION_CONFLICT`, `BUSY`, `VERSION_MISMATCH`, `INVALID_ARGUMENT`, `PLATE_NOT_FOUND`, `OBJECT_NOT_FOUND`, `PARTIALLY_APPLIED` (`details.applied`, `details.failed`, `details.not_attempted`, `details.revision`), `DRIVER_ERROR`. When the outcome of a write is unknown, `details.outcome` says so (`applied_unverified`, `not_applied` or `unknown`).

Example request:
```json
{
  "expected_revision": 0,
  "changes": [
    { "key": "sparse_infill_density", "value": "35%" },
    { "key": "wall_loops", "value": 4 }
  ],
  "dry_run": true
}
```
(`0` stands for a real revision from `studio_get_state`.) Example output: example pending verification.

## studio_list_models

Purpose: objects and instances of the project.

Input: none (`{}`).

Result fields: `models[]` (`object_id`, `name`, `instances[]` with `instance_id`, `plate_id`, `position_mm`, `rotation_deg`, `scale`), `revision`.

Errors: the Studio-not-ready set.

Example request: `{}`. Example output: example pending verification.

## studio_transform_model

Purpose: rotate, translate or scale one object instance.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `object_id` | string | yes | From `studio_list_models`. |
| `instance_id` | string | no | Default: the first instance. |
| `transform.rotate_deg` | `[x, y, z]` numbers | one of the three | Degrees. |
| `transform.translate_mm` | `[x, y, z]` numbers | one of the three | Millimetres; offset (relative) or position (absolute). |
| `transform.scale` | `[x, y, z]` numbers > 0 | one of the three | Multiplied (relative) or replaced (absolute). |
| `transform.mode` | `relative` \| `absolute` | no | Default `relative`. |
| `expected_revision` | integer | yes | `revision` from the last read. |

Result fields: `object_id`, `instance_id`, `before`, `after`, `reread` (each with `position_mm`, `rotation_deg`, `scale`), `revision`. Re-read rotation angles are normalized (Z 270 reads as -90).

Errors: `OBJECT_NOT_FOUND`, `INVALID_ARGUMENT`, `REVISION_CONFLICT`, `BUSY`, `VERSION_MISMATCH`, `DRIVER_ERROR` (`details.outcome` may be `applied_unverified`).

Example request:
```json
{
  "expected_revision": 0,
  "object_id": "<id>",
  "transform": { "rotate_deg": [0, 0, 90], "mode": "relative" }
}
```
Example output: example pending verification.

## studio_set_supports

Purpose: enable or configure supports for the project, a plate or an object. It is translated into `studio_set_settings` changes (`enable_support`, `support_type`, `support_threshold_angle`, `support_on_build_plate_only`) and runs through the same pipeline. A plate target is a plate override (scope `plate`), written with the plate's settings override, not a set of per-object overrides.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `target.scope` | `project` \| `plate` \| `object` | yes | Where to apply. |
| `target.plate_id` / `target.object_id` | integer / string | for `plate` / `object` | Ids. |
| `settings.enabled` | boolean | yes | Supports on or off. |
| `settings.type` | `normal` \| `tree` | no | With `style` it sets `support_type`. |
| `settings.style` | `auto` \| `manual` | no | Default `auto`; needs `type`. |
| `settings.threshold_angle` | integer | no | Overhang angle. |
| `settings.on_build_plate_only` | boolean | no | Supports only from the plate. |
| `expected_revision` | integer | yes | `revision` from the last read. |

Result fields: the same as `studio_set_settings` (`applied`, `diff`, `revision`, `dry_run`).

Errors: those of `studio_set_settings`; `INVALID_ARGUMENT` when `style` is given without `type` or when the target id is missing.

Example request:
```json
{
  "expected_revision": 0,
  "target": { "scope": "object", "object_id": "<id>" },
  "settings": { "enabled": true, "type": "tree", "style": "auto" }
}
```
Example output: example pending verification.

## studio_slice

Purpose: start slicing a plate in Studio. It does not modify the project settings, but when `plate_id` is not the selected plate it calls `plate_select` first, which changes the plate selection in Studio and takes an undo snapshot.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `plate_id` | integer | no | Default: the selected plate; another plate is selected first. |

Result fields: `job_id`, `plate_id`, `status`.

Errors: `PLATE_NOT_FOUND`, `BUSY` (`details.reason`), `REVISION_CONFLICT` (`details.reason` `plate_selection_changed`), `VERSION_MISMATCH`, `DRIVER_ERROR`.

Example request: `{ "plate_id": 1 }`. Example output: example pending verification.

## studio_slice_status

Purpose: progress of a slice job.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `job_id` | string | yes | From `studio_slice`. |

Result fields: `job_id`, `plate_id`, `status`, `progress_percent`, `message`, `error` (`code`, `message`), `slice_result_id`; `details.raw_status` when the driver status is not recognized. A result that Studio has replaced is reported with the error code `STALE_RESULT` inside `error`.

Errors: `JOB_NOT_FOUND`, `INVALID_ARGUMENT`, `DRIVER_ERROR`.

Example request: `{ "job_id": "<id>" }`. Example output: example pending verification.

## studio_cancel_slice

Purpose: cancel a running slice job.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `job_id` | string | yes | From `studio_slice`. |

Result fields: `job_id`, `status`, `cancel_requested`, and `message` when the job had already finished.

Errors: `JOB_NOT_FOUND`, `INVALID_ARGUMENT`, `DRIVER_ERROR`.

Example request: `{ "job_id": "<id>" }`. Example output: example pending verification.

## studio_get_slice_summary

Purpose: time, filament and warnings of the current slice result of a plate.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `plate_id` | integer | no | Default: the selected plate, the only one that can hold a current result. |

Result fields: `plate_id`, `print_time_s` (from Studio's estimate string), `filaments[]` (`index`, `grams`, `meters`, `volume_cm3`; from the header of a temporary exported G-code), `support_volume_mm3`, `warnings`, `note`, `revision`.

Errors: `PLATE_NOT_FOUND`, `INVALID_ARGUMENT`, `BUSY`, `DRIVER_ERROR`.

Example request: `{}`. Example output: example pending verification.

## catalog_search

Purpose: find setting keys by words of key, label or description. Works with Studio closed.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `query` | string | yes | Words to look for. |
| `limit` | integer | no | Default 20, maximum 50. |

Result fields: `query`, `hits[]` (`key`, `label`, `kind`, `units`, `scopes`, `score`, `writable_by_driver`).

Errors: `INVALID_ARGUMENT`.

Example request:
```json
{ "query": "seam position", "limit": 5 }
```
Example output: example pending verification.
