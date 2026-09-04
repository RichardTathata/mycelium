//! Shared test doubles for the integration suites.
#![allow(dead_code)]

use std::time::Duration;

/// A backend that holds each call open for a fixed time — long enough for concurrent calls
/// to overlap, so reservation behaviour is observable rather than timing-dependent.
pub struct SlowEcho(pub Duration);

#[async_trait::async_trait]
impl mycelium::LlmBackend for SlowEcho {
    async fn complete(
        &self,
        _system: &str,
        user: &str,
        _max_tokens: u32,
        _temperature: f32,
    ) -> Result<mycelium::LlmResult, mycelium::LlmError> {
        tokio::time::sleep(self.0).await;
        Ok(mycelium::LlmResult { output: format!("slow: {user}"), model_used: "slow".into(), tokens_used: 1 })
    }
}
