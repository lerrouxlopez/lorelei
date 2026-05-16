#![forbid(unsafe_code)]

use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::Serialize;

#[derive(Debug)]
pub struct HarborClient {
    base_url: String,
    http: reqwest::Client,
}

#[derive(Debug)]
pub struct HarborHttpError {
    pub method: &'static str,
    pub url: String,
    pub status: Option<StatusCode>,
    pub body: Option<String>,
}

impl std::fmt::Display for HarborHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(code) => {
                write!(f, "http {} {}: {}", code.as_u16(), self.method, self.url)?;
            }
            None => write!(f, "http error {} {}", self.method, self.url)?,
        }
        if let Some(b) = &self.body {
            if !b.trim().is_empty() {
                write!(f, ": {b}")?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for HarborHttpError {}

impl HarborClient {
    pub fn new(base_url: String) -> Result<Self, String> {
        let base_url = base_url.trim_end_matches('/').to_string();
        let http = reqwest::Client::builder()
            .user_agent("lorelei-cli")
            .build()
            .map_err(|e| format!("failed to build http client: {e}"))?;
        Ok(Self { base_url, http })
    }

    pub fn default_base_url(explicit: Option<String>) -> String {
        if let Some(v) = explicit {
            return v;
        }
        std::env::var("LORELEI_HARBOR_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
    }

    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, HarborHttpError> {
        let url = format!("{}{}", self.base_url, path);
        let res = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| HarborHttpError {
                method: "GET",
                url,
                status: None,
                body: Some(e.to_string()),
            })?;
        Self::decode_json("GET", res).await
    }

    pub async fn get_status(&self, path: &str) -> Result<StatusCode, HarborHttpError> {
        let url = format!("{}{}", self.base_url, path);
        let res = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| HarborHttpError {
                method: "GET",
                url,
                status: None,
                body: Some(e.to_string()),
            })?;
        let status = res.status();
        if status.is_success() {
            Ok(status)
        } else {
            let final_url = res.url().to_string();
            let body = res.text().await.ok();
            Err(HarborHttpError {
                method: "GET",
                url: final_url,
                status: Some(status),
                body,
            })
        }
    }

    pub async fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, HarborHttpError> {
        let url = format!("{}{}", self.base_url, path);
        let res = self
            .http
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| HarborHttpError {
                method: "POST",
                url,
                status: None,
                body: Some(e.to_string()),
            })?;
        Self::decode_json("POST", res).await
    }

    pub async fn delete_empty(&self, path: &str) -> Result<(), HarborHttpError> {
        let url = format!("{}{}", self.base_url, path);
        let res = self
            .http
            .delete(&url)
            .send()
            .await
            .map_err(|e| HarborHttpError {
                method: "DELETE",
                url: url.clone(),
                status: None,
                body: Some(e.to_string()),
            })?;

        if res.status().is_success() {
            Ok(())
        } else {
            let status = res.status();
            let body = res.text().await.ok();
            Err(HarborHttpError {
                method: "DELETE",
                url,
                status: Some(status),
                body,
            })
        }
    }

    async fn decode_json<T: DeserializeOwned>(
        method: &'static str,
        res: reqwest::Response,
    ) -> Result<T, HarborHttpError> {
        let status = res.status();
        let url = res.url().to_string();
        let text = res.text().await.ok();

        if !status.is_success() {
            return Err(HarborHttpError {
                method,
                url,
                status: Some(status),
                body: text,
            });
        }

        let Some(text) = text else {
            return Err(HarborHttpError {
                method,
                url,
                status: Some(status),
                body: Some("empty response body".to_string()),
            });
        };

        serde_json::from_str(&text).map_err(|e| HarborHttpError {
            method,
            url,
            status: Some(status),
            body: Some(format!("invalid json response: {e}")),
        })
    }
}
