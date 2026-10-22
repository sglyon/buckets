use buckets_plugin::{RpcError, serve_once};
use serde_json::json;

fn main() -> anyhow::Result<()> {
    serve_once(|method, params| match method {
        "commands.list" => Ok(json!([{"id":"hello", "title":"Rust plugin greeting", "description":"SDK example"}])),
        "commands.invoke" if params["request"]["command"] == "hello" => Ok(json!({"message":"Hello from Rust"})),
        _ => Err(RpcError { code: -32601, message: format!("Unknown method or command: {method}") }),
    })
}
