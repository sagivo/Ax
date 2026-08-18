//! `ax daemon` — JSON protocol over stdin/stdout (spec v0.3 §8.6).
//!
//! One request object per line. Codes are permanent and append-only.
//! Methods: check, type-at, effs, caps, perf, complete, repair, context,
//! digest, explain, search, hole.

use crate::driver::Session;
use crate::perf;
use crate::reach;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Deserialize)]
pub struct Request {
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct Response {
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DaemonError>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DaemonError {
    pub code: i32,
    pub message: String,
}

pub fn handle_line(line: &str) -> String {
    let req: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            return serde_json::to_string(&Response {
                id: None,
                result: None,
                error: Some(DaemonError {
                    code: -32700,
                    message: format!("parse error: {e}"),
                }),
            })
            .unwrap();
        }
    };
    let resp = dispatch(req);
    serde_json::to_string(&resp).unwrap()
}

fn dispatch(req: Request) -> Response {
    let src = req
        .params
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let name = req
        .params
        .get("file")
        .and_then(|v| v.as_str())
        .unwrap_or("daemon.ax");
    let mut s = Session::new();
    s.allow_holes = req
        .params
        .get("allow_holes")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let compiled = s.compile(name, src);
    match req.method.as_str() {
        "check" => match compiled {
            Ok(out) => Response {
                id: req.id,
                result: Some(json!({ "ok": true, "defs": out.fns.len() })),
                error: None,
            },
            Err(d) => Response {
                id: req.id,
                result: Some(json!({ "ok": false, "diagnostics": d })),
                error: None,
            },
        },
        "perf" | "complete" | "context" | "caps" | "effs" | "search" | "hole" | "repair"
        | "digest" | "explain" => match compiled {
            Ok(out) => {
                let result = match req.method.as_str() {
                    "perf" => serde_json::to_value(perf::analyze_module(&s.intern, &out, name)).ok(),
                    "complete" => serde_json::to_value(perf::complete_at(
                        &s.intern,
                        &out,
                        src,
                        req.params.get("at").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                    ))
                    .ok(),
                    "context" => serde_json::to_value(perf::context_pack(&s.intern, &out, 1000)).ok(),
                    "caps" => serde_json::to_value(reach::analyze(&s.intern, &out)).ok(),
                    "effs" => {
                        let id = req.params.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
                        Some(json!({ "text": crate::driver::effs_at(&s.intern, &out, id) }))
                    }
                    "search" => {
                        let q = req.params.get("query").and_then(|v| v.as_str()).unwrap_or("");
                        Some(json!({ "text": crate::driver::search(&s.intern, &out, q) }))
                    }
                    "hole" => Some(json!({ "holes": out.holes.len() })),
                    "repair" => serde_json::to_value(perf::repair(name, src)).ok(),
                    "digest" => Some(json!({
                        "module": out.module,
                        "fns": out.fns.iter().map(|f| s.intern.get(f.sig.name)).collect::<Vec<_>>(),
                    })),
                    "explain" => Some(json!({ "card": crate::driver::card_text() })),
                    _ => None,
                };
                Response {
                    id: req.id,
                    result,
                    error: None,
                }
            }
            Err(d) => Response {
                id: req.id,
                result: None,
                error: Some(DaemonError {
                    code: 1,
                    message: format!("{} diagnostics", d.len()),
                }),
            },
        },
        other => Response {
            id: req.id,
            result: None,
            error: Some(DaemonError {
                code: -32601,
                message: format!("unknown method `{other}`"),
            }),
        },
    }
}
