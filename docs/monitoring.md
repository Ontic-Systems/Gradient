# Gradient Monitoring & Observability

## Overview

Gradient is a CLI compiler tool — not a web service. Observability means tracking **CI pipeline health**, **build performance**, and **code quality gates**, not uptime or HTTP latency.

This document captures platform choices, SLO definitions, and alerting setup for the pre-production phase. When Gradient grows to include an online service (e.g., a hosted playground or package registry), revisit the web-service section at the end.

---

## Platform Choices

### Metrics
**Platform: GitHub Actions built-in telemetry**

Every workflow run records duration, status, and job-level timing. This is sufficient for pre-production. No external metrics platform is needed until a web service exists.

- CI success rate — visible in the Actions tab and via the API
- Job duration — tracked per-run by GitHub Actions
- Future: if a hosted service is added, adopt **Prometheus + Grafana** (self-hosted) or **Datadog** (managed)

### Logging
**Platform: GitHub Actions workflow logs**

All build, test, and lint output is captured in workflow run logs, retained for 90 days by default. Structured JSON output (`cargo test --message-format json`) is available for post-processing if needed.

- For future hosted services: **Loki** (if self-hosted Grafana stack) or **Datadog Logs**

### Alerting
**Platform: GitHub email notifications + (optional) Slack**

GitHub sends email on workflow failure to the committing author by default. For broader visibility:

1. **Enable branch failure emails**: Repository → Settings → Notifications → "Notify when run fails"
2. **Slack integration** (optional): Install the GitHub app into a `#gradient-ci` Slack channel. Repo → Settings → Notifications → Add Slack webhook.

---

## Service Level Objectives (SLOs)

These SLOs apply to the CI pipeline, which is Gradient's "production" for the pre-deployment phase.

| SLO | Target | Measurement Window |
|-----|--------|-------------------|
| **CI success rate** (`main` branch) | ≥ 95% of pushes pass all jobs | Rolling 30 days |
| **`check` job duration** | ≤ 5 minutes p95 | Rolling 7 days |
| **`e2e` job duration** | ≤ 10 minutes p95 | Rolling 7 days |
| **Test pass rate** | 100% — no flaky tests permitted on `main` | Per-push |
| **Clippy clean** | Zero `-D warnings` violations on `main` | Per-push |

### SLO Breach Definitions

- **CI success rate drops below 95%**: Investigate recent commits for systematic breakage.
- **Job duration exceeds target**: Check for dependency resolution slowdowns or cache misses; verify `Swatinem/rust-cache` is working.
- **Flaky test detected**: Open a `test: fix flaky <test-name>` issue immediately; do not merge until resolved.

---

## Alerting Setup

### CI Failure Alerts (Active Now)

1. **GitHub email**: On by default. Committing author receives email on workflow failure.
2. **GitHub branch protection**: Require status checks to pass before merge — this is the primary alert gate. See [ONT-35](/ONT/issues/ONT-35).

### Future: Hosted Service Alerts

When a web service or package registry is added:

- Alert on **error rate > 1%** (5-minute window)
- Alert on **p99 latency > 500ms** (5-minute window)
- Alert on **uptime < 99.9%** (monthly)
- Dead-man's switch: alert if no heartbeat for 5 minutes

---

## Validating Alerts

Before first production deploy, validate CI alerts fire correctly:

1. Push a commit that deliberately breaks a test (add `assert!(false)` to a test, push to a branch).
2. Confirm the workflow fails and GitHub sends a failure email.
3. Confirm branch protection blocks merge of the failing branch.
4. Revert the commit; confirm CI goes green.

For future web service alerts, validate in staging:
- Inject synthetic errors using a test endpoint.
- Confirm alert fires within the expected window.
- Confirm alert resolves when errors stop.

---

## Future: Web Service Monitoring

When Gradient includes a hosted component (playground, package registry, LSP relay):

| Concern | Recommended Tool |
|---------|-----------------|
| Metrics | Prometheus + Grafana (self-hosted) or Datadog |
| Logs | Loki (Grafana stack) or Datadog Logs |
| Tracing | OpenTelemetry → Jaeger or Datadog APM |
| Uptime | UptimeRobot (free) or Datadog Synthetics |
| Alerting | Grafana Alerts or PagerDuty |
| On-call | PagerDuty or OpsGenie rotation |

Instrument the Rust service with [`metrics`](https://crates.io/crates/metrics) + [`tracing`](https://crates.io/crates/tracing) crates from day one.

---

*Owner: Ops (Infrastructure Lead) — update this document when deployment topology changes.*
