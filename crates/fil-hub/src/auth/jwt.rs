use anyhow::Result;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String, // user_id
    pub exp: usize,  // expiration timestamp
    pub iat: usize,  // issued at
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

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &str = "test-only-hub-signing-secret";

    #[test]
    fn hub_token_round_trip_preserves_subject_and_lifetime() {
        let token = create_token("test-user", TEST_SECRET).unwrap();
        let claims = verify_token(&token, TEST_SECRET).unwrap();
        assert_eq!(claims.sub, "test-user");
        assert_eq!(claims.exp - claims.iat, 30 * 24 * 60 * 60);
    }

    #[test]
    fn expired_wrong_secret_and_malformed_tokens_are_rejected() {
        let token = create_token("test-user", TEST_SECRET).unwrap();
        assert!(verify_token(&token, "different-test-secret").is_err());
        let now = chrono::Utc::now().timestamp() as usize;
        let claims = Claims {
            sub: "test-user".into(),
            iat: now - 3600,
            exp: now - 120,
        };
        let expired = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(TEST_SECRET.as_bytes()),
        )
        .unwrap();
        for token in ["", "not-a-token", &expired] {
            assert!(verify_token(token, TEST_SECRET).is_err());
        }
    }

    #[test]
    fn non_hs256_algorithm_and_missing_claims_are_rejected() {
        let claims = serde_json::json!({"sub":"test-user", "iat":chrono::Utc::now().timestamp(), "exp":chrono::Utc::now().timestamp() + 3600});
        let other_algorithm = encode(
            &Header::new(jsonwebtoken::Algorithm::HS384),
            &claims,
            &EncodingKey::from_secret(TEST_SECRET.as_bytes()),
        )
        .unwrap();
        assert!(verify_token(&other_algorithm, TEST_SECRET).is_err());
        for field in ["exp", "sub", "iat"] {
            let mut incomplete = claims.clone();
            incomplete.as_object_mut().unwrap().remove(field);
            let token = encode(
                &Header::default(),
                &incomplete,
                &EncodingKey::from_secret(TEST_SECRET.as_bytes()),
            )
            .unwrap();
            assert!(
                verify_token(&token, TEST_SECRET).is_err(),
                "accepted token missing {field}"
            );
        }
    }
}
