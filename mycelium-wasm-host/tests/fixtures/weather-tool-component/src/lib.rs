//! The llm_agent demo's `weather` tool as a capability component (zero-gaps Z4, D4): the same
//! answer `examples/llm_agent.rs`'s in-process handler gives, from inside the sandboxed guest — no
//! node has it compiled in; a stem pulls it by content address, verifies and instantiates it, and
//! the runtime bridges it as the MCP tool `weather`. Kind `describe` answers the tool's own input
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
                "description": "Get current weather for a city",
                "inputSchema": serde_json::from_str::<serde_json::Value>("{\"type\": \"object\", \"properties\": {\"city\": {\"type\": \"string\"}}, \"required\": [\"city\"]}").expect("a literal schema"),
            }));
        }
        let v: serde_json::Value = match serde_json::from_slice(&req.payload) {
            Ok(v) => v,
            Err(e) => return Response { payload: Vec::new(), error: Some(format!("bad json: {e}")) },
        };
        mycelium::host::log::info(&format!("weather: {}", v));
        reply({
        let city = v.get("city").and_then(|c| c.as_str()).unwrap_or("Unknown");
        let temp = 15 + (city.len() as i64 % 20);
        let condition = if temp > 20 { "sunny" } else if temp > 10 { "cloudy" } else { "rainy" };
        serde_json::json!({ "city": city, "temp_c": temp, "condition": condition })
        })
    }
}

export!(Component);
