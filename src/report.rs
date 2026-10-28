use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::db::{Check, Target};
use crate::diff::{Change, ChangeKind};
use crate::spec::MAX_FACTS;
use crate::style::{bold, cyan, dim, green, red, yellow};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Baseline,
    Unchanged,
    Changed,
    Error,
}

impl CheckStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Unchanged => "unchanged",
            Self::Changed => "changed",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetReport {
    pub name: String,
    pub url: String,
    pub status: CheckStatus,
    pub checked_at: String,
    pub format: Option<String>,
    pub title: Option<String>,
    pub operations: Option<usize>,
    pub facts: Option<usize>,
    pub breaking: usize,
    pub compatible: usize,
    pub http_status: Option<u16>,
    pub message: Option<String>,
    pub unresolved_refs: usize,
    pub truncated: bool,
    pub changes: Vec<Change>,
}

const MAX_PRINTED_CHANGES: usize = 40;

#[derive(Debug, Clone, Serialize)]
struct SarifLog {
    #[serde(rename = "$schema")]
    schema: &'static str,
    version: &'static str,
    runs: Vec<SarifRun>,
}

#[derive(Debug, Clone, Serialize)]
struct SarifRun {
    tool: SarifTool,
    results: Vec<SarifResult>,
}

#[derive(Debug, Clone, Serialize)]
struct SarifTool {
    driver: SarifDriver,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifDriver {
    name: &'static str,
    information_uri: &'static str,
    version: &'static str,
    rules: Vec<SarifRule>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifRule {
    id: &'static str,
    name: &'static str,
    short_description: SarifText,
    help_uri: &'static str,
    default_configuration: SarifLevel,
}

#[derive(Debug, Clone, Serialize)]
struct SarifText {
    text: String,
}

#[derive(Debug, Clone, Serialize)]
struct SarifLevel {
    level: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifResult {
    rule_id: &'static str,
    rule_index: usize,
    level: &'static str,
    message: SarifText,
    locations: Vec<SarifLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    properties: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifLocation {
    physical_location: SarifPhysicalLocation,
    logical_locations: Vec<SarifLogicalLocation>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifPhysicalLocation {
    artifact_location: SarifArtifactLocation,
    #[serde(skip_serializing_if = "Option::is_none")]
    region: Option<SarifRegion>,
}

#[derive(Debug, Clone, Serialize)]
struct SarifArtifactLocation {
    uri: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifRegion {
    start_line: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifLogicalLocation {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    fully_qualified_name: Option<String>,
    kind: &'static str,
}

const SARIF_SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";
const SARIF_HELP_URI: &str = "https://github.com/hix-pro/api-watch#what-counts-as-breaking";

const SARIF_RULES: [(&str, &str, &str, &str); 3] = [
    (
        "apiwatch/breaking",
        "BreakingApiChange",
        "The API contract changed in a way that can break existing clients.",
        "error",
    ),
    (
        "apiwatch/compatible",
        "CompatibleApiChange",
        "The API contract changed without breaking existing clients.",
        "note",
    ),
    (
        "apiwatch/check-error",
        "CheckError",
        "API Watch could not fetch or compare this target.",
        "error",
    ),
];

fn sarif_rule_index(rule_id: &str) -> usize {
    SARIF_RULES
        .iter()
        .position(|(id, ..)| *id == rule_id)
        .unwrap_or(0)
}

/// Code scanning resolves relative artifact URIs against the repository root.
/// Local specs are made relative to the current directory; remote specs keep
/// their URL so the alert still names the contract that changed.
fn sarif_uri(source: &str) -> String {
    let source = source.strip_prefix("file://").unwrap_or(source);
    if source.contains("://") {
        return source.to_owned();
    }

    let path = Path::new(source);
    let relative = if path.is_absolute() {
        std::env::current_dir()
            .ok()
            .and_then(|cwd| path.strip_prefix(cwd).ok().map(Path::to_path_buf))
    } else {
        Some(path.to_path_buf())
    };

    let Some(path) = relative else {
        return source.replace('\\', "/");
    };
    let value = path.to_string_lossy().replace('\\', "/");
    value.strip_prefix("./").unwrap_or(&value).to_owned()
}

fn sarif_location(report: &TargetReport, label: &str) -> SarifLocation {
    let source = report.url.strip_prefix("file://").unwrap_or(&report.url);
    let remote = source.contains("://");

    SarifLocation {
        physical_location: SarifPhysicalLocation {
            artifact_location: SarifArtifactLocation {
                uri: sarif_uri(source),
            },
            // A local spec is a repository file, so a file-level annotation is
            // meaningful. Remote specs have no line to annotate.
            region: if remote {
                None
            } else {
                Some(SarifRegion { start_line: 1 })
            },
        },
        logical_locations: vec![SarifLogicalLocation {
            name: report.name.clone(),
            fully_qualified_name: if label.is_empty() {
                None
            } else {
                Some(label.to_owned())
            },
            kind: if label.is_empty() { "module" } else { "member" },
        }],
    }
}

/// SARIF 2.1.0 output for GitHub code scanning and other SARIF consumers.
pub fn render_sarif(reports: &[TargetReport]) -> Result<String> {
    let rules = SARIF_RULES
        .iter()
        .map(|(id, name, description, level)| SarifRule {
            id,
            name,
            short_description: SarifText {
                text: (*description).to_owned(),
            },
            help_uri: SARIF_HELP_URI,
            default_configuration: SarifLevel { level },
        })
        .collect();

    let mut results = Vec::new();
    for report in reports {
        if report.status == CheckStatus::Error {
            let message = report.message.as_deref().unwrap_or("unknown error");
            results.push(SarifResult {
                rule_id: "apiwatch/check-error",
                rule_index: sarif_rule_index("apiwatch/check-error"),
                level: "error",
                message: SarifText {
                    text: format!("API Watch could not check '{}': {message}", report.name),
                },
                locations: vec![sarif_location(report, "")],
                properties: Some(serde_json::json!({
                    "target": report.name,
                    "source": report.url,
                })),
            });
            continue;
        }

        for change in &report.changes {
            let (rule_id, level, preamble) = match change.kind {
                ChangeKind::Breaking => (
                    "apiwatch/breaking",
                    "error",
                    format!("Breaking API change in '{}'", report.name),
                ),
                ChangeKind::Compatible => (
                    "apiwatch/compatible",
                    "note",
                    format!("Compatible API change in '{}'", report.name),
                ),
            };
            results.push(SarifResult {
                rule_id,
                rule_index: sarif_rule_index(rule_id),
                level,
                message: SarifText {
                    text: format!(
                        "{preamble}: {} — {}\n\nSource: {}",
                        change.label, change.detail, report.url
                    ),
                },
                locations: vec![sarif_location(report, &change.label)],
                properties: Some(serde_json::json!({
                    "target": report.name,
                    "area": change.area,
                    "kind": change.kind,
                })),
            });
        }
    }

    let log = SarifLog {
        schema: SARIF_SCHEMA,
        version: "2.1.0",
        runs: vec![SarifRun {
            tool: SarifTool {
                driver: SarifDriver {
                    name: "API Watch",
                    information_uri: "https://github.com/hix-pro/api-watch",
                    version: env!("CARGO_PKG_VERSION"),
                    rules,
                },
            },
            results,
        }],
    };

    let mut output = serde_json::to_string_pretty(&log)?;
    output.push('\n');
    Ok(output)
}

pub fn print_checks(reports: &[TargetReport], json: bool) -> Result<()> {
    if json {
        let errors = reports
            .iter()
            .filter(|report| report.status == CheckStatus::Error)
            .count();
        let breaking = reports.iter().map(|report| report.breaking).sum::<usize>();
        let compatible = reports
            .iter()
            .map(|report| report.compatible)
            .sum::<usize>();
        let changed = reports
            .iter()
            .filter(|report| report.status == CheckStatus::Changed)
            .count();
        let value = serde_json::json!({
            "targets": reports,
            "summary": {
                "checked": reports.len(),
                "changed": changed,
                "breaking": breaking,
                "compatible": compatible,
                "errors": errors,
            }
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    for (index, report) in reports.iter().enumerate() {
        if index > 0 {
            println!();
        }
        print_report(report);
    }

    if reports.len() > 1 {
        println!();
        print_summary(reports);
    }
    Ok(())
}

fn print_report(report: &TargetReport) {
    let mut header = bold(&report.name);
    if let Some(format) = &report.format {
        header.push_str(&dim(" · "));
        header.push_str(format);
    }
    if let Some(title) = &report.title {
        header.push_str(&dim(" · "));
        header.push_str(title);
    }
    println!("{header}");

    match report.status {
        CheckStatus::Baseline => {
            let mut line = cyan("baseline saved");
            append_counts(&mut line, report);
            println!("{line}");
        }
        CheckStatus::Unchanged => {
            let mut line = green("no contract changes");
            append_counts(&mut line, report);
            println!("{line}");
        }
        CheckStatus::Changed => {
            let mut line = if report.breaking > 0 {
                red("breaking changes detected")
            } else {
                yellow("compatible changes only")
            };
            if report.breaking > 0 {
                line.push_str(&dim(" · "));
                line.push_str(&red(&format!("{} breaking", report.breaking)));
            }
            if report.compatible > 0 {
                line.push_str(&dim(" · "));
                line.push_str(&yellow(&format!("{} compatible", report.compatible)));
            }
            if let Some(status) = report.http_status {
                line.push_str(&dim(&format!(" · HTTP {status}")));
            }
            println!("{line}");
        }
        CheckStatus::Error => {
            println!("{}", red("check failed"));
        }
    }

    if let Some(message) = &report.message {
        println!("  {}", dim(message));
    }

    if report.status == CheckStatus::Changed {
        for change in report.changes.iter().take(MAX_PRINTED_CHANGES) {
            print_change(change);
        }
        if report.changes.len() > MAX_PRINTED_CHANGES {
            println!(
                "  {}",
                dim(&format!(
                    "… and {} more changes (use --json to see all)",
                    report.changes.len() - MAX_PRINTED_CHANGES
                ))
            );
        }
    }

    if report.unresolved_refs > 0 {
        println!(
            "  {}",
            yellow(&format!(
                "warning: {} $ref(s) could not be resolved",
                report.unresolved_refs
            ))
        );
    }

    if report.truncated {
        println!(
            "  {}",
            yellow(&format!(
                "warning: fact expansion stopped at the {MAX_FACTS} fact budget; \
                 recursive or deeply nested schemas were truncated"
            ))
        );
    }
}

fn print_change(change: &Change) {
    let (kind, colored) = match change.kind {
        ChangeKind::Breaking => ("BREAKING", red("BREAKING")),
        ChangeKind::Compatible => ("COMPATIBLE", green("COMPATIBLE")),
    };
    let padding = " ".repeat(10usize.saturating_sub(kind.len()));
    println!("  {colored}{padding}  {}", change.label);
    println!("              {}", dim(&change.detail));
}

fn append_counts(line: &mut String, report: &TargetReport) {
    if let Some(facts) = report.facts {
        line.push_str(&dim(&format!(" · {facts} facts")));
    }
    if let Some(operations) = report.operations {
        if operations > 0 {
            line.push_str(&dim(&format!(" · {operations} endpoints")));
        }
    }
    if let Some(status) = report.http_status {
        line.push_str(&dim(&format!(" · HTTP {status}")));
    }
}

fn print_summary(reports: &[TargetReport]) {
    let errors = reports
        .iter()
        .filter(|report| report.status == CheckStatus::Error)
        .count();
    let changed = reports
        .iter()
        .filter(|report| report.status == CheckStatus::Changed)
        .count();
    let breaking = reports.iter().map(|report| report.breaking).sum::<usize>();

    let mut parts = vec![dim(&format!("{} checked", reports.len()))];
    if changed > 0 {
        parts.push(yellow(&format!("{changed} changed")));
    }
    if breaking > 0 {
        parts.push(red(&format!("{breaking} breaking")));
    }
    if errors > 0 {
        parts.push(red(&format!("{errors} error")));
    }
    if changed == 0 && errors == 0 {
        parts.push(green("all clear"));
    }
    println!("{}", parts.join(&dim(" · ")));
}

pub fn print_targets(targets: &[Target], json: bool) -> Result<()> {
    if json {
        let items: Vec<_> = targets
            .iter()
            .map(|target| {
                let mut header_names: Vec<&str> =
                    target.headers.keys().map(String::as_str).collect();
                header_names.sort_unstable();
                serde_json::json!({
                    "name": target.name,
                    "url": target.url,
                    "createdAt": target.created_at,
                    "lastCheckedAt": target.last_checked_at,
                    "status": target.last_status,
                    "breaking": target.last_breaking,
                    "compatible": target.last_compatible,
                    "headers": header_names,
                })
            })
            .collect();
        let value = serde_json::json!({
            "targets": items,
            "total": targets.len(),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    if targets.is_empty() {
        println!("{}", dim("No targets yet."));
        println!();
        println!("Add one with:");
        println!(
            "  {}",
            bold("apiwatch add https://petstore3.swagger.io/api/v3/openapi.json --name petstore")
        );
        return Ok(());
    }

    println!(
        "{}",
        dim(&format!(
            "{} target{}",
            targets.len(),
            if targets.len() == 1 { "" } else { "s" }
        ))
    );
    println!();

    let name_width = targets
        .iter()
        .map(|target| target.name.chars().count())
        .max()
        .unwrap_or(4)
        .clamp(4, 24);

    println!(
        "{}",
        dim(&format!(
            "{:<name_width$}  {:<12}  {:<16}  {:<12}  SOURCE",
            "NAME", "STATUS", "CHANGES", "CHECKED"
        ))
    );

    for target in targets {
        let name = truncate(&target.name, name_width);
        let (status_text, status_color) = target_status(target);
        let status = paint_padded(status_text, 12, status_color);
        let (changes_text, changes_color) = changes_summary(target);
        let changes = paint_padded(&changes_text, 16, changes_color);
        let checked = target
            .last_checked_at
            .as_deref()
            .map(relative_time)
            .unwrap_or_else(|| "never".to_string());
        println!(
            "{:<name_width$}  {}  {}  {:<12}  {}",
            name,
            status,
            changes,
            checked,
            truncate(&target.url, 58)
        );
    }

    Ok(())
}

fn target_status(target: &Target) -> (&'static str, fn(&str) -> String) {
    match target.last_status.as_deref() {
        Some("changed") if target.last_breaking > 0 => ("breaking", red),
        Some("changed") => ("changed", yellow),
        Some("baseline") => ("baseline", cyan),
        Some("unchanged") => ("unchanged", green),
        Some("error") => ("error", red),
        _ => ("not checked", dim),
    }
}

fn changes_summary(target: &Target) -> (String, fn(&str) -> String) {
    let breaking = target.last_breaking;
    let compatible = target.last_compatible;
    if breaking > 0 {
        (format!("{breaking} breaking"), red)
    } else if compatible > 0 {
        (format!("+{compatible} compatible"), yellow)
    } else {
        ("none".to_string(), dim)
    }
}

fn paint_padded(text: &str, width: usize, color: fn(&str) -> String) -> String {
    color(&format!("{text:<width$}"))
}

pub fn print_added(target: &Target) {
    println!("{} {}", green("Added"), bold(&target.name));
    println!("  {:<9} {}", dim("URL"), target.url);
    if !target.headers.is_empty() {
        let mut names: Vec<&str> = target.headers.keys().map(String::as_str).collect();
        names.sort_unstable();
        println!("  {:<9} {}", dim("Headers"), names.join(", "));
    }
    println!();
    println!("Next: {}", bold(&format!("apiwatch check {}", target.name)));
}

/// Re-adding an already-tracked target with the same identity is a no-op:
/// the stored baseline and history stay untouched.
pub fn print_already_tracked(target: &Target) {
    println!("{} {}", cyan("Already tracked"), bold(&target.name));
    println!("  {:<9} {}", dim("URL"), target.url);
    if !target.headers.is_empty() {
        let mut names: Vec<&str> = target.headers.keys().map(String::as_str).collect();
        names.sort_unstable();
        println!("  {:<9} {}", dim("Headers"), names.join(", "));
    }
    println!();
    println!(
        "Keeping the stored baseline. Next: {}",
        bold(&format!("apiwatch check {}", target.name))
    );
}

pub fn print_removed(name: &str) {
    println!("{} {}", green("Removed"), bold(name));
}

pub fn print_history(target: &Target, checks: &[Check], json: bool) -> Result<()> {
    if json {
        let value = serde_json::json!({
            "target": {
                "name": target.name,
                "url": target.url,
                "createdAt": target.created_at,
            },
            "checks": checks,
            "total": checks.len(),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    println!("{}  {}", bold(&target.name), dim(&target.url));
    if checks.is_empty() {
        println!();
        println!("{}", dim("No checks yet."));
        println!("Run: {}", bold(&format!("apiwatch check {}", target.name)));
        return Ok(());
    }
    println!();
    println!(
        "{}",
        dim(&format!(
            "{:<22}  {:<12}  {:>8}  {:>10}  NOTE",
            "CHECKED (UTC)", "STATUS", "BREAKING", "COMPATIBLE"
        ))
    );
    for check in checks {
        let status = check_status(check);
        println!(
            "{:<22}  {:<12}  {:>8}  {:>10}  {}",
            format_timestamp(&check.checked_at),
            status,
            check.breaking,
            check.compatible,
            truncate(check.message.as_deref().unwrap_or(""), 52)
        );
    }
    Ok(())
}

fn check_status(check: &Check) -> String {
    let text = format!("{:<12}", check.status);
    match check.status.as_str() {
        "changed" if check.breaking > 0 => red(&text),
        "changed" => yellow(&text),
        "baseline" => cyan(&text),
        "unchanged" => green(&text),
        "error" => red(&text),
        _ => text,
    }
}

fn truncate(value: &str, max: usize) -> String {
    let characters: Vec<char> = value.chars().collect();
    if characters.len() <= max {
        return value.to_string();
    }
    if max <= 3 {
        return characters.into_iter().take(max).collect();
    }
    let mut result: String = characters.into_iter().take(max - 3).collect();
    result.push_str("...");
    result
}

pub fn relative_time(iso: &str) -> String {
    let Ok(timestamp) = DateTime::parse_from_rfc3339(iso) else {
        return iso.to_string();
    };
    let seconds = (Utc::now() - timestamp.with_timezone(&Utc)).num_seconds();
    if seconds < 60 {
        "just now".to_string()
    } else if seconds < 3_600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h ago", seconds / 3_600)
    } else {
        format!("{}d ago", seconds / 86_400)
    }
}

fn format_timestamp(iso: &str) -> String {
    match DateTime::parse_from_rfc3339(iso) {
        Ok(timestamp) => timestamp
            .with_timezone(&Utc)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        Err(_) => truncate(iso, 22),
    }
}

pub fn status_name(status: CheckStatus) -> &'static str {
    status.label()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{Change, ChangeKind};
    use crate::spec::Area;

    fn target(url: &str, status: CheckStatus, changes: Vec<Change>) -> TargetReport {
        TargetReport {
            name: "orders".to_owned(),
            url: url.to_owned(),
            status,
            checked_at: "2026-10-28T00:00:00Z".to_owned(),
            format: Some("OpenAPI".to_owned()),
            title: Some("Orders API".to_owned()),
            operations: Some(3),
            facts: Some(17),
            breaking: changes
                .iter()
                .filter(|change| change.kind == ChangeKind::Breaking)
                .count(),
            compatible: changes
                .iter()
                .filter(|change| change.kind == ChangeKind::Compatible)
                .count(),
            http_status: Some(200),
            message: None,
            unresolved_refs: 0,
            truncated: false,
            changes,
        }
    }

    fn change(kind: ChangeKind, area: Area, label: &str, detail: &str) -> Change {
        Change {
            kind,
            area,
            label: label.to_owned(),
            detail: detail.to_owned(),
        }
    }

    #[test]
    fn sarif_maps_changes_to_stable_rules_and_locations() {
        let report = target(
            "./spec/openapi.json",
            CheckStatus::Changed,
            vec![
                change(
                    ChangeKind::Breaking,
                    Area::Operation,
                    "DELETE /orders/{id}",
                    "endpoint removed",
                ),
                change(
                    ChangeKind::Compatible,
                    Area::Parameter,
                    "GET /orders · query cursor",
                    "new optional parameter",
                ),
            ],
        );

        let output = render_sarif(&[report]).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert_eq!(value["version"], "2.1.0");
        assert_eq!(value["runs"][0]["tool"]["driver"]["name"], "API Watch");
        assert_eq!(
            value["runs"][0]["tool"]["driver"]["rules"]
                .as_array()
                .unwrap()
                .len(),
            3
        );

        let results = value["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["ruleId"], "apiwatch/breaking");
        assert_eq!(results[0]["level"], "error");
        assert_eq!(
            results[0]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "spec/openapi.json"
        );
        assert_eq!(
            results[0]["locations"][0]["physicalLocation"]["region"]["startLine"],
            1
        );
        assert_eq!(
            results[0]["locations"][0]["logicalLocations"][0]["fullyQualifiedName"],
            "DELETE /orders/{id}"
        );
        assert_eq!(results[0]["properties"]["area"], "operation");
        assert!(
            results[0]["message"]["text"]
                .as_str()
                .unwrap()
                .contains("endpoint removed")
        );

        assert_eq!(results[1]["ruleId"], "apiwatch/compatible");
        assert_eq!(results[1]["level"], "note");
        assert_eq!(results[1]["properties"]["kind"], "compatible");
    }

    #[test]
    fn sarif_keeps_remote_sources_and_flags_check_errors() {
        let mut report = target(
            "https://example.com/openapi.json",
            CheckStatus::Error,
            Vec::new(),
        );
        report.message = Some("HTTP 503".to_owned());

        let output = render_sarif(&[report]).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        let result = &value["runs"][0]["results"][0];

        assert_eq!(result["ruleId"], "apiwatch/check-error");
        assert_eq!(result["level"], "error");
        assert_eq!(
            result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "https://example.com/openapi.json"
        );
        assert!(result["locations"][0]["physicalLocation"]["region"].is_null());
        assert!(
            result["message"]["text"]
                .as_str()
                .unwrap()
                .contains("HTTP 503")
        );
    }

    #[test]
    fn sarif_is_a_valid_empty_report_when_nothing_changed() {
        let report = target("openapi.json", CheckStatus::Unchanged, Vec::new());
        let output = render_sarif(&[report]).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert!(value["runs"][0]["results"].as_array().unwrap().is_empty());
        assert_eq!(
            value["runs"][0]["tool"]["driver"]["rules"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }
}
