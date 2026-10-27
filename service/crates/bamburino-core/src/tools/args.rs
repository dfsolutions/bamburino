// SPDX-License-Identifier: AGPL-3.0-or-later
//! Argument types of the tools (also the MCP input schemas).

use schemars::JsonSchema;
use serde::Deserialize;

use crate::validate::Scope;

/// Default and maximum number of `catalog_search` hits.
pub const SEARCH_DEFAULT_LIMIT: u32 = 20;
pub const SEARCH_MAX_LIMIT: u32 = 50;

/// Which plate or object a scoped call refers to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetRef {
    /// Plate number as reported by `studio_get_state` (`plates[].plate_id`).
    pub plate_id: Option<u32>,
    /// Object id as reported by `studio_get_state` (`models[].object_id`).
    pub object_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetSettingsArgs {
    /// `project`, `plate` (needs `target.plate_id`) or `object` (needs `target.object_id`).
    pub scope: Scope,
    pub target: Option<TargetRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    /// Words of a setting key, label or description.
    pub query: String,
    /// Maximum number of hits (default 20, maximum 50).
    pub limit: Option<u32>,
}

/// One setting change of `studio_set_settings`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Change {
    /// Setting key, e.g. `sparse_infill_density` (see `catalog_search`).
    pub key: String,
    /// New value in natural form (number, string, bool or array).
    pub value: serde_json::Value,
    /// Defaults to `project`.
    pub scope: Option<Scope>,
    /// Needed for scope `plate` (`plate_id`) and `object` (`object_id`).
    pub target: Option<TargetRef>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetSettingsArgs {
    /// 1 to 100 changes, applied all-or-nothing as far as validation goes.
    pub changes: Vec<Change>,
    /// The `revision` from the last read; a mismatch returns 409 and writes nothing.
    pub expected_revision: u64,
    /// When true, validates and returns the diff without changing anything.
    pub dry_run: Option<bool>,
}
