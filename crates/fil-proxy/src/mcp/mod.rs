//! `fil mcp`: a stdio MCP server that lets coding agents notify the user's
//! phone and ask them multiple-choice questions.
//!
//! Hand-written JSON-RPC: the server exposes three tools and needs nothing
//! from an SDK but framing. It relays each call to fil-daemon over the IPC
//! socket and holds no state of its own.

mod daemon;
pub mod install;

use daemon::DaemonClient;
use fil_protocol::agent::{self, AgentReply, AgentRequest};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// Newest first; the first entry is offered when the client asks for an
/// unknown version.
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const PROGRESS_INTERVAL: Duration = Duration::from_secs(20);

const INSTRUCTIONS: &str = "Fil mirrors this terminal to the user's iPhone. \
Use `notify` once when a long task finishes or fails, or when you are blocked, \
and `ask` when you need a decision to continue while the user may be away from \
the screen. Never use them for routine progress updates.";

const NOTIFY_DESCRIPTION: &str = "Send a push notification to the user's iPhone \
through the Fil app. Tapping it opens this exact terminal session on the phone.\n\n\
Use it when the user is probably not watching the screen and something needs \
their attention: a long-running task (build, full test suite, migration, deploy, \
large refactor, anything that took more than a few minutes) has finished or \
failed, or you are blocked and cannot continue without them.\n\n\
Do NOT use it for routine progress updates, after quick tasks, or while the user \
is actively chatting with you. One notification at the end of a unit of work is \
the norm; never send several in a row.\n\n\
Keep the title to a few words (\"Tests passing\", \"Deploy failed\") and the \
message to one or two sentences: the outcome, and what the user needs to do, if anything.";

const ASK_DESCRIPTION: &str = "Ask the user a multiple-choice question on their \
iPhone and wait for the answer. It arrives as a push notification with one button \
per option; the user can also answer from the Fil app. Returns the chosen option \
text exactly as you wrote it.\n\n\
Use it instead of stopping at a terminal prompt when you need a decision to \
continue and the user may be away: confirming a risky or irreversible action \
(deploy, delete data, force-push, run a migration), choosing between approaches, \
or unblocking a long autonomous task.\n\n\
Give 2-4 short, distinct options (40 characters max each); put the context in \
the question. Do not use it for open-ended input, or for questions the user is \
already answering in the chat.\n\n\
If the result says the question is still pending, call `get_answer` with the \
returned question_id (or continue with work that does not depend on it). If it \
expired unanswered, take the safest option or stop and explain what you need.";

const GET_ANSWER_DESCRIPTION: &str = "Wait for the answer to a question asked \
earlier with `ask` that returned \"pending\". Returns the chosen option, \
\"pending\" again if the user still has not answered, or \"expired\".";

pub async fn serve() -> anyhow::Result<()> {
    let daemon = Arc::new(DaemonClient::from_env());
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(message) = out_rx.recv().await {
            let mut line = serde_json::to_vec(&message).unwrap_or_default();
            line.push(b'\n');
            if stdout.write_all(&line).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });

    let server = Server {
        daemon,
        out: out_tx,
        inflight: Arc::default(),
    };
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        server.dispatch(&line);
    }

    // The client closed stdin: abandon calls in flight (questions stay open on
    // the daemon until they expire) and let the writer drain.
    for (_, task) in server.inflight.lock().unwrap().drain() {
        task.abort();
    }
    drop(server);
    let _ = writer.await;
    Ok(())
}

struct Server {
    daemon: Arc<DaemonClient>,
    out: mpsc::UnboundedSender<Value>,
    inflight: Arc<Mutex<HashMap<String, tokio::task::AbortHandle>>>,
}

impl Server {
    fn dispatch(&self, line: &str) {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(_) => {
                self.send(error_response(Value::Null, -32700, "parse error"));
                return;
            }
        };
        let id = message.get("id").cloned();
        let method = message.get("method").and_then(Value::as_str);
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        let (Some(id), Some(method)) = (id, method) else {
            // Notifications and responses to requests we never send.
            if method == Some("notifications/cancelled")
                && let Some(request_id) = params.get("requestId")
                && let Some(task) = self
                    .inflight
                    .lock()
                    .unwrap()
                    .remove(&request_id.to_string())
            {
                task.abort();
            }
            return;
        };

        if method == "tools/call" {
            self.spawn_tool_call(id, params);
        } else {
            self.send(handle_request(&id, method, &params));
        }
    }

    fn spawn_tool_call(&self, id: Value, params: Value) {
        let key = id.to_string();
        let daemon = self.daemon.clone();
        let out = self.out.clone();
        let inflight = self.inflight.clone();
        let progress_token = params.pointer("/_meta/progressToken").cloned();
        let task_key = key.clone();
        // Held across the spawn so a call that finishes instantly cannot
        // remove itself before it is registered.
        let mut registry = self.inflight.lock().unwrap();
        let task = tokio::spawn(async move {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            let progress = progress_token.map(|token| spawn_progress(token, out.clone()));
            let response = match build_request(name, &arguments) {
                Ok(request) => {
                    let reply = daemon.request(&request).await;
                    success_response(&id, tool_result(reply))
                }
                Err(ToolError::UnknownTool) => {
                    error_response(id.clone(), -32602, &format!("unknown tool: {name}"))
                }
                Err(ToolError::Invalid(message)) => success_response(&id, error_result(&message)),
            };
            if let Some(progress) = progress {
                progress.abort();
            }
            inflight.lock().unwrap().remove(&task_key);
            let _ = out.send(response);
        });
        registry.insert(key, task.abort_handle());
    }

    fn send(&self, message: Value) {
        let _ = self.out.send(message);
    }
}

/// Keeps clients that track per-request liveness from giving up on a long
/// `ask` while the user decides.
fn spawn_progress(token: Value, out: mpsc::UnboundedSender<Value>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = 0u64;
        loop {
            tokio::time::sleep(PROGRESS_INTERVAL).await;
            tick += 1;
            let notification = json!({
                "jsonrpc": "2.0",
                "method": "notifications/progress",
                "params": {
                    "progressToken": token,
                    "progress": tick,
                    "message": "Waiting for the user's answer on their phone",
                },
            });
            if out.send(notification).is_err() {
                return;
            }
        }
    })
}

fn handle_request(id: &Value, method: &str, params: &Value) -> Value {
    match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let version = PROTOCOL_VERSIONS
                .iter()
                .find(|version| **version == requested)
                .unwrap_or(&PROTOCOL_VERSIONS[0]);
            success_response(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": {
                        "name": "fil",
                        "title": "Fil",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => success_response(id, json!({})),
        "tools/list" => success_response(id, json!({ "tools": tool_definitions() })),
        _ => error_response(id.clone(), -32601, &format!("method not found: {method}")),
    }
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "notify",
            "title": "Notify the user's phone",
            "description": NOTIFY_DESCRIPTION,
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "A few words, e.g. \"Tests passing\" or \"Build failed\".",
                        "maxLength": agent::MAX_TITLE_CHARS,
                    },
                    "message": {
                        "type": "string",
                        "description": "One or two sentences: what happened and what the user needs to do, if anything.",
                        "maxLength": agent::MAX_BODY_CHARS,
                    },
                },
                "required": ["message"],
                "additionalProperties": false,
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": true,
            },
        },
        {
            "name": "ask",
            "title": "Ask the user on their phone",
            "description": ASK_DESCRIPTION,
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "The question, with the context needed to decide.",
                        "maxLength": agent::MAX_BODY_CHARS,
                    },
                    "options": {
                        "type": "array",
                        "items": { "type": "string", "maxLength": agent::MAX_OPTION_CHARS },
                        "minItems": agent::MIN_OPTIONS,
                        "maxItems": agent::MAX_OPTIONS,
                        "description": "2-4 short, distinct choices, shown as notification buttons.",
                    },
                    "title": {
                        "type": "string",
                        "description": "Optional short heading, e.g. \"Deploy to production?\".",
                        "maxLength": agent::MAX_TITLE_CHARS,
                    },
                    "timeout_seconds": {
                        "type": "integer",
                        "minimum": agent::MIN_ASK_TIMEOUT_SECS,
                        "maximum": agent::MAX_ASK_TIMEOUT_SECS,
                        "description": "How long the question stays answerable. Default 600.",
                    },
                },
                "required": ["question", "options"],
                "additionalProperties": false,
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": true,
            },
        },
        {
            "name": "get_answer",
            "title": "Wait for an answer",
            "description": GET_ANSWER_DESCRIPTION,
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question_id": {
                        "type": "string",
                        "description": "The question_id returned by `ask`.",
                    },
                },
                "required": ["question_id"],
                "additionalProperties": false,
            },
            "annotations": {
                "readOnlyHint": true,
                "openWorldHint": true,
            },
        },
    ])
}

enum ToolError {
    UnknownTool,
    Invalid(String),
}

fn string_arg(arguments: &Value, name: &str) -> Option<String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn env_secs(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn build_request(name: &str, arguments: &Value) -> Result<AgentRequest, ToolError> {
    let session_hint = std::env::var("FIL_SESSION_ID")
        .ok()
        .filter(|id| !id.is_empty());
    // Clients that kill tool calls after ~60s get FIL_ASK_WAIT from
    // `fil mcp install`; the question itself stays open for the full timeout.
    let wait_limit = env_secs("FIL_ASK_WAIT");
    match name {
        "notify" => {
            let message = string_arg(arguments, "message").unwrap_or_default();
            let title = string_arg(arguments, "title").unwrap_or_default();
            if message.trim().is_empty() && title.trim().is_empty() {
                return Err(ToolError::Invalid("`message` is required".into()));
            }
            Ok(AgentRequest::Notify {
                title,
                message,
                session_hint,
            })
        }
        "ask" => {
            let question = string_arg(arguments, "question")
                .filter(|q| !q.trim().is_empty())
                .ok_or_else(|| ToolError::Invalid("`question` is required".into()))?;
            let options: Vec<String> = arguments
                .get("options")
                .and_then(Value::as_array)
                .map(|options| {
                    options
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|o| o.trim().to_string())
                        .collect()
                })
                .unwrap_or_default();
            agent::validate_options(&options).map_err(ToolError::Invalid)?;
            let timeout_secs = arguments
                .get("timeout_seconds")
                .and_then(Value::as_u64)
                .or_else(|| env_secs("FIL_ASK_TIMEOUT"))
                .unwrap_or(agent::DEFAULT_ASK_TIMEOUT_SECS)
                .clamp(agent::MIN_ASK_TIMEOUT_SECS, agent::MAX_ASK_TIMEOUT_SECS);
            Ok(AgentRequest::Ask {
                title: string_arg(arguments, "title").unwrap_or_default(),
                question,
                options,
                timeout_secs,
                wait_secs: wait_limit.unwrap_or(timeout_secs).min(timeout_secs),
                session_hint,
            })
        }
        "get_answer" => {
            let question_id = string_arg(arguments, "question_id")
                .filter(|id| !id.is_empty())
                .ok_or_else(|| ToolError::Invalid("`question_id` is required".into()))?;
            Ok(AgentRequest::WaitAnswer {
                question_id,
                wait_secs: wait_limit.unwrap_or(agent::MAX_ASK_TIMEOUT_SECS),
            })
        }
        _ => Err(ToolError::UnknownTool),
    }
}

fn tool_result(reply: anyhow::Result<AgentReply>) -> Value {
    let reply = match reply {
        Ok(reply) => reply,
        Err(error) => return error_result(&format!("{error:#}")),
    };
    match reply {
        AgentReply::Sent {
            session_id,
            delivered,
        } => {
            let mut text = match delivered {
                Some(1) => "Notification sent to the user's phone.".to_string(),
                Some(n) => format!("Notification sent to {n} devices."),
                None => "Notification queued: this Mac is offline from the Fil hub and will send it when it reconnects.".to_string(),
            };
            if session_id.is_none() {
                text.push_str(
                    " (This terminal is not a Fil session, so tapping it opens the Fil app.)",
                );
            }
            json!({
                "content": [{ "type": "text", "text": text }],
                "structuredContent": { "status": "sent", "delivered": delivered, "session_id": session_id },
            })
        }
        AgentReply::Answered {
            question_id,
            answer,
        } => json!({
            "content": [{ "type": "text", "text": format!("The user answered: {answer}") }],
            "structuredContent": { "status": "answered", "answer": answer, "question_id": question_id },
        }),
        AgentReply::Pending {
            question_id,
            expires_in_secs,
        } => json!({
            "content": [{ "type": "text", "text": format!(
                "No answer yet; the question is still open on the user's phone for {expires_in_secs} more seconds. \
                 Call get_answer with question_id \"{question_id}\" to keep waiting, or continue with work that does not depend on the answer."
            ) }],
            "structuredContent": { "status": "pending", "question_id": question_id, "expires_in_seconds": expires_in_secs },
        }),
        AgentReply::Expired { question_id } => json!({
            "content": [{ "type": "text", "text":
                "No answer: the question expired before the user replied. Take the safest option, or stop and explain what you need."
            }],
            "structuredContent": { "status": "expired", "question_id": question_id },
        }),
        AgentReply::Status { .. } => error_result("unexpected status reply"),
        AgentReply::Error { message } => error_result(&message),
    }
}

fn error_result(message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true,
    })
}

fn success_response(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// `fil mcp test`: one real notification, end to end.
pub async fn send_test_notification() -> anyhow::Result<std::process::ExitCode> {
    let reply = DaemonClient::from_env()
        .request(&AgentRequest::Notify {
            title: "Fil notifications work".into(),
            message: "Your coding agents can now reach you here.".into(),
            session_hint: std::env::var("FIL_SESSION_ID").ok(),
        })
        .await;
    match reply {
        Ok(AgentReply::Sent { delivered, .. }) => {
            match delivered {
                Some(n) => println!("  \x1b[32m✓\x1b[0m Sent to {n} device(s)"),
                None => println!("  \x1b[33m~\x1b[0m Queued: the daemon is offline from the hub"),
            }
            Ok(std::process::ExitCode::SUCCESS)
        }
        Ok(AgentReply::Error { message }) => {
            eprintln!("  \x1b[31m✗\x1b[0m {message}");
            Ok(std::process::ExitCode::FAILURE)
        }
        Ok(other) => anyhow::bail!("unexpected reply: {other:?}"),
        Err(error) => {
            eprintln!("  \x1b[31m✗\x1b[0m {error:#}");
            Ok(std::process::ExitCode::FAILURE)
        }
    }
}

pub(crate) async fn daemon_status() -> anyhow::Result<AgentReply> {
    DaemonClient::from_env()
        .request(&AgentRequest::Status {
            session_hint: std::env::var("FIL_SESSION_ID").ok(),
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_negotiates_the_protocol_version() {
        let response = handle_request(
            &json!(1),
            "initialize",
            &json!({ "protocolVersion": "2025-06-18" }),
        );
        assert_eq!(response["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(response["result"]["serverInfo"]["name"], "fil");
        let response = handle_request(
            &json!(1),
            "initialize",
            &json!({ "protocolVersion": "1999-01-01" }),
        );
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
        assert_eq!(
            handle_request(&json!(2), "nope", &Value::Null)["error"]["code"],
            -32601
        );
    }

    #[test]
    fn tools_have_descriptions_and_schemas() {
        let tools = tool_definitions();
        let names: Vec<_> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["notify", "ask", "get_answer"]);
        for tool in tools.as_array().unwrap() {
            assert!(tool["description"].as_str().unwrap().len() > 80);
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn ask_arguments_are_validated_before_reaching_the_daemon() {
        assert!(matches!(
            build_request("ask", &json!({ "question": "?", "options": ["only"] })),
            Err(ToolError::Invalid(_))
        ));
        assert!(matches!(
            build_request("ask", &json!({ "options": ["a", "b"] })),
            Err(ToolError::Invalid(_))
        ));
        assert!(matches!(
            build_request("nope", &json!({})),
            Err(ToolError::UnknownTool)
        ));
        match build_request(
            "ask",
            &json!({ "question": "Deploy?", "options": ["Yes", "No"], "timeout_seconds": 5 }),
        ) {
            Ok(AgentRequest::Ask {
                timeout_secs,
                wait_secs,
                ..
            }) => {
                assert_eq!(timeout_secs, agent::MIN_ASK_TIMEOUT_SECS);
                assert!(wait_secs <= timeout_secs);
            }
            _ => panic!("expected an ask request"),
        }
    }

    #[test]
    fn replies_become_tool_results() {
        let answered = tool_result(Ok(AgentReply::Answered {
            question_id: "q".into(),
            answer: "Yes".into(),
        }));
        assert_eq!(answered["structuredContent"]["answer"], "Yes");
        assert!(answered.get("isError").is_none());
        let pending = tool_result(Ok(AgentReply::Pending {
            question_id: "q".into(),
            expires_in_secs: 30,
        }));
        assert!(
            pending["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("get_answer")
        );
        let failed = tool_result(Err(anyhow::anyhow!("daemon down")));
        assert_eq!(failed["isError"], true);
    }
}
