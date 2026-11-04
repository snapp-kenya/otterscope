//! Prometheus text exposition (port of `metrics.go`). Hand-rolled on purpose:
//! aggregate counts only, no client dependency, no run content.

use crate::store::Metrics;

pub fn render(version: &str, metrics: &Metrics) -> String {
    let mut out = String::new();
    out.push_str("# HELP otterscope_build_info Build information.\n");
    out.push_str("# TYPE otterscope_build_info gauge\n");
    out.push_str(&format!("otterscope_build_info{{version={version:?}}} 1\n"));
    metric(
        &mut out,
        "otterscope_runs_total",
        "gauge",
        "Total runs stored.",
        metrics.runs,
        "",
    );
    metric(
        &mut out,
        "otterscope_runs",
        "gauge",
        "Runs by status.",
        metrics.runs_ok,
        r#"{status="ok"}"#,
    );
    metric(
        &mut out,
        "otterscope_runs",
        "gauge",
        "Runs by status.",
        metrics.runs_error,
        r#"{status="error"}"#,
    );
    metric(
        &mut out,
        "otterscope_runs",
        "gauge",
        "Runs by status.",
        metrics.runs_running,
        r#"{status="running"}"#,
    );
    metric(
        &mut out,
        "otterscope_steps_total",
        "gauge",
        "Total steps stored.",
        metrics.steps,
        "",
    );
    metric(
        &mut out,
        "otterscope_projects_total",
        "gauge",
        "Number of projects.",
        metrics.projects,
        "",
    );
    metric(
        &mut out,
        "otterscope_alerts_firing",
        "gauge",
        "Alerts currently firing.",
        metrics.alerts_firing,
        "",
    );
    metric(
        &mut out,
        "otterscope_db_size_bytes",
        "gauge",
        "SQLite database file size in bytes.",
        metrics.db_size_bytes,
        "",
    );
    out
}

fn metric(out: &mut String, name: &str, kind: &str, help: &str, value: i64, labels: &str) {
    out.push_str(&format!(
        "# HELP {name} {help}\n# TYPE {name} {kind}\n{name}{labels} {value}\n"
    ));
}
