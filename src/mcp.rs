use crate::state::App;
use serde_json::{Value, json};
use std::sync::Arc;
pub async fn handle(app: Arc<App>, request: Value) -> Option<Value> {
    let id = request.get("id").cloned();
    if request["jsonrpc"] != "2.0" || !request["method"].is_string() {
        return Some(
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":"Invalid Request"}}),
        );
    }
    let method = request["method"].as_str().unwrap();
    id.as_ref()?;
    let params = &request["params"];
    let result = match method {
        "initialize" => Ok(
            json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"lessagent","version":env!("CARGO_PKG_VERSION")},"instructions":"Local computer and coding agent. Open a folder with workspace_open, then pass its workspace id to tools. Commands execute on the host."}),
        ),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let mut defs = crate::tools::definitions();
            for d in &mut defs {
                d["parameters"]["properties"]["workspace"] = json!({"type":"string","description":"Workspace id returned by workspace_open/list"});
                d["parameters"]["required"]
                    .as_array_mut()
                    .map(|a| a.push(json!("workspace")))
                    .unwrap_or_else(|| {
                        d["parameters"]["required"] = json!(["workspace"]);
                    });
            }
            defs.extend([
                json!({"name":"workspace_open","description":"Open an existing absolute local folder.","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}),
                json!({"name":"workspace_list","description":"List open workspaces.","parameters":{"type":"object","properties":{}}}),
                json!({"name":"agent_run","description":"Start a persistent autonomous job using configured provider.","parameters":{"type":"object","properties":{"workspace":{"type":"string"},"prompt":{"type":"string"}},"required":["workspace","prompt"]}}),
                json!({"name":"agent_status","description":"Read a job's status and output.","parameters":{"type":"object","properties":{"job_id":{"type":"string"}},"required":["job_id"]}})
            ]);
            Ok(
                json!({"tools":defs.iter().map(|d|json!({"name":d["name"],"description":d["description"],"inputSchema":d["parameters"]})).collect::<Vec<_>>()}),
            )
        }
        "tools/call" => {
            let result = call(app, params).await;
            let (mut v, is_error) = match result {
                Ok(v) => (v, false),
                Err(e) => (json!({"error":e.to_string()}), true),
            };
            let image = v.as_object_mut().and_then(|o| o.remove("image"));
            let mut content = vec![json!({"type":"text","text":v.to_string()})];
            if let Some(i) = image {
                content.push(json!({"type":"image","mimeType":i["mime"],"data":i["data"]}));
            }
            Ok(json!({"content":content,"isError":is_error}))
        }
        _ => Err((-32601, "Method not found")),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc":"2.0","id":id,"result":r}),
        Err((code, message)) => {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
        }
    })
}
async fn call(app: Arc<App>, p: &Value) -> crate::Result<Value> {
    let name = crate::tools::string(p, "name")?;
    let a = &p["arguments"];
    match name {
        "workspace_open" => Ok(serde_json::to_value(
            app.open_workspace(std::path::Path::new(crate::tools::string(a, "path")?))?,
        )?),
        "workspace_list" => Ok(json!(app.disk.lock().unwrap().workspaces)),
        "agent_run" => Ok(
            json!({"job_id":crate::agent::start(app,crate::tools::string(a,"workspace")?,crate::tools::string(a,"prompt")?)?}),
        ),
        "agent_status" => {
            let id = crate::tools::string(a, "job_id")?;
            Ok(json!(
                app.disk
                    .lock()
                    .unwrap()
                    .jobs
                    .iter()
                    .find(|j| j.id == id)
                    .ok_or_else(|| crate::err("Job not found"))?
            ))
        }
        _ => crate::tools::execute(app, crate::tools::string(a, "workspace")?, name, a).await,
    }
}
