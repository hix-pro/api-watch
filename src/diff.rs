use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::spec::{Area, Fact, FactKind};

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
    let removed: Vec<(&String, &Fact)> = old
        .iter()
        .filter(|(label, _)| !new.contains_key(*label))
        .collect();
    let added: Vec<(&String, &Fact)> = new
        .iter()
        .filter(|(label, _)| !old.contains_key(*label))
        .collect();
    let removed_labels: BTreeSet<&str> = removed.iter().map(|(label, _)| label.as_str()).collect();
    let added_labels: BTreeSet<&str> = added.iter().map(|(label, _)| label.as_str()).collect();

    let mut changes = Vec::new();

    for (label, old_fact) in old {
        match new.get(label) {
            Some(new_fact) => compare_fact(old_fact, new_fact, &mut changes),
            None => {
                // Removing an endpoint, response, or body removes all of its
                // children. Report the owning fact instead of every descendant.
                if has_ancestor(label, old, &removed_labels) {
                    continue;
                }
                changes.push(Change {
                    kind: ChangeKind::Breaking,
                    area: old_fact.area,
                    label: label.clone(),
                    detail: removed_detail(old_fact),
                });
            }
        }
    }

    for (label, new_fact) in added {
        // A new endpoint brings its whole contract with it; its required
        // parameters are not a breaking change to existing clients.
        if has_ancestor(label, new, &added_labels) {
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

/// True when `child` is a fact that belongs to the removed/added `parent`.
fn has_ancestor(label: &str, facts: &BTreeMap<String, Fact>, candidates: &BTreeSet<&str>) -> bool {
    label.char_indices().any(|(index, character)| {
        index > 0
            && matches!(character, ' ' | '.' | '[')
            && candidates.contains(&label[..index])
            && facts
                .get(&label[..index])
                .is_some_and(|parent| is_descendant(parent, &label[..index], label))
    })
}

fn is_descendant(parent: &Fact, parent_label: &str, child: &str) -> bool {
    let Some(rest) = child.strip_prefix(parent_label) else {
        return false;
    };
    match parent.kind {
        FactKind::Operation => rest.starts_with(" · "),
        FactKind::Response | FactKind::RequestBody => {
            rest.starts_with(' ') || rest.starts_with('.')
        }
        FactKind::RequestBodyContent | FactKind::ResponseContent => {
            rest.starts_with('.') || rest.starts_with('[')
        }
        FactKind::Parameter | FactKind::ResponseHeader | FactKind::Field => {
            rest.starts_with('.') || rest.starts_with('[')
        }
    }
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
    match fact.kind {
        // A new media type is an additive capability; clients can keep using
        // the media types that already existed.
        FactKind::RequestBodyContent | FactKind::ResponseContent => ChangeKind::Compatible,
        _ => match fact.area {
            Area::Parameter | Area::Request if fact.required => ChangeKind::Breaking,
            _ => ChangeKind::Compatible,
        },
    }
}

fn removed_detail(fact: &Fact) -> String {
    match fact.kind {
        FactKind::Operation => "endpoint removed",
        FactKind::Parameter => "parameter removed",
        FactKind::RequestBody => "request body removed",
        FactKind::RequestBodyContent => "request content removed",
        FactKind::Response => "response removed",
        FactKind::ResponseContent => "response content removed",
        FactKind::ResponseHeader => "response header removed",
        FactKind::Field => match fact.area {
            Area::Operation => "endpoint removed",
            Area::Parameter => "parameter property removed",
            Area::Request => "request property removed",
            Area::Response => "response property removed",
            Area::Value => "value removed",
        },
    }
    .to_string()
}

fn added_detail(fact: &Fact) -> String {
    match fact.kind {
        FactKind::Operation => "endpoint added".to_string(),
        FactKind::Parameter if fact.required => "new required parameter".to_string(),
        FactKind::Parameter => "new optional parameter".to_string(),
        FactKind::RequestBody if fact.required => "new required request body".to_string(),
        FactKind::RequestBody => "new optional request body".to_string(),
        FactKind::RequestBodyContent => "request content added".to_string(),
        FactKind::Response => "response added".to_string(),
        FactKind::ResponseContent => "response content added".to_string(),
        FactKind::ResponseHeader => "response header added".to_string(),
        FactKind::Field => match fact.area {
            Area::Operation => "endpoint added".to_string(),
            Area::Parameter if fact.required => "new required parameter property".to_string(),
            Area::Parameter => "new optional parameter property".to_string(),
            Area::Request if fact.required => "new required request property".to_string(),
            Area::Request => "new optional request property".to_string(),
            Area::Response => "response property added".to_string(),
            Area::Value => "value added".to_string(),
        },
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
        assert_eq!(changes.len(), 1);
        assert!(changes.iter().any(|change| {
            change.is_breaking()
                && change.label == "POST /pets"
                && change.detail == "endpoint removed"
        }));
    }

    #[test]
    fn adding_an_endpoint_with_required_parameters_is_compatible() {
        let old = facts(&json!({
            "openapi": "3.0.0",
            "info": { "title": "Pets", "version": "1" },
            "paths": {}
        }));
        let new = facts(&json!({
            "openapi": "3.0.0",
            "info": { "title": "Pets", "version": "1" },
            "paths": {
                "/new": {
                    "get": {
                        "parameters": [
                            {
                                "name": "token",
                                "in": "query",
                                "required": true,
                                "schema": { "type": "string" }
                            }
                        ],
                        "responses": {
                            "200": {
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "type": "object",
                                            "required": ["id"],
                                            "properties": { "id": { "type": "integer" } }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }));

        let changes = diff(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Compatible);
        assert_eq!(changes[0].label, "GET /new");
        assert_eq!(changes[0].detail, "endpoint added");
    }

    #[test]
    fn removing_a_response_suppresses_its_children() {
        let old = facts(&base_spec());
        let mut spec = base_spec();
        spec["paths"]["/pets"]["post"]["responses"]
            .as_object_mut()
            .unwrap()
            .remove("200");
        let changes = diff(&old, &facts(&spec));
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].label, "POST /pets · 200");
        assert_eq!(changes[0].detail, "response removed");
    }

    #[test]
    fn removing_a_request_body_reports_one_change() {
        let old = facts(&base_spec());
        let mut spec = base_spec();
        spec["paths"]["/pets"]["post"]
            .as_object_mut()
            .unwrap()
            .remove("requestBody");
        let changes = diff(&old, &facts(&spec));
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].label, "POST /pets · body");
        assert_eq!(changes[0].detail, "request body removed");
    }

    #[test]
    fn response_headers_have_their_own_labels() {
        let mut with_header = base_spec();
        with_header["paths"]["/pets"]["post"]["responses"]["200"]["headers"] = json!({
            "X-Rate-Limit": { "schema": { "type": "integer" } }
        });
        let old = facts(&with_header);

        let mut without_header = with_header.clone();
        without_header["paths"]["/pets"]["post"]["responses"]["200"]
            .as_object_mut()
            .unwrap()
            .remove("headers");
        let removed = diff(&old, &facts(&without_header));
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].label, "POST /pets · 200 header X-Rate-Limit");
        assert_eq!(removed[0].detail, "response header removed");

        let added = diff(&facts(&base_spec()), &old);
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].kind, ChangeKind::Compatible);
        assert_eq!(added[0].detail, "response header added");
    }

    #[test]
    fn adding_request_content_type_is_compatible() {
        let old = facts(&base_spec());
        let mut spec = base_spec();
        spec["paths"]["/pets"]["post"]["requestBody"]["content"]["application/xml"] =
            spec["paths"]["/pets"]["post"]["requestBody"]["content"]["application/json"].clone();
        let changes = diff(&old, &facts(&spec));
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Compatible);
        assert_eq!(changes[0].label, "POST /pets · body application/xml");
        assert_eq!(changes[0].detail, "request content added");
    }

    #[test]
    fn adding_response_content_type_is_compatible() {
        let old = facts(&base_spec());
        let mut spec = base_spec();
        spec["paths"]["/pets"]["post"]["responses"]["200"]["content"]["text/xml"] =
            spec["paths"]["/pets"]["post"]["responses"]["200"]["content"]["application/json"]
                .clone();
        let changes = diff(&old, &facts(&spec));
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Compatible);
        assert_eq!(changes[0].label, "POST /pets · 200 text/xml");
        assert_eq!(changes[0].detail, "response content added");
    }

    #[test]
    fn removing_a_content_type_reports_one_change() {
        let mut with_content = base_spec();
        with_content["paths"]["/pets"]["post"]["requestBody"]["content"]["application/xml"] =
            with_content["paths"]["/pets"]["post"]["requestBody"]["content"]["application/json"]
                .clone();
        let old = facts(&with_content);
        let changes = diff(&old, &facts(&base_spec()));
        assert_eq!(changes.len(), 1);
        assert!(changes[0].is_breaking());
        assert_eq!(changes[0].label, "POST /pets · body application/xml");
        assert_eq!(changes[0].detail, "request content removed");
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
