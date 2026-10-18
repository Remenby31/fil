//! Agent notifications: the request/reply shapes `fil mcp` exchanges with the
//! daemon over the IPC socket, and the content limits every hop enforces.

use serde::{Deserialize, Serialize};

pub const MAX_TITLE_CHARS: usize = 120;
pub const MAX_BODY_CHARS: usize = 1000;
pub const MIN_OPTIONS: usize = 2;
pub const MAX_OPTIONS: usize = 4;
/// iOS truncates notification action titles well before this.
pub const MAX_OPTION_CHARS: usize = 40;
pub const DEFAULT_ASK_TIMEOUT_SECS: u64 = 600;
pub const MAX_ASK_TIMEOUT_SECS: u64 = 3600;
pub const MIN_ASK_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentRequest {
    Notify {
        title: String,
        message: String,
        /// `FIL_SESSION_ID` inherited from the Fil terminal, if any.
        #[serde(default)]
        session_hint: Option<String>,
    },
    Ask {
        title: String,
        question: String,
        options: Vec<String>,
        /// How long the question stays answerable.
        timeout_secs: u64,
        /// How long this call blocks before returning `Pending`.
        wait_secs: u64,
        #[serde(default)]
        session_hint: Option<String>,
    },
    /// Wait for the answer to a question asked earlier.
    WaitAnswer { question_id: String, wait_secs: u64 },
    Status {
        #[serde(default)]
        session_hint: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentReply {
    Sent {
        session_id: Option<String>,
        /// `None` when the hub predates delivery acknowledgements.
        delivered: Option<u32>,
    },
    Answered {
        question_id: String,
        answer: String,
    },
    Pending {
        question_id: String,
        expires_in_secs: u64,
    },
    Expired {
        question_id: String,
    },
    Status {
        hub_connected: bool,
        session_id: Option<String>,
        sessions: usize,
    },
    Error {
        message: String,
    },
}

/// Trim and cap free text. Agents write long messages; a truncated
/// notification is more useful than a rejected one.
pub fn clamp_text(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut clamped: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    clamped.push('…');
    clamped
}

/// Options become notification buttons, so they are validated strictly
/// rather than silently rewritten into something the agent did not offer.
pub fn validate_options(options: &[String]) -> Result<(), String> {
    if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&options.len()) {
        return Err(format!(
            "options must contain between {MIN_OPTIONS} and {MAX_OPTIONS} choices"
        ));
    }
    for (index, option) in options.iter().enumerate() {
        let trimmed = option.trim();
        if trimmed.is_empty() {
            return Err("options must not be empty".into());
        }
        if trimmed.chars().count() > MAX_OPTION_CHARS {
            return Err(format!(
                "option \"{trimmed}\" is longer than {MAX_OPTION_CHARS} characters; use a short label and put details in the question"
            ));
        }
        if options[..index].iter().any(|other| other.trim() == trimmed) {
            return Err(format!("option \"{trimmed}\" is duplicated"));
        }
    }
    Ok(())
}

pub fn validate_notification(title: &str, body: &str, options: &[String]) -> Result<(), String> {
    if title.trim().is_empty() && body.trim().is_empty() {
        return Err("a notification needs a title or a message".into());
    }
    if title.chars().count() > MAX_TITLE_CHARS || body.chars().count() > MAX_BODY_CHARS {
        return Err("notification text is too long".into());
    }
    if !options.is_empty() {
        validate_options(options)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_as_tagged_json() {
        let request = AgentRequest::Ask {
            title: "Deploy".into(),
            question: "Ship it?".into(),
            options: vec!["Yes".into(), "No".into()],
            timeout_secs: 600,
            wait_secs: 50,
            session_hint: Some("abc".into()),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains(r#""kind":"ask""#));
        assert_eq!(
            serde_json::from_str::<AgentRequest>(&json).unwrap(),
            request
        );
        let legacy: AgentRequest =
            serde_json::from_str(r#"{"kind":"notify","title":"t","message":"m"}"#).unwrap();
        assert!(matches!(
            legacy,
            AgentRequest::Notify {
                session_hint: None,
                ..
            }
        ));
    }

    #[test]
    fn text_is_clamped_on_character_boundaries() {
        assert_eq!(clamp_text("  hi  ", 10), "hi");
        assert_eq!(clamp_text("ééééé", 3), "éé…");
    }

    #[test]
    fn options_are_validated() {
        let ok = vec!["Yes".to_string(), "No".to_string()];
        assert!(validate_options(&ok).is_ok());
        assert!(validate_options(&ok[..1]).is_err());
        assert!(validate_options(&["a".into(), "a ".into()]).is_err());
        assert!(validate_options(&["a".into(), " ".into()]).is_err());
        assert!(validate_options(&["a".into(), "x".repeat(41)]).is_err());
        assert!(validate_options(&vec!["o".to_string(); 5]).is_err());
        assert!(validate_notification("", "", &[]).is_err());
        assert!(validate_notification("t", "", &[]).is_ok());
    }
}
