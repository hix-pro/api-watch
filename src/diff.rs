use std::collections::BTreeMap;

use serde::Serialize;

use crate::spec::{Area, Fact};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Breaking,
    Compatible,
}

impl ChangeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Breaking => "breaking",
            Self::Compatible => "compatible",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Change {
    pub kind: ChangeKind,
    pub area: Area,
    pub label: String,
    pub detail: String,
}

impl Change {
    pub fn is_breaking(&self) -> bool {
        self.kind == ChangeKind::Breaking
    }
}

/// Compare two flattened contracts and classify every difference.
pub fn diff(old: &BTreeMap<String, Fact>, new: &BTreeMap<String, Fact>) -> Vec<Change> {
    let mut changes = Vec::new();

    for (label, old_fact) in old {
        match new.get(label) {
            Some(new_fact) => compare_fact(old_fact, new_fact, &mut changes),
            None => changes.push(Change {
                kind: ChangeKind::Breaking,
                area: old_fact.area,
                label: label.clone(),
                detail: removed_detail(old_fact.area),
            }),
        }
    }

    for (label, new_fact) in new {
        if old.contains_key(label) {
            continue;
        }
        changes.push(Change {
            kind: added_kind(new_fact),
            area: new_fact.area,
            label: label.clone(),
            detail: added_detail(new_fact),
        });
    }

    changes.sort_by(|a, b| {
        b.is_breaking()
            .cmp(&a.is_breaking())
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.detail.cmp(&b.detail))
    });
    changes
}

fn compare_fact(old: &Fact, new: &Fact, changes: &mut Vec<Change>) {
    let mut push = |kind: ChangeKind, detail: String| {
        changes.push(Change {
            kind,
            area: new.area,
            label: new.label.clone(),
            detail,
        });
    };

    if old.type_name != new.type_name {
        push(
            ChangeKind::Breaking,
            format!("type changed from {} to {}", old.type_name, new.type_name),
        );
    }

    if old.required != new.required {
        match new.area {
            Area::Request | Area::Parameter => {
                if new.required {
                    push(ChangeKind::Breaking, "field became required".to_string());
                } else {
                    push(ChangeKind::Compatible, "field became optional".to_string());
                }
            }
            Area::Response => {
                if new.required {
                    push(
                        ChangeKind::Compatible,
                        "field is now guaranteed".to_string(),
                    );
                } else {
                    push(
                        ChangeKind::Breaking,
                        "field is no longer guaranteed".to_string(),
                    );
                }
            }
            Area::Operation | Area::Value => {}
        }
    }

    let removed_enum: Vec<String> = old
        .enum_values
        .iter()
        .filter(|value| !new.enum_values.contains(value))
        .cloned()
        .collect();
    let added_enum: Vec<String> = new
        .enum_values
        .iter()
        .filter(|value| !old.enum_values.contains(value))
        .cloned()
        .collect();

    if old.enum_values.is_empty() && !new.enum_values.is_empty() {
        // Narrowing the accepted input values breaks request authors, while narrowing
        // the values a response can contain is safe for consumers.
        let kind = if new.area == Area::Response {
            ChangeKind::Compatible
        } else {
            ChangeKind::Breaking
        };
        push(
            kind,
            format!(
                "value is now restricted to: {}",
                join_values(&new.enum_values)
            ),
        );
    } else if !old.enum_values.is_empty() && new.enum_values.is_empty() {
        // Removing a restriction accepts more input (safe) but allows new response
        // values to appear (potentially unsafe for exhaustive clients).
        let (kind, detail) = if new.area == Area::Response {
            (
                ChangeKind::Breaking,
                "enum restriction removed: any value may now be returned".to_string(),
            )
        } else {
            (
                ChangeKind::Compatible,
                "enum restriction removed: any value is now accepted".to_string(),
            )
        };
        push(kind, detail);
    } else {
        if !removed_enum.is_empty() {
            let (kind, detail) = if new.area == Area::Response {
                (
                    ChangeKind::Compatible,
                    format!(
                        "enum value no longer returned: {}",
                        join_values(&removed_enum)
                    ),
                )
            } else {
                (
                    ChangeKind::Breaking,
                    format!("enum value removed: {}", join_values(&removed_enum)),
                )
            };
            push(kind, detail);
        }
        if !added_enum.is_empty() {
            let (kind, detail) = if new.area == Area::Response {
                (
                    ChangeKind::Breaking,
                    format!(
                        "new enum value may be returned: {}",
                        join_values(&added_enum)
                    ),
                )
            } else {
                (
                    ChangeKind::Compatible,
                    format!("enum value added: {}", join_values(&added_enum)),
                )
            };
            push(kind, detail);
        }
    }

    if old.format != new.format {
        match (&old.format, &new.format) {
            (None, Some(format)) => {
                let kind = if new.area == Area::Response {
                    ChangeKind::Compatible
                } else {
                    ChangeKind::Breaking
                };
                push(kind, format!("format added: {format}"));
            }
            (Some(format), None) => {
                let kind = if new.area == Area::Response {
                    ChangeKind::Breaking
                } else {
                    ChangeKind::Compatible
                };
                push(kind, format!("format restriction removed: {format}"));
            }
            (Some(old_format), Some(new_format)) => push(
                ChangeKind::Breaking,
                format!("format changed from {old_format} to {new_format}"),
            ),
            (None, None) => unreachable!(),
        }
    }
}

fn added_kind(fact: &Fact) -> ChangeKind {
    match fact.area {
        Area::Parameter | Area::Request if fact.required => ChangeKind::Breaking,
        _ => ChangeKind::Compatible,
    }
}

fn removed_detail(area: Area) -> String {
    match area {
        Area::Operation => "endpoint removed",
        Area::Parameter => "parameter removed",
        Area::Request => "request property removed",
        Area::Response => "response property removed",
        Area::Value => "value removed",
    }
    .to_string()
}

fn added_detail(fact: &Fact) -> String {
    match fact.area {
        Area::Operation => "endpoint added".to_string(),
        Area::Parameter if fact.required => "new required parameter".to_string(),
        Area::Parameter => "new optional parameter".to_string(),
        Area::Request if fact.required => "new required request property".to_string(),
        Area::Request => "new optional request property".to_string(),
        Area::Response => "response property added".to_string(),
        Area::Value => "value added".to_string(),
    }
}

fn join_values(values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::spec::{Format, flatten};

    fn base_spec() -> Value {
        json!({
            "openapi": "3.0.0",
            "info": { "title": "Pets", "version": "1" },
            "paths": {
                "/pets": {
                    "post": {
                        "requestBody": {
                            "required": true,
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "required": ["name"],
                                        "properties": {
                                            "name": { "type": "string" },
                                            "tag": { "type": "string" }
                                        }
                                    }
                                }
                            }
                        },
                        "responses": {
                            "200": {
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "type": "object",
                                            "required": ["id"],
                                            "properties": {
                                                "id": { "type": "integer" },
                                                "status": {
                                                    "type": "string",
                                                    "enum": ["available", "sold"]
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    fn facts(spec: &Value) -> BTreeMap<String, Fact> {
        flatten(spec, Format::OpenApi).unwrap().facts
    }

    #[test]
    fn removing_an_endpoint_is_breaking() {
        let old = facts(&base_spec());
        let mut spec = base_spec();
        spec["paths"] = json!({});
        let changes = diff(&old, &facts(&spec));
        assert!(changes.iter().any(|change| {
            change.is_breaking()
                && change.label == "POST /pets"
                && change.detail == "endpoint removed"
        }));
    }

    #[test]
    fn adding_a_required_request_property_is_breaking() {
        let old = facts(&base_spec());
        let mut spec = base_spec();
        spec["paths"]["/pets"]["post"]["requestBody"]["content"]["application/json"]["schema"]["properties"]
            ["color"] = json!({ "type": "string" });
        spec["paths"]["/pets"]["post"]["requestBody"]["content"]["application/json"]["schema"]["required"] =
            json!(["name", "color"]);
        let changes = diff(&old, &facts(&spec));
        assert!(changes.iter().any(|change| {
            change.is_breaking()
                && change.label.ends_with("body application/json.color")
                && change.detail == "new required request property"
        }));
    }

    #[test]
    fn adding_an_optional_request_property_is_compatible() {
        let old = facts(&base_spec());
        let mut spec = base_spec();
        spec["paths"]["/pets"]["post"]["requestBody"]["content"]["application/json"]["schema"]["properties"]
            ["color"] = json!({ "type": "string" });
        let changes = diff(&old, &facts(&spec));
        assert!(changes.iter().any(|change| {
            change.kind == ChangeKind::Compatible
                && change.label.ends_with("body application/json.color")
        }));
        assert!(!changes.iter().any(Change::is_breaking));
    }

    #[test]
    fn removing_a_response_property_is_breaking() {
        let old = facts(&base_spec());
        let mut spec = base_spec();
        spec["paths"]["/pets"]["post"]["responses"]["200"]["content"]["application/json"]["schema"]
            ["properties"]
            .as_object_mut()
            .unwrap()
            .remove("id");
        let changes = diff(&old, &facts(&spec));
        assert!(changes.iter().any(|change| {
            change.is_breaking() && change.label.ends_with("200 application/json.id")
        }));
    }

    #[test]
    fn response_field_becoming_optional_is_breaking() {
        let old = facts(&base_spec());
        let mut spec = base_spec();
        spec["paths"]["/pets"]["post"]["responses"]["200"]["content"]["application/json"]["schema"]
            ["required"] = json!([]);
        let changes = diff(&old, &facts(&spec));
        assert!(changes.iter().any(|change| {
            change.is_breaking()
                && change.label.ends_with("200 application/json.id")
                && change.detail == "field is no longer guaranteed"
        }));
    }

    #[test]
    fn response_enum_addition_is_breaking_and_removal_is_compatible() {
        let old = facts(&base_spec());
        let mut spec = base_spec();
        spec["paths"]["/pets"]["post"]["responses"]["200"]["content"]["application/json"]["schema"]
            ["properties"]["status"]["enum"] = json!(["available", "pending"]);
        let changes = diff(&old, &facts(&spec));
        assert!(changes.iter().any(|change| {
            change.kind == ChangeKind::Compatible
                && change.label.ends_with("status")
                && change.detail.contains("sold")
        }));
        assert!(changes.iter().any(|change| {
            change.is_breaking()
                && change.label.ends_with("status")
                && change.detail.contains("pending")
        }));
    }

    #[test]
    fn request_enum_removal_is_breaking_and_addition_is_compatible() {
        let mut before = base_spec();
        before["paths"]["/pets"]["post"]["requestBody"]["content"]["application/json"]["schema"]
            ["properties"]["tag"] = json!({ "type": "string", "enum": ["cat", "dog"] });
        let old = facts(&before);
        let mut after = before.clone();
        after["paths"]["/pets"]["post"]["requestBody"]["content"]["application/json"]["schema"]["properties"]
            ["tag"]["enum"] = json!(["cat", "bird"]);
        let changes = diff(&old, &facts(&after));
        assert!(changes.iter().any(|change| {
            change.is_breaking()
                && change.label.ends_with("body application/json.tag")
                && change.detail.contains("dog")
        }));
        assert!(changes.iter().any(|change| {
            change.kind == ChangeKind::Compatible
                && change.label.ends_with("body application/json.tag")
                && change.detail.contains("bird")
        }));
    }

    #[test]
    fn changing_a_type_is_breaking() {
        let old = facts(&base_spec());
        let mut spec = base_spec();
        spec["paths"]["/pets"]["post"]["requestBody"]["content"]["application/json"]["schema"]["properties"]
            ["name"] = json!({ "type": "integer" });
        let changes = diff(&old, &facts(&spec));
        assert!(changes.iter().any(|change| {
            change.is_breaking()
                && change.label.ends_with("body application/json.name")
                && change.detail == "type changed from string to integer"
        }));
    }

    #[test]
    fn generic_json_removal_is_breaking() {
        let old_value = json!({ "data": { "id": 1, "name": "Ada" } });
        let new_value = json!({ "data": { "id": 1 } });
        let old = flatten(&old_value, Format::Json).unwrap().facts;
        let new = flatten(&new_value, Format::Json).unwrap().facts;
        let changes = diff(&old, &new);
        assert!(changes.iter().any(|change| {
            change.is_breaking() && change.label == "$.data.name" && change.area == Area::Value
        }));
    }
}
