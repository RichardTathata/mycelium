//! The llm_agent demo's `search` tool as a capability component (zero-gaps Z4, D4): the same
//! answer `examples/llm_agent.rs`'s in-process handler gives, from inside the sandboxed guest — no
//! node has it compiled in; a stem pulls it by content address, verifies and instantiates it, and
//! the runtime bridges it as the MCP tool `search`. Kind `describe` answers the tool's own input
//! schema and description, so the bridge publishes a real schema rather than a generic one.

wit_bindgen::generate!({
    world: "capability-component",
    path: "../../../wit",
});

use exports::mycelium::host::capability::{Guest, Request, Response};

struct Component;

fn reply(v: serde_json::Value) -> Response {
    Response { payload: v.to_string().into_bytes(), error: None }
}

impl Guest for Component {
    fn handle(req: Request) -> Response {
        if req.kind == "describe" {
            return reply(serde_json::json!({
                "description": "Search the web for information",
                "inputSchema": serde_json::from_str::<serde_json::Value>("{\"type\": \"object\", \"properties\": {\"query\": {\"type\": \"string\"}}, \"required\": [\"query\"]}").expect("a literal schema"),
            }));
        }
        let v: serde_json::Value = match serde_json::from_slice(&req.payload) {
            Ok(v) => v,
            Err(e) => return Response { payload: Vec::new(), error: Some(format!("bad json: {e}")) },
        };
        mycelium::host::log::info(&format!("search: {}", v));
        reply({
        let q = v.get("query").and_then(|q| q.as_str()).unwrap_or("");
        serde_json::json!({ "query": q, "results": [
            { "title": format!("{q} — Overview"), "url": "https://example.com/1" },
            { "title": format!("{q} — Deep dive"), "url": "https://example.com/2" },
        ]})
        })
    }
}

export!(Component);
