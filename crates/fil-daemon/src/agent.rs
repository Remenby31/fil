//! Agent notifications: requests from `fil mcp` are forwarded to the hub over
//! the control WebSocket; acknowledgements and answers come back the same way.
//!
//! A question outlives the IPC connection that asked it. Some MCP clients kill
//! a tool call after ~60s, so `ask` may return `Pending` and the agent collects
//! the answer later with `WaitAnswer`, until the question expires.

use crate::process_metadata;
use crate::session_manager::SessionManager;
use fil_protocol::agent::{self, AgentReply, AgentRequest};
use fil_protocol::proto;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tracing::{debug, info, warn};

/// Long enough for the hub to fan out to APNs, short enough that a hub that
/// predates acknowledgements does not stall the agent.
const ACK_WAIT: Duration = Duration::from_secs(5);
const QUEUE_WAIT: Duration = Duration::from_secs(2);

struct Ask {
    options: Vec<String>,
    expires_at: Instant,
    answer: watch::Sender<Option<String>>,
}

#[derive(Default)]
pub struct AgentRelay {
    hub_connected: AtomicBool,
    acks: Mutex<HashMap<String, oneshot::Sender<proto::AgentNotificationAck>>>,
    asks: Mutex<HashMap<String, Ask>>,
}

impl AgentRelay {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn set_hub_connected(&self, connected: bool) {
        self.hub_connected.store(connected, Ordering::Relaxed);
    }

    pub fn on_hub_message(&self, message: proto::HubMessage) {
        match message.payload {
            Some(proto::hub_message::Payload::AgentNotificationAck(ack)) => {
                if let Some(waiter) = self.acks.lock().unwrap().remove(&ack.notification_id) {
                    let _ = waiter.send(ack);
                }
            }
            Some(proto::hub_message::Payload::AgentAnswer(answer)) => {
                let asks = self.asks.lock().unwrap();
                let Some(ask) = asks.get(&answer.notification_id) else {
                    debug!(id = %answer.notification_id, "answer for an unknown or expired question");
                    return;
                };
                if !ask.options.contains(&answer.answer) {
                    warn!(id = %answer.notification_id, "ignoring an answer that is not one of the options");
                    return;
                }
                // First answer wins; a second phone answering later is ignored.
                ask.answer.send_if_modified(|current| {
                    if current.is_some() {
                        return false;
                    }
                    *current = Some(answer.answer.clone());
                    true
                });
                info!(id = %answer.notification_id, "agent question answered");
            }
            _ => {}
        }
    }

    fn expect_ack(&self, id: &str) -> oneshot::Receiver<proto::AgentNotificationAck> {
        let (tx, rx) = oneshot::channel();
        self.acks.lock().unwrap().insert(id.to_string(), tx);
        rx
    }

    async fn wait_answer(&self, question_id: &str, wait: Duration) -> AgentReply {
        let (mut rx, expires_at) = {
            let asks = self.asks.lock().unwrap();
            match asks.get(question_id) {
                Some(ask) => (ask.answer.subscribe(), ask.expires_at),
                None => {
                    return AgentReply::Expired {
                        question_id: question_id.to_string(),
                    };
                }
            }
        };
        let deadline = (Instant::now() + wait).min(expires_at);
        match tokio::time::timeout_at(deadline, rx.wait_for(Option::is_some)).await {
            Ok(Ok(answer)) => AgentReply::Answered {
                question_id: question_id.to_string(),
                answer: answer.clone().unwrap_or_default(),
            },
            // The sender is dropped when the question expires.
            Ok(Err(_)) => AgentReply::Expired {
                question_id: question_id.to_string(),
            },
            Err(_) if Instant::now() >= expires_at => AgentReply::Expired {
                question_id: question_id.to_string(),
            },
            Err(_) => AgentReply::Pending {
                question_id: question_id.to_string(),
                expires_in_secs: expires_at
                    .saturating_duration_since(Instant::now())
                    .as_secs(),
            },
        }
    }
}

pub struct AgentContext {
    pub relay: Arc<AgentRelay>,
    pub sessions: Arc<SessionManager>,
    pub hub: mpsc::Sender<proto::DaemonMessage>,
}

pub async fn handle(
    ctx: &AgentContext,
    request: AgentRequest,
    peer_pid: Option<i32>,
) -> AgentReply {
    match request {
        AgentRequest::Notify {
            title,
            message,
            session_hint,
        } => {
            let session_id = resolve_session(&ctx.sessions, session_hint.as_deref(), peer_pid);
            let notification = proto::AgentNotification {
                notification_id: uuid::Uuid::new_v4().to_string(),
                session_id: session_id.clone().unwrap_or_default(),
                title: agent::clamp_text(&title, agent::MAX_TITLE_CHARS),
                body: agent::clamp_text(&message, agent::MAX_BODY_CHARS),
                options: Vec::new(),
                expires_at: 0,
            };
            if let Err(message) = agent::validate_notification(
                &notification.title,
                &notification.body,
                &notification.options,
            ) {
                return AgentReply::Error { message };
            }
            match deliver(ctx, notification).await {
                Ok(delivered) => AgentReply::Sent {
                    session_id,
                    delivered,
                },
                Err(message) => AgentReply::Error { message },
            }
        }
        AgentRequest::Ask {
            title,
            question,
            options,
            timeout_secs,
            wait_secs,
            session_hint,
        } => {
            let options: Vec<String> = options.iter().map(|o| o.trim().to_string()).collect();
            if let Err(message) = agent::validate_options(&options) {
                return AgentReply::Error { message };
            }
            let timeout_secs =
                timeout_secs.clamp(agent::MIN_ASK_TIMEOUT_SECS, agent::MAX_ASK_TIMEOUT_SECS);
            let session_id = resolve_session(&ctx.sessions, session_hint.as_deref(), peer_pid);
            let id = uuid::Uuid::new_v4().to_string();
            let notification = proto::AgentNotification {
                notification_id: id.clone(),
                session_id: session_id.unwrap_or_default(),
                title: agent::clamp_text(&title, agent::MAX_TITLE_CHARS),
                body: agent::clamp_text(&question, agent::MAX_BODY_CHARS),
                options: options.clone(),
                expires_at: chrono::Utc::now().timestamp() + timeout_secs as i64,
            };
            if let Err(message) = agent::validate_notification(
                &notification.title,
                &notification.body,
                &notification.options,
            ) {
                return AgentReply::Error { message };
            }

            // Register before sending: a fast tap must not race the bookkeeping.
            let expires_at = Instant::now() + Duration::from_secs(timeout_secs);
            ctx.relay.asks.lock().unwrap().insert(
                id.clone(),
                Ask {
                    options,
                    expires_at,
                    answer: watch::channel(None).0,
                },
            );
            if let Err(message) = deliver(ctx, notification).await {
                ctx.relay.asks.lock().unwrap().remove(&id);
                return AgentReply::Error { message };
            }
            spawn_expiry(ctx, id.clone(), expires_at);
            ctx.relay
                .wait_answer(&id, Duration::from_secs(wait_secs))
                .await
        }
        AgentRequest::WaitAnswer {
            question_id,
            wait_secs,
        } => {
            ctx.relay
                .wait_answer(&question_id, Duration::from_secs(wait_secs))
                .await
        }
        AgentRequest::Status { session_hint } => AgentReply::Status {
            hub_connected: ctx.relay.hub_connected.load(Ordering::Relaxed),
            session_id: resolve_session(&ctx.sessions, session_hint.as_deref(), peer_pid),
            sessions: ctx.sessions.all_session_infos().len(),
        },
    }
}

/// Returns how many phones were notified, or `None` when the hub did not say.
async fn deliver(
    ctx: &AgentContext,
    notification: proto::AgentNotification,
) -> Result<Option<u32>, String> {
    let id = notification.notification_id.clone();
    let ack = ctx.relay.expect_ack(&id);
    let message = proto::DaemonMessage {
        payload: Some(proto::daemon_message::Payload::AgentNotification(
            notification,
        )),
    };
    if !matches!(
        tokio::time::timeout(QUEUE_WAIT, ctx.hub.send(message)).await,
        Ok(Ok(()))
    ) {
        ctx.relay.acks.lock().unwrap().remove(&id);
        return Err("fil-daemon could not queue the notification for the hub".into());
    }
    match tokio::time::timeout(ACK_WAIT, ack).await {
        Ok(Ok(ack)) if ack.delivered == 0 => Err(if ack.error.is_empty() {
            "no iPhone is registered for notifications on this account".into()
        } else {
            ack.error
        }),
        Ok(Ok(ack)) => Ok(Some(ack.delivered)),
        _ => {
            ctx.relay.acks.lock().unwrap().remove(&id);
            if !ctx.relay.hub_connected.load(Ordering::Relaxed) {
                // Still queued: the control socket sends it after reconnecting.
                warn!(%id, "hub offline; notification queued");
            }
            Ok(None)
        }
    }
}

fn spawn_expiry(ctx: &AgentContext, id: String, expires_at: Instant) {
    let relay = ctx.relay.clone();
    let hub = ctx.hub.clone();
    tokio::spawn(async move {
        tokio::time::sleep_until(expires_at).await;
        let answered = relay
            .asks
            .lock()
            .unwrap()
            .remove(&id)
            .is_some_and(|ask| ask.answer.borrow().is_some());
        if !answered {
            let closed = proto::DaemonMessage {
                payload: Some(proto::daemon_message::Payload::AgentAskClosed(
                    proto::AgentAskClosed {
                        notification_id: id,
                    },
                )),
            };
            let _ = hub.try_send(closed);
        }
    });
}

/// The process tree is authoritative: `fil mcp` runs under the agent, which
/// runs under the shell that a fil proxy spawned. `FIL_SESSION_ID` covers
/// trees the walk cannot see through (e.g. a tmux server).
fn resolve_session(
    sessions: &SessionManager,
    hint: Option<&str>,
    peer_pid: Option<i32>,
) -> Option<String> {
    if let Some(pid) = peer_pid {
        let chain = process_metadata::ancestor_pids(pid);
        if let Some(session_id) = sessions.session_for_proxy_pids(&chain) {
            return Some(session_id);
        }
    }
    hint.filter(|hint| sessions.contains(hint))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_manager::ProxySession;

    fn context() -> (AgentContext, mpsc::Receiver<proto::DaemonMessage>) {
        let (hub, rx) = mpsc::channel(8);
        let sessions = Arc::new(SessionManager::new());
        sessions.add(ProxySession {
            session_id: "live".into(),
            shell: "zsh".into(),
            command: "claude".into(),
            cwd: "/tmp".into(),
            created_at: 0,
            cols: 80,
            rows: 24,
            proxy_pid: Some(4242),
        });
        (
            AgentContext {
                relay: AgentRelay::new(),
                sessions,
                hub,
            },
            rx,
        )
    }

    fn ack(id: &str, delivered: u32) -> proto::HubMessage {
        proto::HubMessage {
            payload: Some(proto::hub_message::Payload::AgentNotificationAck(
                proto::AgentNotificationAck {
                    notification_id: id.into(),
                    delivered,
                    error: String::new(),
                },
            )),
        }
    }

    fn answer(id: &str, answer: &str) -> proto::HubMessage {
        proto::HubMessage {
            payload: Some(proto::hub_message::Payload::AgentAnswer(
                proto::AgentAnswer {
                    notification_id: id.into(),
                    answer: answer.into(),
                },
            )),
        }
    }

    async fn next_notification(
        rx: &mut mpsc::Receiver<proto::DaemonMessage>,
    ) -> proto::AgentNotification {
        match rx.recv().await.unwrap().payload {
            Some(proto::daemon_message::Payload::AgentNotification(n)) => n,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn session_hint_must_name_a_live_session() {
        let (ctx, _rx) = context();
        assert_eq!(
            resolve_session(&ctx.sessions, Some("live"), None).as_deref(),
            Some("live")
        );
        assert_eq!(resolve_session(&ctx.sessions, Some("stale"), None), None);
        assert_eq!(resolve_session(&ctx.sessions, None, None), None);
    }

    #[tokio::test]
    async fn notify_reports_hub_delivery() {
        let (ctx, mut rx) = context();
        let relay = ctx.relay.clone();
        let hub = tokio::spawn(async move {
            let n = next_notification(&mut rx).await;
            assert_eq!(n.session_id, "live");
            assert!(n.options.is_empty());
            relay.on_hub_message(ack(&n.notification_id, 2));
        });
        let reply = handle(
            &ctx,
            AgentRequest::Notify {
                title: "Done".into(),
                message: "Tests pass".into(),
                session_hint: Some("live".into()),
            },
            None,
        )
        .await;
        hub.await.unwrap();
        assert_eq!(
            reply,
            AgentReply::Sent {
                session_id: Some("live".into()),
                delivered: Some(2)
            }
        );
    }

    #[tokio::test]
    async fn notify_without_registered_phone_is_an_error() {
        let (ctx, mut rx) = context();
        let relay = ctx.relay.clone();
        tokio::spawn(async move {
            let n = next_notification(&mut rx).await;
            relay.on_hub_message(ack(&n.notification_id, 0));
        });
        let reply = handle(
            &ctx,
            AgentRequest::Notify {
                title: "Done".into(),
                message: String::new(),
                session_hint: None,
            },
            None,
        )
        .await;
        assert!(matches!(reply, AgentReply::Error { .. }));
    }

    #[tokio::test]
    async fn ask_returns_the_chosen_option_and_ignores_others() {
        let (ctx, mut rx) = context();
        let relay = ctx.relay.clone();
        tokio::spawn(async move {
            let n = next_notification(&mut rx).await;
            assert_eq!(n.options, vec!["Yes", "No"]);
            assert!(n.expires_at > chrono::Utc::now().timestamp());
            relay.on_hub_message(ack(&n.notification_id, 1));
            relay.on_hub_message(answer(&n.notification_id, "Maybe"));
            relay.on_hub_message(answer(&n.notification_id, "No"));
            relay.on_hub_message(answer(&n.notification_id, "Yes"));
        });
        let reply = handle(
            &ctx,
            AgentRequest::Ask {
                title: "Deploy".into(),
                question: "Ship to prod?".into(),
                options: vec!["Yes".into(), " No ".into()],
                timeout_secs: 600,
                wait_secs: 600,
                session_hint: None,
            },
            None,
        )
        .await;
        assert!(matches!(reply, AgentReply::Answered { ref answer, .. } if answer == "No"));
    }

    #[tokio::test(start_paused = true)]
    async fn unanswered_question_is_pending_then_expires_and_closes_on_the_hub() {
        let (ctx, mut rx) = context();
        let relay = ctx.relay.clone();
        let (id_tx, id_rx) = oneshot::channel();
        let hub = tokio::spawn(async move {
            let n = next_notification(&mut rx).await;
            relay.on_hub_message(ack(&n.notification_id, 1));
            id_tx.send(n.notification_id.clone()).unwrap();
            match rx.recv().await.unwrap().payload {
                Some(proto::daemon_message::Payload::AgentAskClosed(closed)) => {
                    assert_eq!(closed.notification_id, n.notification_id)
                }
                other => panic!("unexpected {other:?}"),
            }
        });
        let reply = handle(
            &ctx,
            AgentRequest::Ask {
                title: String::new(),
                question: "Which DB?".into(),
                options: vec!["Postgres".into(), "SQLite".into()],
                timeout_secs: 60,
                wait_secs: 10,
                session_hint: None,
            },
            None,
        )
        .await;
        let id = id_rx.await.unwrap();
        assert!(matches!(reply, AgentReply::Pending { ref question_id, .. } if *question_id == id));
        let reply = handle(
            &ctx,
            AgentRequest::WaitAnswer {
                question_id: id.clone(),
                wait_secs: 3600,
            },
            None,
        )
        .await;
        assert!(matches!(reply, AgentReply::Expired { .. }));
        hub.await.unwrap();
    }

    #[tokio::test]
    async fn answer_survives_between_wait_calls() {
        let (ctx, mut rx) = context();
        let relay = ctx.relay.clone();
        let (id_tx, id_rx) = oneshot::channel();
        tokio::spawn(async move {
            let n = next_notification(&mut rx).await;
            relay.on_hub_message(ack(&n.notification_id, 1));
            id_tx.send(n.notification_id).unwrap();
        });
        let reply = handle(
            &ctx,
            AgentRequest::Ask {
                title: String::new(),
                question: "Continue?".into(),
                options: vec!["Yes".into(), "No".into()],
                timeout_secs: 600,
                wait_secs: 0,
                session_hint: None,
            },
            None,
        )
        .await;
        assert!(matches!(reply, AgentReply::Pending { .. }));
        let id = id_rx.await.unwrap();
        ctx.relay.on_hub_message(answer(&id, "Yes"));
        let reply = handle(
            &ctx,
            AgentRequest::WaitAnswer {
                question_id: id,
                wait_secs: 0,
            },
            None,
        )
        .await;
        assert!(matches!(reply, AgentReply::Answered { ref answer, .. } if answer == "Yes"));
    }
}
