use sqlx::SqlitePool;
use tracing::{debug, info};
use uuid::Uuid;

/// Find an existing user by provider+provider_id, or by email (cross-provider linking),
/// or create a new one.
pub async fn find_or_create_user(
    pool: &SqlitePool,
    provider: &str,
    provider_id: &str,
    email: Option<&str>,
    display_name: &str,
) -> String {
    // 1. Check by exact provider match
    if let Ok(Some(id)) = sqlx::query_scalar::<_, String>(
        "SELECT id FROM users WHERE provider = ? AND provider_id = ?",
    )
    .bind(provider)
    .bind(provider_id)
    .fetch_optional(pool)
    .await
    {
        debug!(user_id = %id, provider, "existing user found");
        return id;
    }

    // 2. Check user_links table
    if let Ok(Some(id)) = sqlx::query_scalar::<_, String>(
        "SELECT user_id FROM user_links WHERE provider = ? AND provider_id = ?",
    )
    .bind(provider)
    .bind(provider_id)
    .fetch_optional(pool)
    .await
    {
        debug!(user_id = %id, provider, "user found via link");
        return id;
    }

    // 3. Cross-provider email linking: find user with same email from different provider
    if let Some(email) = email {
        if !email.is_empty() {
            if let Ok(Some(id)) = sqlx::query_scalar::<_, String>(
                "SELECT id FROM users WHERE email = ? AND provider != ?",
            )
            .bind(email)
            .bind(provider)
            .fetch_optional(pool)
            .await
            {
                // Link this provider to the existing user
                let link_id = Uuid::new_v4().to_string();
                sqlx::query(
                    "INSERT OR IGNORE INTO user_links (id, user_id, provider, provider_id) VALUES (?, ?, ?, ?)",
                )
                .bind(&link_id)
                .bind(&id)
                .bind(provider)
                .bind(provider_id)
                .execute(pool)
                .await
                .ok();

                info!(user_id = %id, provider, email, "linked account via email match");
                return id;
            }
        }
    }

    // 4. Create new user
    let new_id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO users (id, provider, provider_id, email, display_name) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&new_id)
    .bind(provider)
    .bind(provider_id)
    .bind(email)
    .bind(display_name)
    .execute(pool)
    .await
    .ok();

    info!(user_id = %new_id, provider, "created new user");
    new_id
}
