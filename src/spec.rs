use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Area {
    Operation,
    Parameter,
    Request,
    Response,
    Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FactKind {
    #[default]
    Field,
    Operation,
    Parameter,
    RequestBody,
    RequestBodyContent,
    Response,
    ResponseContent,
    ResponseHeader,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Fact {
    pub area: Area,
    pub label: String,
    pub type_name: String,
    pub required: bool,
    #[serde(default)]
    pub kind: FactKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    OpenApi,
    Json,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenApi => "openapi",
            Self::Json => "json",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OpenApi => "OpenAPI",
            Self::Json => "JSON",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Flattened {
    pub facts: BTreeMap<String, Fact>,
    pub format: Format,
    pub operations: usize,
    pub title: Option<String>,
    pub unresolved_refs: usize,
}

impl Flattened {
    pub fn fact_count(&self) -> usize {
        self.facts.len()
    }
}

const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// Parse a spec from JSON or YAML and detect whether it is an OpenAPI document.
pub fn parse_document(body: &str) -> Result<(Value, Format)> {
    let trimmed = body.trim_start_matches('\u{feff}').trim();
    if trimmed.is_empty() {
        bail!("the fetched document is empty");
    }

    let value = match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => value,
        Err(json_error) => match serde_yaml_ng::from_str::<Value>(trimmed) {
            Ok(value) => value,
            Err(yaml_error) => {
                bail!("could not parse the document as JSON ({json_error}) or YAML ({yaml_error})")
            }
        },
    };

    if !value.is_object() && !value.is_array() {
        bail!(
            "the fetched document is not a JSON object or array (it looks like plain text, HTML, or another format)"
        );
    }

    let format = detect_format(&value)?;
    Ok((value, format))
}

pub fn detect_format(value: &Value) -> Result<Format> {
    if value.get("openapi").is_some() || value.get("swagger").is_some() {
        if value.get("paths").is_some_and(Value::is_object) {
            return Ok(Format::OpenApi);
        }
        bail!("the document declares OpenAPI but has no 'paths' object");
    }
    Ok(Format::Json)
}

/// Turn a parsed document into a flat, comparable set of contract facts.
pub fn flatten(value: &Value, format: Format) -> Result<Flattened> {
    match format {
        Format::OpenApi => flatten_openapi(value),
        Format::Json => Ok(flatten_json(value)),
    }
}

fn flatten_openapi(root: &Value) -> Result<Flattened> {
    let paths = root
        .get("paths")
        .and_then(Value::as_object)
        .context("OpenAPI document has no 'paths' object")?;

    let mut facts = BTreeMap::new();
    let mut unresolved_refs = 0usize;
    let mut operations = 0usize;
    let mut paths: Vec<_> = paths.iter().collect();
    paths.sort_by_key(|(path, _)| *path);

    for (path, item) in paths {
        let item = resolve(root, item, 0, &mut unresolved_refs);
        let Some(item_obj) = item.as_object() else {
            continue;
        };

        let path_parameters =
            collect_parameters(root, [item_obj.get("parameters")], &mut unresolved_refs);

        for method in METHODS {
            let Some(operation) = item_obj.get(method) else {
                continue;
            };
            let operation = resolve(root, operation, 0, &mut unresolved_refs);
            let Some(operation_obj) = operation.as_object() else {
                continue;
            };

            operations += 1;
            let label = format!("{} {}", method.to_uppercase(), path);
            insert_fact(
                &mut facts,
                Fact {
                    area: Area::Operation,
                    label: label.clone(),
                    type_name: "endpoint".to_string(),
                    required: false,
                    kind: FactKind::Operation,
                    enum_values: Vec::new(),
                    format: None,
                },
            );

            let mut parameters = path_parameters.clone();
            parameters.extend(collect_parameters(
                root,
                [operation_obj.get("parameters")],
                &mut unresolved_refs,
            ));

            for parameter in parameters.values() {
                let Some(parameter_obj) = parameter.as_object() else {
                    continue;
                };
                let location = parameter_obj
                    .get("in")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let name = parameter_obj
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("");

                if location == "body" {
                    let body_label = format!("{label} · body");
                    let required = parameter_obj
                        .get("required")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    insert_fact(
                        &mut facts,
                        Fact {
                            area: Area::Request,
                            label: body_label.clone(),
                            type_name: "body".to_string(),
                            required,
                            kind: FactKind::RequestBody,
                            enum_values: Vec::new(),
                            format: None,
                        },
                    );
                    if let Some(schema) = parameter_schema(parameter_obj) {
                        for media in media_types(root, operation_obj, "consumes") {
                            flatten_content(
                                root,
                                &schema,
                                &format!("{body_label} {media}"),
                                Area::Request,
                                FactKind::RequestBodyContent,
                                &mut facts,
                                &mut unresolved_refs,
                            );
                        }
                    }
                    continue;
                }

                if name.is_empty() {
                    continue;
                }

                let required = parameter_obj
                    .get("required")
                    .and_then(Value::as_bool)
                    .unwrap_or(location == "path");
                let parameter_label = format!("{label} · {location} {name}");
                let schema = parameter_schema(parameter_obj);

                match schema {
                    Some(schema) => {
                        flatten_schema(
                            root,
                            &schema,
                            &parameter_label,
                            Area::Parameter,
                            required,
                            &mut facts,
                            &mut unresolved_refs,
                            0,
                        );
                        if let Some(fact) = facts.get_mut(&parameter_label) {
                            fact.kind = FactKind::Parameter;
                        }
                    }
                    None => insert_fact(
                        &mut facts,
                        Fact {
                            area: Area::Parameter,
                            label: parameter_label,
                            type_name: "any".to_string(),
                            required,
                            kind: FactKind::Parameter,
                            enum_values: Vec::new(),
                            format: None,
                        },
                    ),
                }
            }

            handle_request_body(
                root,
                operation_obj,
                &label,
                &mut facts,
                &mut unresolved_refs,
            );
            handle_responses(
                root,
                operation_obj,
                &label,
                &mut facts,
                &mut unresolved_refs,
            );
        }
    }

    Ok(Flattened {
        facts,
        format: Format::OpenApi,
        operations,
        title: document_title(root),
        unresolved_refs,
    })
}

fn handle_request_body(
    root: &Value,
    operation: &Map<String, Value>,
    operation_label: &str,
    facts: &mut BTreeMap<String, Fact>,
    unresolved_refs: &mut usize,
) {
    let Some(request_body) = operation.get("requestBody") else {
        return;
    };
    let request_body = resolve(root, request_body, 0, unresolved_refs);
    let Some(request_body_obj) = request_body.as_object() else {
        return;
    };

    let required = request_body_obj
        .get("required")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let body_label = format!("{operation_label} · body");
    insert_fact(
        facts,
        Fact {
            area: Area::Request,
            label: body_label.clone(),
            type_name: "body".to_string(),
            required,
            kind: FactKind::RequestBody,
            enum_values: Vec::new(),
            format: None,
        },
    );

    if let Some(content) = request_body_obj.get("content").and_then(Value::as_object) {
        for (media, media_obj) in content {
            if let Some(schema) = media_obj.get("schema") {
                flatten_content(
                    root,
                    schema,
                    &format!("{body_label} {media}"),
                    Area::Request,
                    FactKind::RequestBodyContent,
                    facts,
                    unresolved_refs,
                );
            }
        }
    }
}

fn handle_responses(
    root: &Value,
    operation: &Map<String, Value>,
    operation_label: &str,
    facts: &mut BTreeMap<String, Fact>,
    unresolved_refs: &mut usize,
) {
    let Some(responses) = operation.get("responses").and_then(Value::as_object) else {
        return;
    };

    for (status, response) in responses {
        let response = resolve(root, response, 0, unresolved_refs);
        let Some(response_obj) = response.as_object() else {
            continue;
        };

        let status_label = format!("{operation_label} · {status}");
        insert_fact(
            facts,
            Fact {
                area: Area::Response,
                label: status_label.clone(),
                type_name: "response".to_string(),
                required: true,
                kind: FactKind::Response,
                enum_values: Vec::new(),
                format: None,
            },
        );

        if let Some(content) = response_obj.get("content").and_then(Value::as_object) {
            for (media, media_obj) in content {
                if let Some(schema) = media_obj.get("schema") {
                    flatten_content(
                        root,
                        schema,
                        &format!("{status_label} {media}"),
                        Area::Response,
                        FactKind::ResponseContent,
                        facts,
                        unresolved_refs,
                    );
                }
            }
        } else if let Some(schema) = response_obj.get("schema") {
            // Swagger 2 keeps the response schema inline instead of under `content`.
            for media in media_types(root, operation, "produces") {
                flatten_content(
                    root,
                    schema,
                    &format!("{status_label} {media}"),
                    Area::Response,
                    FactKind::ResponseContent,
                    facts,
                    unresolved_refs,
                );
            }
        }

        handle_response_headers(root, response_obj, &status_label, facts, unresolved_refs);
    }
}

fn handle_response_headers(
    root: &Value,
    response: &Map<String, Value>,
    status_label: &str,
    facts: &mut BTreeMap<String, Fact>,
    unresolved_refs: &mut usize,
) {
    let Some(headers) = response.get("headers").and_then(Value::as_object) else {
        return;
    };

    for (name, header) in headers {
        let header = resolve(root, header, 0, unresolved_refs);
        let Some(header_obj) = header.as_object() else {
            continue;
        };
        let header_label = format!("{status_label} header {name}");

        match parameter_schema(header_obj) {
            Some(schema) => {
                flatten_schema(
                    root,
                    &schema,
                    &header_label,
                    Area::Response,
                    false,
                    facts,
                    unresolved_refs,
                    0,
                );
                if let Some(fact) = facts.get_mut(&header_label) {
                    fact.kind = FactKind::ResponseHeader;
                }
            }
            None => insert_fact(
                facts,
                Fact {
                    area: Area::Response,
                    label: header_label,
                    type_name: "any".to_string(),
                    required: false,
                    kind: FactKind::ResponseHeader,
                    enum_values: Vec::new(),
                    format: None,
                },
            ),
        }
    }
}

fn collect_parameters<'a>(
    root: &'a Value,
    sources: [Option<&'a Value>; 1],
    unresolved_refs: &mut usize,
) -> BTreeMap<(String, String), &'a Value> {
    let mut parameters = BTreeMap::new();
    for source in sources.into_iter().flatten() {
        let Some(array) = source.as_array() else {
            continue;
        };
        for parameter in array {
            let parameter = resolve(root, parameter, 0, unresolved_refs);
            let Some(parameter_obj) = parameter.as_object() else {
                continue;
            };
            let Some(name) = parameter_obj.get("name").and_then(Value::as_str) else {
                continue;
            };
            let Some(location) = parameter_obj.get("in").and_then(Value::as_str) else {
                continue;
            };
            parameters.insert((location.to_string(), name.to_string()), parameter);
        }
    }
    parameters
}

fn parameter_schema(parameter: &Map<String, Value>) -> Option<Value> {
    if let Some(schema) = parameter.get("schema") {
        return Some(schema.clone());
    }
    if let Some(schema) = parameter
        .get("content")
        .and_then(Value::as_object)
        .and_then(|content| content.values().next())
        .and_then(|media| media.get("schema"))
    {
        return Some(schema.clone());
    }

    // Swagger 2.0 defines non-body parameters (and response headers) inline
    // instead of under `schema`. Carry the schema keywords into a synthetic schema.
    let mut schema = Map::new();
    for key in ["type", "format", "enum", "items"] {
        if let Some(value) = parameter.get(key) {
            schema.insert(key.to_string(), value.clone());
        }
    }
    if schema.is_empty() {
        None
    } else {
        Some(Value::Object(schema))
    }
}

fn media_types(root: &Value, operation: &Map<String, Value>, key: &str) -> Vec<String> {
    let mut values: Vec<String> = operation
        .get(key)
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .or_else(|| root.get(key).and_then(Value::as_array))
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    values.sort();
    values.dedup();
    if values.is_empty() {
        values.push("application/json".to_string());
    }
    values
}

#[allow(clippy::too_many_arguments)]
fn flatten_schema(
    root: &Value,
    schema: &Value,
    base: &str,
    area: Area,
    required: bool,
    facts: &mut BTreeMap<String, Fact>,
    unresolved_refs: &mut usize,
    depth: usize,
) {
    if depth > 24 {
        insert_fact(
            facts,
            Fact {
                area,
                label: base.to_string(),
                type_name: "recursive".to_string(),
                required,
                kind: FactKind::Field,
                enum_values: Vec::new(),
                format: None,
            },
        );
        return;
    }

    let schema = resolve(root, schema, 0, unresolved_refs);
    let Some(schema_obj) = schema.as_object() else {
        insert_fact(
            facts,
            Fact {
                area,
                label: base.to_string(),
                type_name: json_type(schema).to_string(),
                required,
                kind: FactKind::Field,
                enum_values: Vec::new(),
                format: None,
            },
        );
        return;
    };

    let required_set = required_set(schema_obj);
    insert_fact(
        facts,
        Fact {
            area,
            label: base.to_string(),
            type_name: schema_type(schema_obj),
            required,
            kind: FactKind::Field,
            enum_values: enum_values(schema_obj),
            format: schema_obj
                .get("format")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
    );

    let mut composed_required = BTreeSet::new();

    if let Some(branches) = schema_obj.get("allOf").and_then(Value::as_array) {
        for branch in branches {
            flatten_schema(
                root,
                branch,
                base,
                area,
                required,
                facts,
                unresolved_refs,
                depth + 1,
            );
        }

        // `required` may live beside `allOf`, or in a different branch than the
        // property it names. All branches are merged at the same label, so mark
        // every composed property after the whole composition is flattened.
        collect_all_of_required(root, schema_obj, &mut composed_required, 0);
    }

    for keyword in ["oneOf", "anyOf"] {
        if let Some(branches) = schema_obj.get(keyword).and_then(Value::as_array) {
            for (index, branch) in branches.iter().enumerate() {
                flatten_schema(
                    root,
                    branch,
                    &format!("{base}[{}]", index + 1),
                    area,
                    required,
                    facts,
                    unresolved_refs,
                    depth + 1,
                );
            }
        }
    }

    if let Some(properties) = schema_obj.get("properties").and_then(Value::as_object) {
        for (name, property) in properties {
            flatten_schema(
                root,
                property,
                &format!("{base}.{name}"),
                area,
                required_set.contains(name),
                facts,
                unresolved_refs,
                depth + 1,
            );
        }
    }

    for name in &composed_required {
        if let Some(fact) = facts.get_mut(&format!("{base}.{name}")) {
            fact.required = true;
        }
    }

    if let Some(additional) = schema_obj.get("additionalProperties") {
        match additional {
            Value::Bool(true) => insert_fact(
                facts,
                Fact {
                    area,
                    label: format!("{base}.*"),
                    type_name: "any".to_string(),
                    required: false,
                    kind: FactKind::Field,
                    enum_values: Vec::new(),
                    format: None,
                },
            ),
            Value::Object(_) => flatten_schema(
                root,
                additional,
                &format!("{base}.*"),
                area,
                false,
                facts,
                unresolved_refs,
                depth + 1,
            ),
            _ => {}
        }
    }

    if let Some(items) = schema_obj.get("items") {
        flatten_schema(
            root,
            items,
            &format!("{base}[]"),
            area,
            false,
            facts,
            unresolved_refs,
            depth + 1,
        );
    }

    if let Some(prefix_items) = schema_obj.get("prefixItems").and_then(Value::as_array) {
        for (index, item) in prefix_items.iter().enumerate() {
            flatten_schema(
                root,
                item,
                &format!("{base}[{index}]"),
                area,
                false,
                facts,
                unresolved_refs,
                depth + 1,
            );
        }
    }
}

/// Flatten one request/response media type. The content root is a container,
/// not a property: requiredness belongs to the body root, and adding a media
/// type is always compatible for existing clients.
#[allow(clippy::too_many_arguments)]
fn flatten_content(
    root: &Value,
    schema: &Value,
    label: &str,
    area: Area,
    kind: FactKind,
    facts: &mut BTreeMap<String, Fact>,
    unresolved_refs: &mut usize,
) {
    flatten_schema(root, schema, label, area, false, facts, unresolved_refs, 0);
    if let Some(fact) = facts.get_mut(label) {
        fact.kind = kind;
    }
}

/// Collect `required` names from an `allOf` composition. All branches describe
/// the same object, so a property required in any branch is required overall.
fn collect_all_of_required(
    root: &Value,
    schema: &Map<String, Value>,
    names: &mut BTreeSet<String>,
    depth: usize,
) {
    if depth > 24 {
        return;
    }
    names.extend(required_set(schema));

    let Some(branches) = schema.get("allOf").and_then(Value::as_array) else {
        return;
    };
    let mut ignored_unresolved = 0;
    for branch in branches {
        // Resolve only for reading `required`; flattening the branch later
        // reports unresolved refs so they are not counted twice.
        let branch = resolve(root, branch, 0, &mut ignored_unresolved);
        if let Some(branch_obj) = branch.as_object() {
            collect_all_of_required(root, branch_obj, names, depth + 1);
        }
    }
}

fn flatten_json(value: &Value) -> Flattened {
    let mut facts = BTreeMap::new();
    flatten_json_value(value, "$", &mut facts, 0);
    Flattened {
        facts,
        format: Format::Json,
        operations: 0,
        title: None,
        unresolved_refs: 0,
    }
}

fn flatten_json_value(value: &Value, path: &str, facts: &mut BTreeMap<String, Fact>, depth: usize) {
    if depth > 32 {
        return;
    }

    insert_fact(
        facts,
        Fact {
            area: Area::Value,
            label: path.to_string(),
            type_name: json_type(value).to_string(),
            required: false,
            kind: FactKind::Field,
            enum_values: Vec::new(),
            format: None,
        },
    );

    match value {
        Value::Object(map) => {
            for (key, child) in map {
                flatten_json_value(child, &format!("{path}.{key}"), facts, depth + 1);
            }
        }
        Value::Array(items) => {
            if let Some(first) = items.first() {
                flatten_json_value(first, &format!("{path}[]"), facts, depth + 1);
            }
        }
        _ => {}
    }
}

fn schema_type(schema: &Map<String, Value>) -> String {
    let mut type_name = match schema.get("type") {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("|"),
        _ if schema.contains_key("$ref") => "ref".to_string(),
        _ if schema.contains_key("properties")
            || schema.contains_key("additionalProperties")
            || schema.contains_key("required") =>
        {
            "object".to_string()
        }
        _ if schema.contains_key("items") || schema.contains_key("prefixItems") => {
            "array".to_string()
        }
        _ if schema.contains_key("oneOf") || schema.contains_key("anyOf") => "union".to_string(),
        _ if schema.contains_key("allOf") => "object".to_string(),
        _ if schema.contains_key("enum") => "enum".to_string(),
        _ if schema.contains_key("const") => "const".to_string(),
        _ => "any".to_string(),
    };

    if schema
        .get("nullable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && !type_name.split('|').any(|part| part == "null")
    {
        type_name.push_str("|null");
    }
    type_name
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn enum_values(schema: &Map<String, Value>) -> Vec<String> {
    let mut values: Vec<String> = schema
        .get("enum")
        .and_then(Value::as_array)
        .map(|values| values.iter().map(enum_value).collect())
        .unwrap_or_default();
    if let Some(const_value) = schema.get("const") {
        values.push(enum_value(const_value));
    }
    values.sort();
    values.dedup();
    values
}

fn enum_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    }
}

fn required_set(schema: &Map<String, Value>) -> BTreeSet<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn document_title(root: &Value) -> Option<String> {
    let info = root.get("info")?.as_object()?;
    let title = info.get("title").and_then(Value::as_str)?;
    let version = info.get("version").and_then(Value::as_str);
    Some(match version {
        Some(version) => format!("{title} {version}"),
        None => title.to_string(),
    })
}

fn insert_fact(facts: &mut BTreeMap<String, Fact>, fact: Fact) {
    facts.entry(fact.label.clone()).or_insert(fact);
}

fn resolve<'a>(
    root: &'a Value,
    node: &'a Value,
    mut depth: usize,
    unresolved_refs: &mut usize,
) -> &'a Value {
    let mut current = node;
    loop {
        let Some(reference) = current.get("$ref").and_then(Value::as_str) else {
            return current;
        };
        if depth >= 32 {
            return current;
        }
        match resolve_pointer(root, reference) {
            Some(target) => {
                current = target;
                depth += 1;
            }
            None => {
                *unresolved_refs += 1;
                return current;
            }
        }
    }
}

fn resolve_pointer<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let pointer = reference.strip_prefix('#')?;
    if pointer.is_empty() {
        return Some(root);
    }
    let pointer = pointer.strip_prefix('/')?;

    let mut current = root;
    for token in pointer.split('/') {
        let token = token.replace("~1", "/").replace("~0", "~");
        current = match current {
            Value::Object(map) => map.get(&token)?,
            Value::Array(items) => items.get(token.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

/// Derive a short, stable target name from a URL or local path.
pub fn derive_name(url: &str) -> String {
    let source = url
        .strip_prefix("file://")
        .unwrap_or(url)
        .trim_end_matches('/');

    if !source.contains("://") {
        let stem = std::path::Path::new(source)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("local");
        return sanitize_name(stem);
    }

    let Ok(parsed) = reqwest::Url::parse(url) else {
        return "target".to_string();
    };
    let host = parsed.host_str().unwrap_or("local").replace('.', "-");
    let generic = [
        "openapi.json",
        "openapi.yaml",
        "openapi.yml",
        "swagger.json",
        "swagger.yaml",
        "swagger.yml",
        "spec.json",
        "spec.yaml",
        "spec.yml",
        "api-docs",
        "v3",
        "v2",
    ];
    let segments: Vec<&str> = parsed
        .path_segments()
        .map(|segments| segments.filter(|segment| !segment.is_empty()).collect())
        .unwrap_or_default();

    let mut parts = vec![host];
    if let Some(last) = segments.last() {
        if generic.contains(&last.to_ascii_lowercase().as_str()) {
            if let Some(previous) = segments.iter().rev().nth(1) {
                parts.push((*previous).to_string());
            }
        } else {
            parts.push((*last).to_string());
        }
    }

    sanitize_name(&parts.join("-"))
}

fn sanitize_name(value: &str) -> String {
    let mut name = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            name.push(character.to_ascii_lowercase());
        } else if (character == '-' || character == '_' || character == '.') && !name.ends_with('-')
        {
            name.push('-');
        }
    }
    let name = name.trim_matches('-').to_string();
    let name = if name.is_empty() {
        "target".to_string()
    } else {
        name
    };
    name.chars().take(64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn openapi() -> Value {
        serde_json::json!({
            "openapi": "3.0.0",
            "info": { "title": "Pets", "version": "1.0.0" },
            "paths": {
                "/pets": {
                    "get": {
                        "parameters": [
                            { "name": "limit", "in": "query", "schema": { "type": "integer" } }
                        ],
                        "responses": {
                            "200": {
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "type": "array",
                                            "items": { "$ref": "#/components/schemas/Pet" }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    "post": {
                        "requestBody": {
                            "required": true,
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/NewPet" }
                                }
                            }
                        },
                        "responses": { "201": { "description": "created" } }
                    }
                }
            },
            "components": {
                "schemas": {
                    "Pet": {
                        "type": "object",
                        "required": ["id", "name"],
                        "properties": {
                            "id": { "type": "integer" },
                            "name": { "type": "string" },
                            "status": { "type": "string", "enum": ["available", "sold"] }
                        }
                    },
                    "NewPet": {
                        "type": "object",
                        "required": ["name"],
                        "properties": {
                            "name": { "type": "string" },
                            "tag": { "type": "string" }
                        }
                    }
                }
            }
        })
    }

    #[test]
    fn flattens_openapi_operations_parameters_and_schemas() {
        let flattened = flatten(&openapi(), Format::OpenApi).unwrap();
        assert_eq!(flattened.operations, 2);
        assert_eq!(flattened.unresolved_refs, 0);
        assert!(flattened.facts.contains_key("GET /pets"));
        assert!(flattened.facts.contains_key("GET /pets · query limit"));
        assert!(
            flattened
                .facts
                .contains_key("GET /pets · 200 application/json[]")
        );
        assert!(
            flattened
                .facts
                .contains_key("GET /pets · 200 application/json[].id")
        );
        assert!(
            flattened
                .facts
                .contains_key("POST /pets · body application/json.name")
        );
        assert!(
            flattened
                .facts
                .contains_key("GET /pets · 200 application/json[].status")
        );
        let name = flattened
            .facts
            .get("POST /pets · body application/json.name")
            .unwrap();
        assert!(name.required);
        let tag = flattened
            .facts
            .get("POST /pets · body application/json.tag")
            .unwrap();
        assert!(!tag.required);
        let status = flattened
            .facts
            .get("GET /pets · 200 application/json[].status")
            .unwrap();
        assert_eq!(status.enum_values, vec!["available", "sold"]);
        assert_eq!(flattened.title.as_deref(), Some("Pets 1.0.0"));
    }

    #[test]
    fn flattens_swagger2_parameters_media_types_and_response_headers() {
        let value = serde_json::json!({
            "swagger": "2.0",
            "info": { "title": "Swagger Params", "version": "1" },
            "paths": {
                "/pets": {
                    "get": {
                        "parameters": [
                            { "name": "limit", "in": "query", "type": "integer", "format": "int32" },
                            { "name": "kind", "in": "query", "type": "string", "enum": ["cat", "dog"] }
                        ],
                        "responses": {
                            "200": {
                                "description": "ok",
                                "schema": {
                                    "type": "array",
                                    "items": { "$ref": "#/definitions/Pet" }
                                },
                                "headers": {
                                    "X-Rate-Limit": { "type": "integer", "format": "int32" },
                                    "X-Tags": { "type": "array", "items": { "type": "string" } }
                                }
                            }
                        }
                    },
                    "post": {
                        "consumes": ["application/json", "application/xml"],
                        "parameters": [
                            {
                                "name": "body",
                                "in": "body",
                                "required": true,
                                "schema": { "$ref": "#/definitions/NewPet" }
                            }
                        ],
                        "responses": { "201": { "description": "created" } }
                    }
                }
            },
            "definitions": {
                "Pet": {
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "integer" },
                        "name": { "type": "string" }
                    }
                },
                "NewPet": {
                    "type": "object",
                    "required": ["name"],
                    "properties": {
                        "name": { "type": "string" },
                        "tag": { "type": "string" }
                    }
                }
            }
        });

        let flattened = flatten(&value, Format::OpenApi).unwrap();
        assert_eq!(flattened.unresolved_refs, 0);

        let limit = &flattened.facts["GET /pets · query limit"];
        assert_eq!(limit.type_name, "integer");
        assert_eq!(limit.format.as_deref(), Some("int32"));
        assert_eq!(limit.kind, FactKind::Parameter);
        assert!(!limit.required);

        let kind = &flattened.facts["GET /pets · query kind"];
        assert_eq!(kind.type_name, "string");
        assert_eq!(kind.enum_values, vec!["cat", "dog"]);

        let body = &flattened.facts["POST /pets · body"];
        assert_eq!(body.kind, FactKind::RequestBody);
        assert!(body.required);
        assert!(
            flattened
                .facts
                .contains_key("POST /pets · body application/json.name")
        );
        assert!(
            flattened
                .facts
                .contains_key("POST /pets · body application/xml.name")
        );
        assert!(flattened.facts["POST /pets · body application/xml.name"].required);

        assert!(
            flattened
                .facts
                .contains_key("GET /pets · 200 application/json[].id")
        );
        let rate_limit = &flattened.facts["GET /pets · 200 header X-Rate-Limit"];
        assert_eq!(rate_limit.kind, FactKind::ResponseHeader);
        assert_eq!(rate_limit.type_name, "integer");
        assert_eq!(rate_limit.format.as_deref(), Some("int32"));
        assert_eq!(
            flattened.facts["GET /pets · 200 header X-Tags[]"].type_name,
            "string"
        );
    }

    #[test]
    fn propagates_all_of_required_names_across_branches() {
        let value = serde_json::json!({
            "openapi": "3.0.0",
            "info": { "title": "Compose", "version": "1" },
            "paths": {
                "/pets": {
                    "post": {
                        "requestBody": {
                            "required": true,
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "required": ["name"],
                                        "properties": {
                                            "parentOnly": { "type": "string" }
                                        },
                                        "allOf": [
                                            {
                                                "type": "object",
                                                "required": ["id", "parentOnly"]
                                            },
                                            {
                                                "type": "object",
                                                "properties": {
                                                    "id": { "type": "integer" },
                                                    "name": { "type": "string" },
                                                    "tag": { "type": "string" }
                                                }
                                            }
                                        ]
                                    }
                                }
                            }
                        },
                        "responses": { "204": { "description": "ok" } }
                    }
                }
            }
        });

        let flattened = flatten(&value, Format::OpenApi).unwrap();
        assert!(flattened.facts["POST /pets · body application/json.id"].required);
        assert!(flattened.facts["POST /pets · body application/json.name"].required);
        assert!(flattened.facts["POST /pets · body application/json.parentOnly"].required);
        assert!(!flattened.facts["POST /pets · body application/json.tag"].required);
    }

    #[test]
    fn parses_yaml_openapi() {
        let body = r#"
openapi: 3.0.0
info:
  title: Tiny
  version: "1"
paths:
  /ping:
    get:
      responses:
        "200":
          description: ok
"#;
        let (value, format) = parse_document(body).unwrap();
        assert_eq!(format, Format::OpenApi);
        assert_eq!(flatten(&value, format).unwrap().operations, 1);
    }

    #[test]
    fn rejects_plain_text_and_html_documents() {
        for body in ["<html><body>login</body></html>", "service unavailable"] {
            let error = parse_document(body).unwrap_err();
            assert!(error.to_string().contains("not a JSON object or array"));
        }
    }

    #[test]
    fn generic_json_flattening_tracks_nested_paths() {
        let value = serde_json::json!({
            "data": [{ "id": 1, "name": "Ada" }],
            "ok": true
        });
        let flattened = flatten(&value, Format::Json).unwrap();
        assert!(flattened.facts.contains_key("$.data[]"));
        assert!(flattened.facts.contains_key("$.data[].name"));
        assert_eq!(flattened.facts["$.data[].id"].type_name, "integer");
    }

    #[test]
    fn derives_readable_names() {
        assert_eq!(
            derive_name("https://petstore3.swagger.io/api/v3/openapi.json"),
            "petstore3-swagger-io-v3"
        );
        assert_eq!(
            derive_name("https://api.example.com/openapi.yaml"),
            "api-example-com"
        );
        assert_eq!(derive_name("/tmp/my-spec.json"), "my-spec");
    }
}
