use anyhow::Result;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,   // user_id
    pub exp: usize,    // expiration timestamp
    pub iat: usize,    // issued at
}

pub fn create_token(user_id: &str, secret: &str) -> Result<String> {
    let now = chrono::Utc::now().timestamp() as usize;
    let claims = Claims {
        sub: user_id.to_string(),
        exp: now + 30 * 24 * 60 * 60, // 30 days
        iat: now,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;

    Ok(token)
}

pub fn verify_token(token: &str, secret: &str) -> Result<Claims> {
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;

    Ok(token_data.claims)
}
