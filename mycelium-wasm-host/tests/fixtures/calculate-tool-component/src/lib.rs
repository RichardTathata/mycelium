//! The llm_agent demo's `calculate` tool as a capability component (zero-gaps Z4, D4): the same
//! answer `examples/llm_agent.rs`'s in-process handler gives, from inside the sandboxed guest — no
//! node has it compiled in; a stem pulls it by content address, verifies and instantiates it, and
//! the runtime bridges it as the MCP tool `calculate`. Kind `describe` answers the tool's own input
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
                "description": "Evaluate a simple arithmetic expression like '3 + 4'",
                "inputSchema": serde_json::from_str::<serde_json::Value>("{\"type\": \"object\", \"properties\": {\"expression\": {\"type\": \"string\"}}, \"required\": [\"expression\"]}").expect("a literal schema"),
            }));
        }
        let v: serde_json::Value = match serde_json::from_slice(&req.payload) {
            Ok(v) => v,
            Err(e) => return Response { payload: Vec::new(), error: Some(format!("bad json: {e}")) },
        };
        mycelium::host::log::info(&format!("calculate: {}", v));
        reply({
        let expr = v.get("expression").and_then(|e| e.as_str()).unwrap_or("0");
        let result: f64 = (|| {
            let p: Vec<&str> = expr.split_whitespace().collect();
            if p.len() == 3 {
                let a: f64 = p[0].parse().ok()?;
                let b: f64 = p[2].parse().ok()?;
                match p[1] {
                    "+" => Some(a + b), "-" => Some(a - b), "*" => Some(a * b),
                    "/" => if b != 0.0 { Some(a / b) } else { None },
                    _ => None,
                }
            } else { None }
        })().unwrap_or(f64::NAN);
        serde_json::json!({ "expression": expr, "result": result })
        })
    }
}

export!(Component);
