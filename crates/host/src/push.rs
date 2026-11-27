use std::{path::Path, sync::Arc, time::Duration};

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Result, store::{Pending, Store, now}};

const OAUTH_URL: &str = "https://oauth2.googleapis.com/token";
const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";

pub struct Fcm {
    client: Client,
    credentials: Credentials,
    key: EncodingKey,
    endpoint: Url,
    token: Option<(String, i64)>,
}

#[derive(Deserialize)]
struct Credentials {
    project_id: String,
    client_email: String,
    private_key: String,
}

#[derive(Serialize)]
struct Claims<'a> {
    iss: &'a str,
    aud: &'static str,
    scope: &'static str,
    iat: i64,
    exp: i64,
}

#[derive(Deserialize)]
struct AccessToken {
    access_token: String,
    expires_in: i64,
}

enum Delivery { Accepted, Unregistered, Retry(i64), Rejected }

impl Fcm {
    pub fn load(path: &Path) -> Result<Self> {
        let credentials: Credentials = serde_json::from_reader(std::fs::File::open(path)?)?;
        if credentials.project_id.is_empty() ||
            !credentials.project_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
            return Err("invalid Firebase project id".into());
        }
        let key = EncodingKey::from_rsa_pem(credentials.private_key.as_bytes())?;
        let endpoint = Url::parse(&format!("https://fcm.googleapis.com/v1/projects/{}/messages:send", credentials.project_id))?;
        let client = Client::builder().timeout(Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none()).build()?;
        Ok(Self { client, credentials, key, endpoint, token: None })
    }

    async fn access_token(&mut self) -> Result<&str> {
        let timestamp = now();
        if self.token.as_ref().is_none_or(|(_, expiry)| *expiry <= timestamp + 30) {
            let claims = Claims { iss: &self.credentials.client_email, aud: OAUTH_URL, scope: FCM_SCOPE, iat: timestamp, exp: timestamp + 3600 };
            let jwt = encode(&Header::new(Algorithm::RS256), &claims, &self.key)?;
            let result: AccessToken = self.client.post(OAUTH_URL)
                .form(&[("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"), ("assertion", jwt.as_str())])
                .send().await?.error_for_status()?.json().await?;
            self.token = Some((result.access_token, timestamp + result.expires_in));
        }
        Ok(&self.token.as_ref().unwrap().0)
    }

    async fn deliver(&mut self, item: &Pending) -> Result<Delivery> {
        let token = self.access_token().await?.to_owned();
        let ttl = item.expires - now();
        if ttl <= 0 {
            return Ok(Delivery::Rejected);
        }
        let response = self.client.post(self.endpoint.clone()).bearer_auth(token)
            .json(&json!({
                "message": {
                    "token": item.token,
                    "notification": {"title": "Surveillance alert", "body": "Open the app to see the event."},
                    "data": {"event_id": item.event_id},
                    "android": {
                        "ttl": format!("{ttl}s"),
                        "priority": "HIGH",
                        "notification": {
                            "channel_id": "surveillance_alerts",
                            "tag": item.event_id,
                            "visibility": "PRIVATE"
                        }
                    }
                }
            })).send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(Delivery::Accepted);
        }
        if status == StatusCode::UNAUTHORIZED {
            self.token = None;
            return Ok(Delivery::Retry(5));
        }
        if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
            let minimum = if status == StatusCode::TOO_MANY_REQUESTS { 60 } else { 2 };
            let delay = response.headers().get(reqwest::header::RETRY_AFTER)
                .and_then(|header| header.to_str().ok()).and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(minimum).clamp(minimum, 120);
            return Ok(Delivery::Retry(delay));
        }
        if status == StatusCode::NOT_FOUND {
            let body: Value = response.json().await?;
            if body["error"]["details"].as_array().is_some_and(|items| items.iter().any(|item| item["errorCode"] == "UNREGISTERED")) {
                return Ok(Delivery::Unregistered);
            }
        }
        tracing::warn!(%status, "Firebase rejected an alert");
        Ok(Delivery::Rejected)
    }
}

pub async fn work(store: Arc<Store>, mut fcm: Option<Fcm>, retention_days: u64) {
    let mut prune_at = 0;
    loop {
        if now() >= prune_at {
            if let Err(error) = store.prune(retention_days) {
                tracing::warn!(%error, "Could not prune event history");
            }
            prune_at = now() + 3600;
        }
        if let Some(fcm) = &mut fcm {
            match store.pending() {
                Ok(Some(item)) => {
                    if let Err(error) = deliver_one(&store, fcm, item).await {
                        tracing::warn!(%error, "Could not update alert queue");
                    } else {
                        continue;
                    }
                }
                Ok(None) => (),
                Err(error) => tracing::warn!(%error, "Could not read alert queue"),
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn deliver_one(store: &Store, fcm: &mut Fcm, item: Pending) -> Result<()> {
    let backoff = 2_i64.pow((item.attempts + 1).clamp(1, 5) as u32);
    match fcm.deliver(&item).await {
        Ok(Delivery::Accepted | Delivery::Rejected) => store.sent(&item.event_id),
        Ok(Delivery::Unregistered) => store.invalid_token(&item),
        Ok(Delivery::Retry(delay)) => store.retry(&item, delay.max(backoff)),
        Err(error) => {
            tracing::warn!(%error, "Alert delivery failed. Retrying.");
            store.retry(&item, backoff)
        }
    }
}
