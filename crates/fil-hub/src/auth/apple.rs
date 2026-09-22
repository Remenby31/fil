use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use base64::Engine;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use tracing::{debug, error};

use crate::auth::jwt;
use crate::auth::shared::{provider_http_client, provider_json};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct AppleAuthRequest {
    identity_token: String,
    full_name: Option<String>,
}

#[derive(Deserialize)]
struct AppleTokenClaims {
    sub: String,
    email: Option<String>,
    #[serde(default)]
    email_verified: serde_json::Value,
}

pub async fn apple_auth_callback(
    State(state): State<AppState>,
    Json(req): Json<AppleAuthRequest>,
) -> impl IntoResponse {
    let client = match provider_http_client() {
        Ok(client) => client,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    apple_auth_with_provider(&state, req, client, "https://appleid.apple.com/auth/keys").await
}

async fn apple_auth_with_provider(
    state: &AppState,
    req: AppleAuthRequest,
    client: &reqwest::Client,
    keys_url: &str,
) -> axum::response::Response {
    if state.config.apple_client_id.trim().is_empty() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    // Reject malformed or unsupported tokens before requesting Apple's keys.
    if apple_token_header(&req.identity_token).is_err() {
        return (StatusCode::UNAUTHORIZED, "Invalid Apple identity token").into_response();
    }
    let keys = match provider_json::<JwkSet>(client.get(keys_url)).await {
        Ok(keys) => keys,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Apple verification unavailable",
            )
                .into_response();
        }
    };
    let claims = match decode_apple_token(
        &req.identity_token,
        (!state.config.apple_client_id.is_empty()).then_some(state.config.apple_client_id.as_str()),
        &keys,
    ) {
        Ok(claims) => claims,
        Err(e) => {
            error!(error = %e, "failed to decode Apple identity token");
            return (StatusCode::UNAUTHORIZED, "Invalid Apple identity token").into_response();
        }
    };

    let apple_user_id = claims.sub;
    // Only Apple's signed, verified email may populate the profile. The request body
    // is controlled by the client and is not an identity proof.
    let email = if claims.email_verified == true || claims.email_verified == "true" {
        claims.email
    } else {
        None
    };
    let display_name = req.full_name.unwrap_or_else(|| "Apple User".to_string());

    debug!(apple_user_id = %apple_user_id, "Apple user authenticated");

    // Resolve the provider identity without linking unrelated accounts by email.
    let user_id = match crate::auth::shared::find_or_create_user(
        &state.db.pool,
        "apple",
        &apple_user_id,
        email.as_deref(),
        &display_name,
    )
    .await
    {
        Ok(user_id) => user_id,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Auth failed").into_response(),
    };

    // Generate JWT
    let token = match jwt::create_token(&user_id, &state.config.jwt_secret) {
        Ok(t) => t,
        Err(e) => {
            error!(error = %e, "failed to create JWT");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Auth failed").into_response();
        }
    };

    Json(serde_json::json!({
        "token": token,
        "user_id": user_id,
    }))
    .into_response()
}

fn apple_token_header(token: &str) -> Result<jsonwebtoken::Header, &'static str> {
    if token.len() > 16 * 1024 {
        return Err("identity token too large");
    }
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
        return Err("invalid JWT structure");
    }
    let header = decode_header(token).map_err(|_| "invalid JWT header")?;
    if header.alg != Algorithm::RS256 {
        return Err("invalid Apple signing algorithm");
    }
    if !header
        .kid
        .as_deref()
        .is_some_and(|kid| !kid.trim().is_empty() && kid.len() <= 128)
    {
        return Err("invalid signing key ID");
    }
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| "invalid JWT payload")?;
    let _: AppleTokenClaims =
        serde_json::from_slice(&payload).map_err(|_| "invalid JWT payload")?;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[2])
        .map_err(|_| "invalid JWT signature encoding")?;
    Ok(header)
}

fn decode_apple_token(
    token: &str,
    expected_audience: Option<&str>,
    keys: &JwkSet,
) -> Result<AppleTokenClaims, String> {
    let audience = expected_audience
        .filter(|s| !s.trim().is_empty())
        .ok_or("Apple audience not configured")?;
    let header = apple_token_header(token)?;
    let kid = header.kid.as_deref().ok_or("missing signing key ID")?;
    let key = keys.find(kid).ok_or("unknown Apple signing key")?;
    let key = DecodingKey::from_jwk(key).map_err(|_| "invalid Apple key")?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.leeway = 0;
    validation.validate_nbf = true;
    validation.set_audience(&[audience]);
    validation.set_issuer(&["https://appleid.apple.com"]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    let claims = decode::<AppleTokenClaims>(token, &key, &validation)
        .map_err(|_| "invalid Apple identity token")?
        .claims;
    if claims.sub.trim().is_empty() {
        return Err("empty subject".into());
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::shared::tests::mock_provider;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::{Value, json};

    // TEST ONLY: disposable 2048-bit RSA fixture generated with
    // `openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048`.
    // This key is public test data, never an Apple or application credential.
    // Keeping it under cfg(test) avoids OpenSSL/runtime or new dependency needs.
    const TEST_ONLY_RSA_PRIVATE_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQCfMCUYTUGwx91N
A/NYYKuw40xmooYSuZUe6dh1J3gfAllin9DF6Tlql08PWzjhu8fZ94LNmr5ZjJW6
dzUTHG4QjcT0jdWfoLcGF4EJUs6OVNAUjcVaDEZaoMI0x0RRFt6JfkcMuhG1Dqar
qms1rLxgCqnOKmZY9VcClxNJMUA86tH+i1cOm1eAUNz8ZSgQwZzLObSH6pXEQSn0
7a/5Fwwu3FEhLA28RGickSPuOBWVg9imadZkBCLC22kqkg1Q1ipSebCGFIF/Y5TY
WNk1CgeLHysJCj743xfxgU7enwyOWT4uHgYnufk8Hb/A7GwONKEA+vwyzhBQ8oEo
JhDkm1+lAgMBAAECggEAFoeuYFui1f3blOhfuxznf+ATsp0DA8hvUhI/uAkPh7gB
5aL/3drt7OegYWiESvKUTQWqZiYmFa9/i4YOpXdLHp2qCADBANgyvzJ5aVPmdx+K
2bXc/ispk+8XvXeMdDub4HLWZHc7RVhe/4HdrZmWibNIqCy5EpMarvup15yYXmaW
h+EhnJEWpMlcEtcty4+aN7hc+/F+YVxr+l/qNRqimUKjESldcApSpxgwy9ACg3sd
LK/kBl/NAc5UqzVal+YWsTDDrHaaVylqfNGx4gZ3GRQNyNN8Bl5Pfv49Ce9Qyi94
K0A8OHelWXzheDQPKfGOSZS3ooh+K+RK+fVZGqTDYQKBgQDQ44MsUoOMc2AmbovK
vAAtf8pjRGkvXQTX0ulbZKhP04tIBi/xdsrttVW/EhlJ1jpBUMtmqtiuhk8JImvM
8vTP0lSMUZP+8IPe1c9PtqGb/wMdk0Wa/EuB4MNWiFU0vC6djSxNKZ1BLFHPenFP
zskcO3EcgW1EMDu2Fjw3EXXukQKBgQDDFxgwUXTW4WNWixwYvU5BA9piaItzfU6V
GFihjfb9RhmJnVp09xyy4KPY/7OXD1KgL4QPJSFeows8wiuHRuKCm969FjQsyPpM
jybyHiULzZlmYBvcAQnTmqaXoxolDvKMkk4niYV6cSJQCyegEDRv8agIwRCg84/l
L6EYvQBR1QKBgBRAmq0nat2pKf9P5HnJdHL02th4/4G6EQgjyMA1qCPlLLHU97z/
eXlGhYO663y/KnK+tJnForB5ERyfm7gJLjcf+1aHakPjacWnESx3Vn/bX5/0cWEv
aNq0wfuXyDsOq65Wy57HlBmHhH8LLgVA1TrJgJP08HUWABQNX9Uu+jIhAoGAc5wa
/H85AyHb0WxsgQil+AdFgi27/fuS9u3PkCVl6Z+CALgb49aQzjwrPKwDDBDLgRvH
YYY6aS+ruBzE2Myb7JRcAafH0YZkNbxbcv2ELKNxNWbc+5ot7ZTnBlNkafOars0A
vZNUY0Pp9o81szgHKxOE5XMr3IWZj1KTX+qY5uUCgYBB0MpTdxEM/HIdTZu9eSth
MkjopYfev9o/79QR87zYDRi5Hi9CosHgn/sOJf5DuSj+VYBNUdrEHDoRa4uUxd+e
iP4aP+kGjUHI2vakAHPo6892mLq6l9/nzmU2gpEyh9XhUS2vKj4JrWQV78q5iU2D
++7P3w7E3gF0ccz3Xi666A==
-----END PRIVATE KEY-----"#;

    fn test_keys() -> JwkSet {
        let der = base64::engine::general_purpose::STANDARD
            .decode(
                TEST_ONLY_RSA_PRIVATE_KEY
                    .lines()
                    .filter(|line| !line.starts_with("-----"))
                    .collect::<String>(),
            )
            .unwrap();
        let pair = ring::rsa::KeyPair::from_pkcs8(&der).unwrap();
        let components = ring::rsa::PublicKeyComponents::<Vec<u8>>::from(pair.public());
        serde_json::from_value(json!({"keys": [{
            "kty": "RSA", "kid": "test-only-apple-key", "alg": "RS256", "use": "sig",
            "n": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(components.n),
            "e": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(components.e),
        }]}))
        .unwrap()
    }

    fn test_claims() -> Value {
        json!({
            "sub": "test-only-apple-subject", "iss": "https://appleid.apple.com",
            "aud": "sh.fil.app", "exp": chrono::Utc::now().timestamp() + 3600,
            "email": "signed@example.test", "email_verified": true,
        })
    }

    fn signed_token(claims: &Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-only-apple-key".into());
        encode(
            &header,
            claims,
            &EncodingKey::from_rsa_pem(TEST_ONLY_RSA_PRIVATE_KEY.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    async fn keys_provider() -> crate::auth::shared::tests::TestProvider {
        mock_provider(
            axum::Router::new().route("/", axum::routing::get(|| async { Json(test_keys()) })),
        )
        .await
    }

    #[test]
    fn valid_rsa_signature_and_required_claims_are_accepted() {
        let claims = decode_apple_token(
            &signed_token(&test_claims()),
            Some("sh.fil.app"),
            &test_keys(),
        )
        .unwrap();
        assert_eq!(claims.sub, "test-only-apple-subject");
        assert_eq!(claims.email.as_deref(), Some("signed@example.test"));
        let mut multiple_audiences = test_claims();
        multiple_audiences["aud"] = json!(["other-client", "sh.fil.app"]);
        assert!(
            decode_apple_token(
                &signed_token(&multiple_audiences),
                Some("sh.fil.app"),
                &test_keys()
            )
            .is_ok()
        );
    }

    #[test]
    fn invalid_audience_issuer_expiry_subject_and_missing_claims_are_rejected() {
        for (field, value) in [
            ("aud", json!("attacker-client")),
            ("aud", json!(["other-client"])),
            ("iss", json!("https://attacker.invalid")),
            ("exp", json!(chrono::Utc::now().timestamp() - 1)),
            ("nbf", json!(chrono::Utc::now().timestamp() + 3600)),
            ("sub", json!("")),
            ("sub", json!("  ")),
        ] {
            let mut claims = test_claims();
            claims[field] = value;
            assert!(
                decode_apple_token(&signed_token(&claims), Some("sh.fil.app"), &test_keys())
                    .is_err(),
                "accepted invalid {field}"
            );
        }
        for field in ["aud", "iss", "exp", "sub"] {
            let mut claims = test_claims();
            claims.as_object_mut().unwrap().remove(field);
            assert!(
                decode_apple_token(&signed_token(&claims), Some("sh.fil.app"), &test_keys())
                    .is_err(),
                "accepted missing {field}"
            );
        }
        for audience in [None, Some(""), Some("  ")] {
            assert!(
                decode_apple_token(&signed_token(&test_claims()), audience, &test_keys()).is_err()
            );
        }
    }

    #[test]
    fn tampered_signature_payload_wrong_key_and_hmac_confusion_are_rejected() {
        let valid = signed_token(&test_claims());
        let parts: Vec<_> = valid.split('.').collect();
        let mut signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[2])
            .unwrap();
        signature[0] ^= 1;
        let bad_signature = format!(
            "{}.{}.{}",
            parts[0],
            parts[1],
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
        );
        assert!(decode_apple_token(&bad_signature, Some("sh.fil.app"), &test_keys()).is_err());
        let mut forged_claims = test_claims();
        forged_claims["sub"] = json!("someone-else");
        let forged_payload = format!(
            "{}.{}.{}",
            parts[0],
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&forged_claims).unwrap()),
            parts[2]
        );
        assert!(decode_apple_token(&forged_payload, Some("sh.fil.app"), &test_keys()).is_err());

        let mut wrong_keys = serde_json::to_value(test_keys()).unwrap();
        let mut modulus = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(wrong_keys["keys"][0]["n"].as_str().unwrap())
            .unwrap();
        modulus[50] ^= 1;
        wrong_keys["keys"][0]["n"] =
            json!(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(modulus));
        assert!(
            decode_apple_token(
                &valid,
                Some("sh.fil.app"),
                &serde_json::from_value(wrong_keys).unwrap()
            )
            .is_err()
        );
        assert!(decode_apple_token(&valid, Some("sh.fil.app"), &JwkSet { keys: vec![] }).is_err());

        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("test-only-apple-key".into());
        let hmac = encode(
            &header,
            &test_claims(),
            &EncodingKey::from_secret(b"test-only-hmac-key"),
        )
        .unwrap();
        assert!(decode_apple_token(&hmac, Some("sh.fil.app"), &test_keys()).is_err());
    }

    #[tokio::test]
    async fn malformed_input_and_missing_configuration_do_not_fetch_keys() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        let provider = mock_provider(axum::Router::new().route(
            "/",
            axum::routing::get(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                async { StatusCode::INTERNAL_SERVER_ERROR }
            }),
        ))
        .await;
        let mut state = crate::state::test_state().await;
        for identity_token in [
            "".to_string(),
            "not-a-jwt".into(),
            "e30.e30.".into(),
            "x".repeat(16 * 1024 + 1),
        ] {
            let response = apple_auth_with_provider(
                &state,
                AppleAuthRequest {
                    identity_token,
                    full_name: None,
                },
                &provider.client,
                &provider.url,
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        state.config.apple_client_id.clear();
        let response = apple_auth_with_provider(
            &state,
            AppleAuthRequest {
                identity_token: signed_token(&test_claims()),
                full_name: None,
            },
            &provider.client,
            &provider.url,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn only_signed_verified_email_is_stored_and_token_names_a_real_user() {
        let provider = keys_provider().await;
        let state = crate::state::test_state().await;
        for (index, verified) in [
            json!(true),
            json!("true"),
            json!(false),
            json!("false"),
            json!(null),
            json!(1),
        ]
        .into_iter()
        .enumerate()
        {
            let mut claims = test_claims();
            claims["sub"] = json!(format!("test-apple-{index}"));
            claims["email_verified"] = verified;
            let req: AppleAuthRequest = serde_json::from_value(json!({
                "identity_token": signed_token(&claims), "email": "untrusted-request@example.test",
            }))
            .unwrap();
            let response =
                apple_auth_with_provider(&state, req, &provider.client, &provider.url).await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
                .await
                .unwrap();
            let auth: Value = serde_json::from_slice(&body).unwrap();
            let authenticated =
                crate::auth::authenticate_token(auth["token"].as_str().unwrap(), &state)
                    .await
                    .unwrap();
            assert_eq!(authenticated.user_id, auth["user_id"].as_str().unwrap());
            let email: Option<String> = sqlx::query_scalar("SELECT email FROM users WHERE id = ?")
                .bind(&authenticated.user_id)
                .fetch_one(&state.db.pool)
                .await
                .unwrap();
            assert_eq!(
                email.as_deref(),
                (index < 2).then_some("signed@example.test")
            );
        }
    }

    #[tokio::test]
    async fn apple_callback_rejects_db_failure_without_issuing_a_token() {
        let provider = keys_provider().await;
        let state = crate::state::test_state().await;
        sqlx::query("CREATE TRIGGER fail_user_insert BEFORE INSERT ON users BEGIN SELECT RAISE(ABORT, 'test-only failure'); END")
            .execute(&state.db.pool).await.unwrap();
        let response = apple_auth_with_provider(
            &state,
            AppleAuthRequest {
                identity_token: signed_token(&test_claims()),
                full_name: None,
            },
            &provider.client,
            &provider.url,
        )
        .await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), b"Auth failed");
    }

    #[tokio::test]
    async fn keys_failure_and_invalid_signed_claims_never_issue_tokens() {
        let state = crate::state::test_state().await;
        let unavailable = mock_provider(axum::Router::new().route(
            "/",
            axum::routing::get(|| async { (StatusCode::BAD_GATEWAY, "test-only-upstream-detail") }),
        ))
        .await;
        let response = apple_auth_with_provider(
            &state,
            AppleAuthRequest {
                identity_token: signed_token(&test_claims()),
                full_name: None,
            },
            &unavailable.client,
            &unavailable.url,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), b"Apple verification unavailable");

        let provider = keys_provider().await;
        let mut claims = test_claims();
        claims["aud"] = json!("other-app");
        let response = apple_auth_with_provider(
            &state,
            AppleAuthRequest {
                identity_token: signed_token(&claims),
                full_name: None,
            },
            &provider.client,
            &provider.url,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&state.db.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn unsigned_identity_cannot_authenticate() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            br#"{"sub":"audit-user","iss":"https://appleid.apple.com","aud":"sh.fil.app","exp":4102444800}"#,
        );
        let token = format!("eyJhbGciOiJub25lIn0.{payload}.invalid");
        assert!(decode_apple_token(&token, Some("sh.fil.app"), &JwkSet { keys: vec![] }).is_err());
    }
}
