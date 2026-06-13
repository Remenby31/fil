use axum::extract::{Query, State};
use axum::response::{IntoResponse, Redirect};
use serde::Deserialize;
use tracing::{debug, error};
use uuid::Uuid;

use crate::auth::jwt;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CallbackParams {
    code: String,
    state: String,
}

#[derive(Deserialize)]
struct GitHubTokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct GitHubUser {
    id: i64,
    login: String,
    email: Option<String>,
    name: Option<String>,
}

pub async fn github_auth_start(State(state): State<AppState>) -> impl IntoResponse {
    let oauth_state = Uuid::new_v4().to_string();

    // Store the state for CSRF protection
    let _ = sqlx::query("INSERT INTO oauth_states (state, provider) VALUES (?, 'github')")
        .bind(&oauth_state)
        .execute(&state.db.pool)
        .await;

    let url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}/auth/github/callback&state={}&scope=user:email",
        state.config.github_client_id,
        state.config.public_url,
        oauth_state,
    );

    Redirect::temporary(&url)
}

pub async fn github_auth_callback(
    State(state): State<AppState>,
    Query(params): Query<CallbackParams>,
) -> impl IntoResponse {
    // Verify CSRF state
    let state_exists = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM oauth_states WHERE state = ? AND provider = 'github'"
    )
        .bind(&params.state)
        .fetch_one(&state.db.pool)
        .await
        .unwrap_or(0);

    if state_exists == 0 {
        return (axum::http::StatusCode::BAD_REQUEST, "Invalid OAuth state").into_response();
    }

    // Clean up used state
    let _ = sqlx::query("DELETE FROM oauth_states WHERE state = ?")
        .bind(&params.state)
        .execute(&state.db.pool)
        .await;

    // Exchange code for access token
    let client = reqwest::Client::new();
    let token_res = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&[
            ("client_id", state.config.github_client_id.as_str()),
            ("client_secret", state.config.github_client_secret.as_str()),
            ("code", &params.code),
        ])
        .send()
        .await;

    let token_data = match token_res {
        Ok(resp) => match resp.json::<GitHubTokenResponse>().await {
            Ok(data) => data,
            Err(e) => {
                error!(error = %e, "failed to parse GitHub token response");
                return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "GitHub auth failed").into_response();
            }
        },
        Err(e) => {
            error!(error = %e, "failed to exchange GitHub code");
            return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "GitHub auth failed").into_response();
        }
    };

    // Get user info
    let user_res = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", token_data.access_token))
        .header("User-Agent", "fil-hub")
        .send()
        .await;

    let github_user = match user_res {
        Ok(resp) => match resp.json::<GitHubUser>().await {
            Ok(user) => user,
            Err(e) => {
                error!(error = %e, "failed to parse GitHub user");
                return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "GitHub auth failed").into_response();
            }
        },
        Err(e) => {
            error!(error = %e, "failed to fetch GitHub user");
            return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "GitHub auth failed").into_response();
        }
    };

    debug!(github_id = github_user.id, login = %github_user.login, "GitHub user authenticated");

    // Find or create user
    let provider_id = github_user.id.to_string();
    let existing_user = sqlx::query_scalar::<_, String>(
        "SELECT id FROM users WHERE provider = 'github' AND provider_id = ?"
    )
        .bind(&provider_id)
        .fetch_optional(&state.db.pool)
        .await
        .unwrap_or(None);

    let user_id = match existing_user {
        Some(id) => id,
        None => {
            let new_id = Uuid::new_v4().to_string();
            let display_name = github_user.name.unwrap_or(github_user.login);
            let _ = sqlx::query(
                "INSERT INTO users (id, provider, provider_id, email, display_name) VALUES (?, 'github', ?, ?, ?)"
            )
                .bind(&new_id)
                .bind(&provider_id)
                .bind(&github_user.email)
                .bind(&display_name)
                .execute(&state.db.pool)
                .await;
            debug!(user_id = %new_id, "created new user");
            new_id
        }
    };

    // Generate JWT
    let token = jwt::create_token(&user_id, &state.config.jwt_secret).unwrap();

    // Return the token as JSON (the CLI will capture this)
    axum::Json(serde_json::json!({
        "token": token,
        "user_id": user_id,
    })).into_response()
}
