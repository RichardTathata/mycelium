//! `[[serve]]` for a stem (design-time-tooling.md §16, X2): the declared form of
//! `mycelium-reason`'s `serve_model` bridge. While this node hosts the install a section names in
//! `while_live`, the prompt skill `{ns}/{name}` is registered over an OpenAI-compatible backend, so
//! an `InferenceRouter` resolves and calls it over `llm.invoke`; when the install goes — its probe
//! failed, a floor moved, the node is being drained — the skill is retracted with it and the router
//! fails over. Feature `llm`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use mycelium::{CapFilter, GossipAgent, LlmBackend, OpenAiBackend, PromptSkillHandle, PromptTemplate, ServeDecl};

fn is_live_here(agent: &GossipAgent, decl: &ServeDecl) -> bool {
    let Some(w) = &decl.while_live else { return true };
    let me = agent.node_id();
    agent.capabilities().resolve(&CapFilter::new(w.ns.as_str(), w.name.as_str())).iter().any(|(n, _)| n == me)
}

/// One task for all of a unit's `[[serve]]` sections: every `tick`, register what is live here and
/// retract what is not. Abort the task to retract everything.
///
/// `decls` carries each section with its bearer already resolved (`stem::resolve_serve_key`, at
/// start): a literal, a variable's value, or none. The key is never logged.
pub fn spawn(agent: Arc<GossipAgent>, decls: Vec<(ServeDecl, Option<String>)>, tick: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut held: HashMap<usize, PromptSkillHandle> = HashMap::new();
        loop {
            for (i, (d, key)) in decls.iter().enumerate() {
                let live = is_live_here(&agent, d);
                if live && !held.contains_key(&i) {
                    let backend: Arc<dyn LlmBackend> = Arc::new(OpenAiBackend::new(
                        d.endpoint.clone(),
                        key.clone().unwrap_or_else(|| "none".into()),
                        d.model.clone(),
                    ));
                    let template = PromptTemplate {
                        system: String::new(),
                        user_template: "{{input}}".into(),
                        max_tokens: d.max_tokens.unwrap_or(256),
                        temperature: d.temperature.unwrap_or(0.7),
                        metadata: HashMap::new(),
                    };
                    match agent.llm().register_prompt_skill(d.ns.as_str(), d.name.as_str(), template, backend).await {
                        Ok(h) => {
                            tracing::info!(skill = %format!("{}/{}", d.ns, d.name), "serving: the install it waits for is live here");
                            held.insert(i, h);
                        }
                        Err(e) => tracing::warn!(error = %e, "serve: registering the prompt skill failed — retrying next tick"),
                    }
                } else if !live && held.remove(&i).is_some() {
                    tracing::info!(skill = %format!("{}/{}", d.ns, d.name), "serve: the install went away here — skill retracted");
                }
            }
            tokio::time::sleep(tick).await;
        }
    })
}
