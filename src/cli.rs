use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use clap::{Args, Parser, Subcommand};
use reqwest::header::{HeaderName, HeaderValue};

use crate::db::{self, NewCheck, NewSnapshot, Store, Target};
use crate::diff;
use crate::fetch;
use crate::report::{self, CheckStatus, TargetReport};
use crate::spec::{self, Format};

#[derive(Debug, Parser)]
#[command(
    name = "apiwatch",
    version,
    about = "Catch breaking changes in the APIs you depend on",
    long_about = "API Watch stores a baseline of an OpenAPI or JSON document, then compares each \
                  new fetch against it. Breaking changes are reported before they reach production.",
    after_help = "EXAMPLES:\n  \
        apiwatch add https://petstore3.swagger.io/api/v3/openapi.json --name petstore\n  \
        apiwatch check petstore\n  \
        apiwatch check --fail-on-breaking\n  \
        apiwatch verify petstore ./openapi.yaml --fail-on-breaking\n  \
        apiwatch list\n  \
        apiwatch history petstore --limit 20"
)]
pub struct Cli {
    /// Path to the SQLite database (default: ~/.apiwatch/apiwatch.db)
    #[arg(long, global = true, env = "APIWATCH_DB", value_name = "PATH")]
    pub db: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Track a new API spec by URL or local file
    Add(AddArgs),
    /// Stop tracking a target and delete its history
    #[command(alias = "rm")]
    Remove {
        /// Target name
        name: String,
    },
    /// List tracked targets and their last check
    #[command(alias = "ls")]
    List {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Fetch specs and report contract changes
    Check(CheckArgs),
    /// Compare a candidate spec against the saved baseline without recording a check
    Verify(VerifyArgs),
    /// Show the check history for one target
    History(HistoryArgs),
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// URL or local path to an OpenAPI 3.x, Swagger 2.0, or JSON document
    pub source: String,

    /// Target name (default: derived from the URL)
    #[arg(long)]
    pub name: Option<String>,

    /// Extra HTTP header, for example "Authorization: Bearer $TOKEN"
    #[arg(short = 'H', long = "header", value_name = "NAME: VALUE")]
    pub headers: Vec<String>,
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Check only this target (default: all targets)
    pub name: Option<String>,

    /// Print machine-readable JSON
    #[arg(long, conflicts_with = "sarif")]
    pub json: bool,

    /// Print a SARIF 2.1.0 report for GitHub code scanning
    #[arg(long)]
    pub sarif: bool,

    /// Exit with code 2 when breaking changes are found
    #[arg(long)]
    pub fail_on_breaking: bool,
}

#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// Tracked target whose latest snapshot is the baseline
    pub name: String,

    /// Candidate spec: URL or local path to an OpenAPI, Swagger, or JSON document
    pub source: String,

    /// Extra HTTP header, for example "Authorization: Bearer $TOKEN"
    #[arg(short = 'H', long = "header", value_name = "NAME: VALUE")]
    pub headers: Vec<String>,

    /// Print machine-readable JSON
    #[arg(long, conflicts_with = "sarif")]
    pub json: bool,

    /// Print a SARIF 2.1.0 report for GitHub code scanning
    #[arg(long)]
    pub sarif: bool,

    /// Exit with code 2 when breaking changes are found
    #[arg(long)]
    pub fail_on_breaking: bool,
}

#[derive(Debug, Args)]
pub struct HistoryArgs {
    /// Target name
    pub name: String,

    /// Number of checks to show
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=1000))]
    pub limit: u32,

    /// Print machine-readable JSON
    #[arg(long)]
    pub json: bool,
}

pub fn run(cli: Cli) -> Result<i32> {
    let Cli { db, command } = cli;
    let db_path = db.unwrap_or_else(db::default_db_path);
    let mut store =
        Store::open(&db_path).with_context(|| format!("opening database {}", db_path.display()))?;

    match command {
        Command::Add(args) => add(&store, args),
        Command::Remove { name } => remove(&store, &name),
        Command::List { json } => {
            let targets = store.targets()?;
            report::print_targets(&targets, json)?;
            Ok(0)
        }
        Command::Check(args) => check(&mut store, args),
        Command::Verify(args) => verify(&store, args),
        Command::History(args) => history(&store, args),
    }
}

fn add(store: &Store, args: AddArgs) -> Result<i32> {
    let source = args.source.trim();
    if source.is_empty() {
        bail!("source cannot be empty");
    }

    let name = args
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| spec::derive_name(source));
    validate_name(&name)?;

    let headers = parse_headers(&args.headers)?;
    let (target, already_tracked) = store.add_target(&name, source, &headers, &now())?;
    if already_tracked {
        report::print_already_tracked(&target);
    } else {
        report::print_added(&target);
    }
    Ok(0)
}

fn remove(store: &Store, name: &str) -> Result<i32> {
    let target = store
        .target(name)?
        .with_context(|| format!("target '{name}' not found"))?;
    let snapshots = store.snapshot_count(target.id)?;
    store.remove_target(name)?;
    report::print_removed(&target.name);
    if snapshots > 0 {
        println!(
            "{}",
            crate::style::dim(&format!(
                "Deleted {snapshots} snapshot{} and all check history.",
                if snapshots == 1 { "" } else { "s" }
            ))
        );
    }
    Ok(0)
}

fn check(store: &mut Store, args: CheckArgs) -> Result<i32> {
    let targets = match args.name.as_deref() {
        Some(name) => vec![
            store
                .target(name)?
                .with_context(|| format!("target '{name}' not found"))?,
        ],
        None => store.targets()?,
    };

    if targets.is_empty() {
        bail!("no targets yet. Add one with `apiwatch add https://example.com/openapi.json`");
    }

    let mut reports = Vec::with_capacity(targets.len());
    for target in targets {
        reports.push(check_target(store, &target)?);
    }

    if args.sarif {
        print!("{}", report::render_sarif(&reports)?);
    } else {
        report::print_checks(&reports, args.json)?;
    }

    let has_error = reports
        .iter()
        .any(|report| report.status == CheckStatus::Error);
    let has_breaking = reports.iter().any(|report| report.breaking > 0);

    if has_error {
        return Ok(1);
    }
    if args.fail_on_breaking && has_breaking {
        return Ok(2);
    }
    Ok(0)
}

fn verify(store: &Store, args: VerifyArgs) -> Result<i32> {
    let target = store
        .target(&args.name)?
        .with_context(|| format!("target '{}' not found", args.name))?;
    let baseline = store.latest_snapshot(target.id)?.with_context(|| {
        format!(
            "target '{}' has no baseline yet. Run `apiwatch check {}` first.",
            target.name, target.name
        )
    })?;

    let source = args.source.trim();
    if source.is_empty() {
        bail!("source cannot be empty");
    }
    let headers = parse_headers(&args.headers)?;
    let fetched = fetch::fetch(source, &headers, None)
        .with_context(|| format!("reading candidate spec {source}"))?;
    let body = fetched
        .body
        .context("the candidate returned success but no response body")?;

    let (value, format) =
        spec::parse_document(&body).with_context(|| format!("parsing candidate spec {source}"))?;
    let baseline_format = format_from_db(&baseline.format)?;
    if baseline_format != format {
        bail!(
            "candidate is {} but the baseline is {}. Remove and re-add the target to reset the baseline.",
            format.label(),
            baseline_format.label()
        );
    }
    let flattened = spec::flatten(&value, format)
        .with_context(|| format!("flattening candidate spec {source}"))?;

    let (baseline_value, _) =
        spec::parse_document(&baseline.content).context("could not parse the stored baseline")?;
    let baseline_flat = spec::flatten(&baseline_value, baseline_format)
        .context("could not flatten the stored baseline")?;

    let changes = diff::diff(&baseline_flat.facts, &flattened.facts);
    let status = if changes.is_empty() {
        CheckStatus::Unchanged
    } else {
        CheckStatus::Changed
    };
    let message = if changes.is_empty() {
        format!(
            "candidate matches the baseline captured {}",
            baseline.captured_at
        )
    } else {
        format!(
            "candidate compared with the baseline captured {}",
            baseline.captured_at
        )
    };

    // The report points at the candidate file so SARIF annotations land on the
    // spec under review, not on the stored target URL.
    let mut display = target.clone();
    display.url = source.to_string();
    let report = report_from_flattened(
        &display,
        status,
        now(),
        format,
        &flattened,
        fetched.http_status,
        Some(message),
        changes,
    );
    let breaking = report.breaking;

    if args.sarif {
        print!("{}", report::render_sarif(&[report])?);
    } else {
        report::print_checks(&[report], args.json)?;
    }

    if args.fail_on_breaking && breaking > 0 {
        return Ok(2);
    }
    Ok(0)
}

fn check_target(store: &mut Store, target: &Target) -> Result<TargetReport> {
    let checked_at = now();
    let previous = store.latest_snapshot(target.id)?;
    let previous_etag = previous
        .as_ref()
        .and_then(|snapshot| snapshot.etag.as_deref());

    let fetched = match fetch::fetch(&target.url, &target.headers, previous_etag) {
        Ok(fetched) => fetched,
        Err(error) => {
            return error_report(store, target, &checked_at, None, format!("{error:#}"));
        }
    };

    if fetched.not_modified {
        let Some(previous) = previous else {
            return error_report(
                store,
                target,
                &checked_at,
                fetched.http_status,
                "server returned 304 but no baseline exists".to_string(),
            );
        };
        let format = format_from_db(&previous.format)?;
        let message = "not modified (ETag matched)".to_string();
        store.record_check(
            target.id,
            NewCheck {
                checked_at: &checked_at,
                status: "unchanged",
                http_status: fetched.http_status,
                breaking: 0,
                compatible: 0,
                message: Some(&message),
                snapshot: None,
                changes: &[],
            },
        )?;
        return Ok(TargetReport {
            name: target.name.clone(),
            url: target.url.clone(),
            status: CheckStatus::Unchanged,
            checked_at,
            format: Some(format.label().to_string()),
            title: None,
            operations: None,
            facts: Some(previous.fact_count as usize),
            breaking: 0,
            compatible: 0,
            http_status: fetched.http_status,
            message: Some(message),
            unresolved_refs: 0,
            truncated: false,
            changes: Vec::new(),
        });
    }

    let body = fetched
        .body
        .context("the server returned success but no response body")?;
    let (value, format) = match spec::parse_document(&body) {
        Ok(parsed) => parsed,
        Err(error) => {
            return error_report(
                store,
                target,
                &checked_at,
                fetched.http_status,
                format!("{error:#}"),
            );
        }
    };

    if let Some(previous) = &previous {
        let previous_format = format_from_db(&previous.format)?;
        if previous_format != format {
            return error_report(
                store,
                target,
                &checked_at,
                fetched.http_status,
                format!(
                    "document is now {} but the baseline is {}. Refusing to overwrite the baseline; remove and re-add the target to reset it.",
                    format.label(),
                    previous_format.label()
                ),
            );
        }
    }

    let flattened = match spec::flatten(&value, format) {
        Ok(flattened) => flattened,
        Err(error) => {
            return error_report(
                store,
                target,
                &checked_at,
                fetched.http_status,
                format!("{error:#}"),
            );
        }
    };

    let Some(previous) = previous else {
        let snapshot = NewSnapshot {
            captured_at: &checked_at,
            format: format.as_str(),
            etag: fetched.etag.as_deref(),
            content: &body,
            fact_count: flattened.fact_count(),
        };
        store.record_check(
            target.id,
            NewCheck {
                checked_at: &checked_at,
                status: "baseline",
                http_status: fetched.http_status,
                breaking: 0,
                compatible: 0,
                message: None,
                snapshot: Some(snapshot),
                changes: &[],
            },
        )?;
        return Ok(report_from_flattened(
            target,
            CheckStatus::Baseline,
            checked_at,
            format,
            &flattened,
            fetched.http_status,
            None,
            Vec::new(),
        ));
    };

    if previous.content == body {
        let message = "content unchanged (server did not return 304)".to_string();
        store.record_check(
            target.id,
            NewCheck {
                checked_at: &checked_at,
                status: "unchanged",
                http_status: fetched.http_status,
                breaking: 0,
                compatible: 0,
                message: Some(&message),
                snapshot: None,
                changes: &[],
            },
        )?;
        return Ok(report_from_flattened(
            target,
            CheckStatus::Unchanged,
            checked_at,
            format,
            &flattened,
            fetched.http_status,
            Some(message),
            Vec::new(),
        ));
    }

    let previous_value = match spec::parse_document(&previous.content) {
        Ok((value, _)) => value,
        Err(error) => {
            return error_report(
                store,
                target,
                &checked_at,
                fetched.http_status,
                format!("could not parse the stored baseline: {error:#}"),
            );
        }
    };
    let previous_format = format_from_db(&previous.format)?;
    let previous_flat = match spec::flatten(&previous_value, previous_format) {
        Ok(flattened) => flattened,
        Err(error) => {
            return error_report(
                store,
                target,
                &checked_at,
                fetched.http_status,
                format!("could not flatten the stored baseline: {error:#}"),
            );
        }
    };

    let changes = diff::diff(&previous_flat.facts, &flattened.facts);
    let breaking = changes.iter().filter(|change| change.is_breaking()).count();
    let compatible = changes.len() - breaking;
    let status = if changes.is_empty() {
        CheckStatus::Unchanged
    } else {
        CheckStatus::Changed
    };
    let message = if changes.is_empty() {
        Some("no contract changes (descriptions or formatting only)".to_string())
    } else {
        None
    };

    let snapshot = NewSnapshot {
        captured_at: &checked_at,
        format: format.as_str(),
        etag: fetched.etag.as_deref(),
        content: &body,
        fact_count: flattened.fact_count(),
    };
    store.record_check(
        target.id,
        NewCheck {
            checked_at: &checked_at,
            status: report::status_name(status),
            http_status: fetched.http_status,
            breaking,
            compatible,
            message: message.as_deref(),
            snapshot: Some(snapshot),
            changes: &changes,
        },
    )?;

    Ok(report_from_flattened(
        target,
        status,
        checked_at,
        format,
        &flattened,
        fetched.http_status,
        message,
        changes,
    ))
}

#[allow(clippy::too_many_arguments)]
fn report_from_flattened(
    target: &Target,
    status: CheckStatus,
    checked_at: String,
    format: Format,
    flattened: &spec::Flattened,
    http_status: Option<u16>,
    message: Option<String>,
    changes: Vec<diff::Change>,
) -> TargetReport {
    let breaking = changes.iter().filter(|change| change.is_breaking()).count();
    let compatible = changes.len() - breaking;
    TargetReport {
        name: target.name.clone(),
        url: target.url.clone(),
        status,
        checked_at,
        format: Some(format.label().to_string()),
        title: flattened.title.clone(),
        operations: Some(flattened.operations),
        facts: Some(flattened.fact_count()),
        breaking,
        compatible,
        http_status,
        message,
        unresolved_refs: flattened.unresolved_refs,
        truncated: flattened.truncated,
        changes,
    }
}

fn error_report(
    store: &mut Store,
    target: &Target,
    checked_at: &str,
    http_status: Option<u16>,
    message: String,
) -> Result<TargetReport> {
    store.record_check(
        target.id,
        NewCheck {
            checked_at,
            status: "error",
            http_status,
            breaking: 0,
            compatible: 0,
            message: Some(&message),
            snapshot: None,
            changes: &[],
        },
    )?;

    Ok(TargetReport {
        name: target.name.clone(),
        url: target.url.clone(),
        status: CheckStatus::Error,
        checked_at: checked_at.to_string(),
        format: None,
        title: None,
        operations: None,
        facts: None,
        breaking: 0,
        compatible: 0,
        http_status,
        message: Some(message),
        unresolved_refs: 0,
        truncated: false,
        changes: Vec::new(),
    })
}

fn history(store: &Store, args: HistoryArgs) -> Result<i32> {
    let target = store
        .target(&args.name)?
        .with_context(|| format!("target '{}' not found", args.name))?;
    let checks = store.history(target.id, args.limit as usize)?;
    report::print_history(&target, &checks, args.json)?;
    Ok(0)
}

fn parse_headers(values: &[String]) -> Result<BTreeMap<String, String>> {
    let mut headers = BTreeMap::new();
    for raw in values {
        let (name, value) = raw.split_once(':').with_context(|| {
            format!("invalid header '{raw}': expected a value like 'Authorization: Bearer ...'")
        })?;
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() {
            bail!("invalid header '{raw}': name is empty");
        }
        HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid header name '{name}'"))?;
        HeaderValue::from_str(value)
            .with_context(|| format!("invalid value for header '{name}'"))?;
        if headers
            .keys()
            .any(|existing: &String| existing.eq_ignore_ascii_case(name))
        {
            bail!("duplicate header '{name}'");
        }
        headers.insert(name.to_string(), value.to_string());
    }
    Ok(headers)
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("target name cannot be empty");
    }
    if name.chars().count() > 64 {
        bail!("target name must be 64 characters or fewer");
    }
    if !name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
    {
        bail!("target names may only contain letters, digits, '-', '_' and '.'");
    }
    Ok(())
}

fn format_from_db(value: &str) -> Result<Format> {
    match value {
        "openapi" => Ok(Format::OpenApi),
        "json" => Ok(Format::Json),
        other => bail!("unknown stored format '{other}'"),
    }
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_header_names() {
        let error = parse_headers(&["bad name: value".to_string()]).unwrap_err();
        assert!(error.to_string().contains("invalid header name"));
    }

    #[test]
    fn parses_headers_with_colons_in_values() {
        let headers = parse_headers(&["X-Webhook: https://example.com/hook".to_string()]).unwrap();
        assert_eq!(
            headers.get("X-Webhook").map(String::as_str),
            Some("https://example.com/hook")
        );
    }

    #[test]
    fn rejects_duplicate_headers_case_insensitively() {
        let error = parse_headers(&[
            "Authorization: one".to_string(),
            "authorization: two".to_string(),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("duplicate header"));
    }

    #[test]
    fn validates_target_names() {
        assert!(validate_name("my-api").is_ok());
        assert!(validate_name("my api").is_err());
        assert!(validate_name("").is_err());
    }
}
