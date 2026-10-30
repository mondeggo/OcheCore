use anyhow::{Context, Result, bail};
use oche_domain::{ApiError, CameraInventory, Command, ConfigChange, NativeEvent, Snapshot};
use reqwest::{Client as HttpClient, Response};
use serde::de::DeserializeOwned;
use std::time::Duration;

#[derive(Clone)]
pub struct Client {
    pub endpoint: String,
    http: HttpClient,
}

impl Client {
    pub fn new(host: &str, port: u16) -> Result<Self> {
        let host = if host.parse::<std::net::Ipv6Addr>().is_ok() {
            format!("[{host}]")
        } else {
            host.into()
        };
        let url = reqwest::Url::parse(&format!("http://{host}:{port}/api/v1"))?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/api/v1"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            bail!(
                "--host must be a hostname or IP address, without credentials, a scheme, port, or path"
            );
        }
        Ok(Self {
            endpoint: url.to_string(),
            http: HttpClient::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(2))
                .build()?,
        })
    }

    async fn decode<T: DeserializeOwned>(response: Response) -> Result<T> {
        let status = response.status();
        if !status.is_success() {
            let error = response.json::<ApiError>().await.context(
                "server returned an unexpected response; check the native OcheCore address",
            )?;
            bail!("{}: {}", error.code, error.message);
        }
        response.json().await.context("invalid native API response")
    }

    pub async fn snapshot(&self) -> Result<Snapshot> {
        let response = self.http.get(format!("{}/status", self.endpoint)).timeout(Duration::from_secs(3)).send().await
            .with_context(|| format!("cannot reach {}; start `oche-core run` in another terminal, or check --host and --port", self.endpoint))?;
        Self::decode(response).await
    }

    pub async fn history(&self) -> Result<Vec<NativeEvent>> {
        Self::decode(
            self.http
                .get(format!("{}/events/recent", self.endpoint))
                .timeout(Duration::from_secs(3))
                .send()
                .await?,
        )
        .await
    }

    pub async fn cameras(&self) -> Result<CameraInventory> {
        Self::decode(
            self.http
                .get(format!("{}/cameras/available", self.endpoint))
                .timeout(Duration::from_secs(3))
                .send()
                .await?,
        )
        .await
    }

    /// The latest JPEG frame of a streaming camera.
    pub async fn camera_snapshot(&self, slot: u8) -> Result<Vec<u8>> {
        let response = self
            .http
            .get(format!("{}/cameras/{slot}/snapshot", self.endpoint))
            .timeout(Duration::from_secs(5))
            .send()
            .await?;
        if !response.status().is_success() {
            return Self::decode::<Snapshot>(response)
                .await
                .map(|_| unreachable!());
        }
        Ok(response.bytes().await?.to_vec())
    }

    pub async fn command(&self, command: &Command) -> Result<Snapshot> {
        // Calibration measures every camera's frame on the board's machine.
        let timeout = if matches!(command, Command::Calibrate { .. }) {
            Duration::from_secs(90)
        } else {
            Duration::from_secs(3)
        };
        Self::decode(
            self.http
                .post(format!("{}/command", self.endpoint))
                .json(command)
                .timeout(timeout)
                .send()
                .await?,
        )
        .await
    }

    pub async fn configure(&self, change: &ConfigChange) -> Result<Snapshot> {
        Self::decode(
            self.http
                .put(format!("{}/config", self.endpoint))
                .json(change)
                .timeout(Duration::from_secs(3))
                .send()
                .await?,
        )
        .await
    }

    pub async fn events(&self) -> Result<Response> {
        let response = tokio::time::timeout(
            Duration::from_secs(3),
            self.http.get(format!("{}/events", self.endpoint)).send(),
        )
        .await??;
        if !response.status().is_success() {
            return Self::decode::<Snapshot>(response)
                .await
                .map(|_| unreachable!());
        }
        Ok(response)
    }
}
