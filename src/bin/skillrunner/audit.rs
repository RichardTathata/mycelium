#[cfg(not(feature = "compliance"))]
use bytes::Bytes;
use serde::Serialize;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mycelium::GossipAgent;

use super::llm::ToolCall;

#[derive(Debug, Serialize)]
pub(crate) struct ToolCallSummary {
    pub name:    String,
    pub success: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct AuditRecord {
    pub skill_ns:      String,
    pub skill_name:    String,
    pub caller:        String,
    /// RPC correlation nonce — use as trace ID to correlate request/response.
    pub nonce:         u64,
    pub success:       bool,
    pub duration_ms:   u64,
    pub tool_calls:    Vec<ToolCallSummary>,
    pub ts_unix_nanos: u128,
}

impl AuditRecord {
    pub(crate) fn new(
        skill_ns:    &str,
        skill_name:  &str,
        caller:      &str,
        nonce:       u64,
        success:     bool,
        duration_ms: u64,
        tool_calls:  &[ToolCall],
    ) -> Self {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);

        AuditRecord {
            skill_ns:    skill_ns.to_string(),
            skill_name:  skill_name.to_string(),
            caller:      caller.to_string(),
            nonce,
            success,
            duration_ms,
            tool_calls: tool_calls.iter().map(|tc| ToolCallSummary {
                name:    tc.name.clone(),
                success: true,
            }).collect(),
            ts_unix_nanos: ts,
        }
    }
}

/// Write one invocation audit record.
///
/// **With `compliance` (WS2):** routed into the node's tamper-evident,
/// hash-chained, signed audit trail at `sys/audit/{node}/{seq}` via
/// `GossipAgent::audit` — the rich SkillRunner fields (nonce, duration, tool
/// calls) ride in the record's `detail`. This replaces the old plain trail so
/// there is a single, verifiable audit. Requires `GossipConfig::tls` (records
/// are signed); a missing identity is a misconfiguration and is logged, not
/// silently dropped.
///
/// **Without `compliance`:** falls back to a plain JSON entry at
/// `audit/{ts_unix_nanos}/{node_id}` — no signing/chaining is possible without
/// the identity key, so the legacy time-sorted trail remains. The raw KV routes refuse `audit/`, so the
/// dashboard reads these through this binary's own route ([`invocations_json`]).
pub(crate) fn write_audit(agent: &Arc<GossipAgent>, rec: &AuditRecord) {
    #[cfg(feature = "compliance")]
    {
        let target  = format!("{}/{}", rec.skill_ns, rec.skill_name);
        let outcome = if rec.success {
            mycelium::AuditOutcome::Success
        } else {
            mycelium::AuditOutcome::Error
        };
        let detail = serde_json::json!({
            "nonce":       rec.nonce,
            "duration_ms": rec.duration_ms,
            "tool_calls":  rec.tool_calls.iter().map(|t| &t.name).collect::<Vec<_>>(),
        })
        .to_string();
        if let Err(e) = agent.audit(
            mycelium::AuditAction::Invoke,
            rec.caller.clone(),
            target,
            outcome,
            Some(detail),
        ) {
            tracing::warn!("audit: hash-chained write failed ({e}); set GossipConfig::tls — record not recorded");
        }
    }
    #[cfg(not(feature = "compliance"))]
    {
        // No identity key without compliance → plain, unsigned, time-keyed trail.
        let key = format!("audit/{}/{}", rec.ts_unix_nanos, agent.node_id());
        match serde_json::to_vec(rec) {
            Ok(json) => { let _ = agent.kv().set(key, Bytes::from(json)); }
            Err(e)   => tracing::warn!("audit: serialisation failed: {e}"),
        }
    }
}

/// The dashboard's invocation panel in a build without `compliance`: how many fallback records this node holds
/// under `audit/` and the ten newest, newest first (`GET /gateway/skillrunner/invocations`). `entries` is
/// `agent.kv().scan_prefix("audit/")`; a key is `audit/{ts_unix_nanos}/{node}`, ordered by its timestamp as a
/// number, and a value that is not a JSON object is counted and left out of `recent`. Unsigned records any
/// admitted member can write: a display, not an audit.
#[cfg(not(feature = "compliance"))]
pub(crate) fn invocations_json(entries: Vec<(Arc<str>, Bytes)>) -> serde_json::Value {
    let count = entries.len();
    let mut dated: Vec<(u128, serde_json::Value)> = entries
        .into_iter()
        .filter_map(|(key, value)| {
            let ts = key.strip_prefix("audit/")?.split('/').next()?.parse::<u128>().ok()?;
            let record: serde_json::Value = serde_json::from_slice(&value).ok()?;
            record.is_object().then_some((ts, record))
        })
        .collect();
    dated.sort_by_key(|(ts, _)| std::cmp::Reverse(*ts));
    let recent: Vec<serde_json::Value> = dated.into_iter().take(10).map(|(_, record)| record).collect();
    serde_json::json!({ "count": count, "recent": recent })
}

#[cfg(all(test, not(feature = "compliance")))]
mod tests {
    use super::*;

    /// The panel's route: the count is every record held, `recent` the ten newest by timestamp as a number
    /// (a 9-digit timestamp sorts before a 10-digit one, which a string order gets wrong), junk left out.
    #[test]
    fn invocations_counts_every_record_and_shows_the_ten_newest() {
        let record = |n: u128| {
            let rec = AuditRecord { skill_ns: "ns".into(), skill_name: format!("skill-{n}"), caller: "c".into(), nonce: 1,
                success: n.is_multiple_of(2), duration_ms: 5, tool_calls: vec![], ts_unix_nanos: n };
            (Arc::<str>::from(format!("audit/{n}/10.0.0.5:9000")), Bytes::from(serde_json::to_vec(&rec).unwrap()))
        };
        let mut entries: Vec<_> = [999_999_999u128, 1_000_000_000, 1_000_000_001].into_iter().map(record).collect();
        entries.extend((1..=9u128).map(record));
        entries.push((Arc::from("audit/2000000000/n"), Bytes::from_static(b"not json")));
        let out = invocations_json(entries);
        assert_eq!(out["count"], 13, "every record held, the unreadable one included");
        let names: Vec<&str> = out["recent"].as_array().unwrap().iter().map(|r| r["skill_name"].as_str().unwrap()).collect();
        assert_eq!(names.len(), 10);
        assert_eq!(&names[..3], ["skill-1000000001", "skill-1000000000", "skill-999999999"], "newest first, by number");
        assert_eq!(out["recent"][0]["success"], false);
        assert_eq!(invocations_json(Vec::new()), serde_json::json!({"count": 0, "recent": []}));
    }
}

// ── Optional OTEL export ──────────────────────────────────────────────────────

#[cfg(feature = "otel")]
pub(crate) mod otel {
    use opentelemetry::trace::{Span, SpanKind, Tracer, TracerProvider as _};
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::trace::TracerProvider;

    use super::super::config::OtelSection;
    use super::AuditRecord;

    pub(crate) fn init_tracer(cfg: &OtelSection) -> Result<TracerProvider, Box<dyn std::error::Error>> {
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(&cfg.endpoint)
            .build()?;

        let provider = TracerProvider::builder()
            .with_simple_exporter(exporter)
            .build();

        Ok(provider)
    }

    pub(crate) fn emit_span(provider: &TracerProvider, cfg: &OtelSection, rec: &AuditRecord) {
        let tracer = provider.tracer(cfg.service_name.clone());
        let mut span = tracer
            .span_builder(format!("{}/{}", rec.skill_ns, rec.skill_name))
            .with_kind(SpanKind::Server)
            .start(&tracer);

        span.set_attribute(KeyValue::new("skill.ns",       rec.skill_ns.clone()));
        span.set_attribute(KeyValue::new("skill.name",     rec.skill_name.clone()));
        span.set_attribute(KeyValue::new("caller",         rec.caller.clone()));
        span.set_attribute(KeyValue::new("nonce",          rec.nonce as i64));
        span.set_attribute(KeyValue::new("success",        rec.success));
        span.set_attribute(KeyValue::new("duration_ms",    rec.duration_ms as i64));
        span.set_attribute(KeyValue::new("tool_calls",     rec.tool_calls.len() as i64));
        span.end();
    }
}
