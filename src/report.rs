use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::db::{Check, Target};
use crate::diff::{Change, ChangeKind};
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
    pub changes: Vec<Change>,
}

const MAX_PRINTED_CHANGES: usize = 40;

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
