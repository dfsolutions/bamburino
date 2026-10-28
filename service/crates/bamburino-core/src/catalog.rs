// SPDX-License-Identifier: AGPL-3.0-or-later
//! Embedded parameter catalog: loading, lookup, search and typo suggestions.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

static RAW: &[u8] = include_bytes!("../../../../catalog/catalog.json");
static EMBEDDED: OnceLock<Catalog> = OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("invalid catalog JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Catalog {
    pub studio_version: String,
    pub source_revision: String,
    pub options: BTreeMap<String, OptionDef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OptionDef {
    pub key: String,
    pub kind: OptionKind,
    pub nullable: bool,
    pub scopes: Vec<CatalogScope>,
    pub label: String,
    pub description: String,
    pub units: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub readonly: bool,
    pub enum_values: Vec<String>,
    /// Strings Bambu Studio actually serializes for enum options; empty when
    /// the catalog carries none (then `enum_values` are the serialized forms).
    pub serialized_enum_values: Vec<String>,
    /// Number, bool, list, or a string such as `"400%"`. `None` when the
    /// catalog only carries a `default_expression`; JSON `null` and an absent
    /// field both map to `None`.
    pub default: Option<serde_json::Value>,
    /// True when the catalog marks the option as out of scope (`excluded_reason`).
    pub excluded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionKind {
    Bool,
    Int,
    Float,
    Percent,
    FloatOrPercent,
    String,
    Enum,
    Bools,
    Ints,
    Floats,
    Percents,
    FloatsOrPercents,
    Strings,
    Enums,
    Point,
    Points,
    PointsGroups,
    IntsGroups,
    Other(String),
}

impl OptionKind {
    fn parse(s: &str) -> OptionKind {
        match s {
            "Bool" => Self::Bool,
            "Int" => Self::Int,
            "Float" => Self::Float,
            "Percent" => Self::Percent,
            "FloatOrPercent" => Self::FloatOrPercent,
            "String" => Self::String,
            "Enum" => Self::Enum,
            "Bools" => Self::Bools,
            "Ints" => Self::Ints,
            "Floats" => Self::Floats,
            "Percents" => Self::Percents,
            "FloatsOrPercents" => Self::FloatsOrPercents,
            "Strings" => Self::Strings,
            "Enums" => Self::Enums,
            "Point" => Self::Point,
            "Points" => Self::Points,
            "PointsGroups" => Self::PointsGroups,
            "IntsGroups" => Self::IntsGroups,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Bool => "Bool",
            Self::Int => "Int",
            Self::Float => "Float",
            Self::Percent => "Percent",
            Self::FloatOrPercent => "FloatOrPercent",
            Self::String => "String",
            Self::Enum => "Enum",
            Self::Bools => "Bools",
            Self::Ints => "Ints",
            Self::Floats => "Floats",
            Self::Percents => "Percents",
            Self::FloatsOrPercents => "FloatsOrPercents",
            Self::Strings => "Strings",
            Self::Enums => "Enums",
            Self::Point => "Point",
            Self::Points => "Points",
            Self::PointsGroups => "PointsGroups",
            Self::IntsGroups => "IntsGroups",
            Self::Other(s) => s,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogScope {
    Project,
    Object,
    Volume,
    HeightRange,
    Filament,
    Plate,
}

impl CatalogScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Object => "object",
            Self::Volume => "volume",
            Self::HeightRange => "height_range",
            Self::Filament => "filament",
            Self::Plate => "plate",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub key: String,
    pub label: String,
    pub kind: String,
    pub units: Option<String>,
    pub scopes: Vec<String>,
    pub score: f64,
}

// Wire format. No `deny_unknown_fields`: the generator adds evidence fields.
#[derive(Deserialize)]
struct RawCatalog {
    studio_version: String,
    source_revision: String,
    options: BTreeMap<String, RawOption>,
}

#[derive(Deserialize)]
struct RawOption {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    nullable: bool,
    scopes: Vec<CatalogScope>,
    #[serde(default)]
    label: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    units: Option<String>,
    #[serde(default)]
    min: Option<f64>,
    #[serde(default)]
    max: Option<f64>,
    #[serde(default)]
    readonly: bool,
    #[serde(default)]
    enum_values: Vec<String>,
    #[serde(default)]
    serialized_enum_values: Vec<String>,
    #[serde(default)]
    default: Option<serde_json::Value>,
    #[serde(default)]
    excluded_reason: Option<String>,
}

impl Catalog {
    /// The catalog compiled into the binary, parsed once.
    ///
    /// # Panics
    /// If the embedded JSON is invalid (a build-time defect).
    pub fn embedded() -> &'static Catalog {
        EMBEDDED.get_or_init(|| Catalog::from_json(RAW).expect("embedded catalog.json is invalid"))
    }

    pub fn raw_json() -> &'static [u8] {
        RAW
    }

    pub fn from_json(bytes: &[u8]) -> Result<Catalog, CatalogError> {
        let raw: RawCatalog = serde_json::from_slice(bytes)?;
        let options = raw
            .options
            .into_iter()
            .map(|(key, o)| {
                let def = OptionDef {
                    key: key.clone(),
                    kind: OptionKind::parse(&o.kind),
                    nullable: o.nullable,
                    scopes: o.scopes,
                    label: o.label,
                    description: o.description,
                    units: o.units,
                    min: o.min,
                    max: o.max,
                    readonly: o.readonly,
                    enum_values: o.enum_values,
                    serialized_enum_values: o.serialized_enum_values,
                    default: o.default,
                    excluded: o.excluded_reason.is_some(),
                };
                (key, def)
            })
            .collect();
        Ok(Catalog {
            studio_version: raw.studio_version,
            source_revision: raw.source_revision,
            options,
        })
    }

    pub fn get(&self, key: &str) -> Option<&OptionDef> {
        self.options.get(key)
    }

    /// Case-insensitive search: every whitespace- or underscore-separated token
    /// must match the key, label, description or an enum value (key/label weigh
    /// most). Excluded options (out of the service's scope) are never returned.
    /// Real match weights move the score in steps of at least 0.5; the
    /// description-length tie-break is capped at 0.09 and can never reorder
    /// results that differ in real weight.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        // Keys are matched with `_` as a space, so queries get the same treatment.
        let q = query.to_lowercase().replace('_', " ");
        let tokens: Vec<&str> = q.split_whitespace().collect();
        if tokens.is_empty() || limit == 0 {
            return Vec::new();
        }
        let phrase = tokens.join(" ");
        let mut hits: Vec<SearchHit> = self
            .options
            .values()
            .filter(|o| !o.excluded)
            .filter_map(|o| {
                let key = o.key.to_lowercase().replace('_', " ");
                let label = o.label.to_lowercase();
                let desc = o.description.to_lowercase();
                let enums = o.enum_values.join(" ").to_lowercase();
                let mut score = 0.0;
                for t in &tokens {
                    let mut s = 0.0;
                    if key.contains(t) {
                        s += 3.0;
                    }
                    if label.contains(t) {
                        s += 2.0;
                    }
                    if desc.contains(t) {
                        s += 1.0;
                    }
                    if enums.contains(t) {
                        s += 0.5;
                    }
                    if s == 0.0 {
                        return None;
                    }
                    score += s;
                }
                if key == phrase {
                    score += 10.0;
                }
                if key.contains(&phrase) {
                    score += 5.0;
                }
                if label.contains(&phrase) {
                    score += 3.0;
                }
                // Tie-break: a short description means the option is the
                // primary subject of the match, not an incidental mention.
                score -= (desc.chars().count() as f64 / 10_000.0).min(0.09);
                Some(SearchHit {
                    key: o.key.clone(),
                    label: o.label.clone(),
                    kind: o.kind.as_str().to_string(),
                    units: o.units.clone(),
                    scopes: o.scopes.iter().map(|s| s.as_str().to_string()).collect(),
                    score,
                })
            })
            .collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.key.cmp(&b.key)));
        hits.truncate(limit);
        hits
    }

    /// Up to `n` keys closest to `unknown` (Jaro-Winkler >= 0.75 on key or
    /// label). Excluded options are never suggested.
    pub fn suggest(&self, unknown: &str, n: usize) -> Vec<String> {
        const THRESHOLD: f64 = 0.75;
        let u = unknown.trim().to_lowercase();
        if u.is_empty() || n == 0 {
            return Vec::new();
        }
        let mut scored: Vec<(f64, &str)> = self
            .options
            .values()
            .filter(|o| !o.excluded)
            .filter_map(|o| {
                let by_key = strsim::jaro_winkler(&u, &o.key.to_lowercase());
                let by_label = strsim::jaro_winkler(&u, &o.label.to_lowercase());
                let best = by_key.max(by_label);
                (best >= THRESHOLD).then_some((best, o.key.as_str()))
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        scored
            .into_iter()
            .take(n)
            .map(|(_, k)| k.to_string())
            .collect()
    }
}
