//! HTTP client for the Ray Jobs API.
//!
//! Wraps the Ray Jobs REST API endpoints:
//! - `POST /api/jobs/` — submit a new job
//! - `GET  /api/jobs/` — list all jobs
//! - `GET  /api/jobs/{id}` — get job details/status
//! - `POST /api/jobs/{id}/stop` — stop a running job
//! - `GET  /api/version` — check Ray dashboard version
//! - `GET  /api/nodes` — list cluster nodes (autoscaler awareness)
//! - `POST /api/placement_groups/` — create placement groups
//! - `GET  /api/placement_groups/{id}` — get placement group status

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use reqwest::{Method, RequestBuilder};
use serde::{Deserialize, Serialize};

use crate::autoscaler::NodesResponse;
use crate::dashboard::JobListResponse;
use crate::error::RayError;
use crate::placement_group::{PlacementGroupConfig, PlacementGroupResponse, PlacementGroupStatus};

/// Submission request payload for the Ray Jobs API.
#[derive(Debug, Serialize)]
pub struct JobSubmitRequest {
    /// The shell command to execute as the job entrypoint.
    pub entrypoint: String,
    /// Optional submission ID (if not provided, Ray generates one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submission_id: Option<String>,
    /// Resource requirements for the job's entrypoint process.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint_num_cpus: Option<f64>,
    /// Memory requirement in bytes for the entrypoint process.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint_num_gpus: Option<f64>,
    /// Additional resources for the entrypoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint_resources: Option<HashMap<String, f64>>,
    /// Environment variables to set for the job.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_env: Option<serde_json::Value>,
    /// Metadata key-value pairs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}

/// Response from a job submission.
#[derive(Debug, Deserialize)]
pub struct JobSubmitResponse {
    /// The unique submission ID for the job.
    pub submission_id: String,
}

/// Ray job status as returned by the Jobs API.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RayJobStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Stopped,
}

/// Response from getting job details.
#[derive(Debug, Deserialize)]
pub struct JobDetailsResponse {
    /// Current status of the job.
    pub status: RayJobStatus,
    /// Human-readable status message.
    #[serde(default)]
    pub message: Option<String>,
    /// The entrypoint command.
    #[serde(default)]
    pub entrypoint: Option<String>,
    /// Start time in milliseconds since epoch (if started).
    #[serde(default)]
    pub start_time: Option<u64>,
    /// End time in milliseconds since epoch (if finished).
    #[serde(default)]
    pub end_time: Option<u64>,
    /// Job metadata.
    #[serde(default)]
    pub metadata: Option<HashMap<String, String>>,
}

/// Response from the Ray version endpoint.
#[derive(Debug, Deserialize)]
pub struct VersionResponse {
    /// Ray version string.
    pub ray_version: String,
}

#[derive(Clone)]
enum RayAuth {
    Disabled {
        mode: Option<String>,
    },
    Token {
        token: String,
        sources_consulted: Vec<&'static str>,
    },
}

impl RayAuth {
    fn from_environment() -> Result<Self, RayError> {
        Self::resolve(
            std::env::var("RAY_AUTH_MODE").ok(),
            std::env::var("RAY_AUTH_TOKEN").ok(),
            std::env::var_os("RAY_AUTH_TOKEN_PATH").map(PathBuf::from),
            std::env::var_os("HOME").map(PathBuf::from),
        )
    }

    fn resolve(
        mode: Option<String>,
        env_token: Option<String>,
        token_path: Option<PathBuf>,
        home: Option<PathBuf>,
    ) -> Result<Self, RayError> {
        if !mode
            .as_deref()
            .is_some_and(|mode| mode.eq_ignore_ascii_case("token"))
        {
            return Ok(Self::Disabled { mode });
        }

        let mut sources_consulted = vec!["RAY_AUTH_TOKEN"];
        if let Some(token) = env_token {
            if let Some(token) = Self::normalize_token(token, "RAY_AUTH_TOKEN")? {
                return Ok(Self::Token {
                    token,
                    sources_consulted,
                });
            }
        }

        sources_consulted.push("RAY_AUTH_TOKEN_PATH");
        if let Some(path) = token_path.as_deref() {
            if let Some(token) = Self::read_token_file(path, "RAY_AUTH_TOKEN_PATH", false)? {
                return Ok(Self::Token {
                    token,
                    sources_consulted,
                });
            }
        }

        sources_consulted.push("~/.ray/auth_token");
        if let Some(path) = home.as_deref().map(|home| home.join(".ray/auth_token")) {
            if let Some(token) = Self::read_token_file(&path, "~/.ray/auth_token", true)? {
                return Ok(Self::Token {
                    token,
                    sources_consulted,
                });
            }
        }

        Err(RayError::AuthTokenMissing)
    }

    fn read_token_file(
        path: &std::path::Path,
        source: &str,
        missing_is_absent: bool,
    ) -> Result<Option<String>, RayError> {
        match std::fs::read_to_string(path) {
            Ok(token) => Self::normalize_token(token, &format!("{source} ({})", path.display())),
            Err(error) if missing_is_absent && error.kind() == std::io::ErrorKind::NotFound => {
                Ok(None)
            }
            Err(error) => Err(RayError::AuthTokenRead {
                token_source: source.to_string(),
                path: path.to_path_buf(),
                reason: error.to_string(),
            }),
        }
    }

    fn normalize_token(token: String, source: &str) -> Result<Option<String>, RayError> {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Ok(None);
        }
        reqwest::header::HeaderValue::from_str(&token).map_err(|error| {
            RayError::AuthTokenMalformed {
                token_source: source.to_string(),
                reason: error.to_string(),
            }
        })?;
        Ok(Some(token))
    }

    fn is_token(&self) -> bool {
        matches!(self, Self::Token { .. })
    }

    fn mode_diagnostic(&self) -> String {
        match self {
            Self::Disabled { mode: Some(mode) } => format!("{mode:?}"),
            Self::Disabled { mode: None } => "unset".to_string(),
            Self::Token { .. } => "\"token\"".to_string(),
        }
    }

    fn sources_diagnostic(&self) -> String {
        match self {
            Self::Disabled { .. } => {
                "none (token lookup only runs when RAY_AUTH_MODE=token)".to_string()
            }
            Self::Token {
                sources_consulted, ..
            } => sources_consulted.join(", "),
        }
    }
}

impl fmt::Debug for RayAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled { mode } => formatter
                .debug_struct("Disabled")
                .field("mode", mode)
                .finish(),
            Self::Token {
                sources_consulted, ..
            } => formatter
                .debug_struct("Token")
                .field("token", &"[REDACTED]")
                .field("sources_consulted", sources_consulted)
                .finish(),
        }
    }
}

/// HTTP client for the Ray Jobs API.
#[derive(Clone)]
pub struct RayClient {
    /// Base URL for the Ray dashboard (e.g., `http://127.0.0.1:8265`).
    base_url: String,
    /// The underlying HTTP client.
    client: reqwest::Client,
    /// Optional dashboard authentication. Its debug representation is redacted.
    auth: RayAuth,
}

impl fmt::Debug for RayClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RayClient")
            .field("base_url", &self.base_url)
            .field("client", &self.client)
            .field("auth", &self.auth)
            .finish()
    }
}

impl RayClient {
    /// Create a new Ray API client.
    pub fn new(base_url: String, client: reqwest::Client) -> Result<Self, RayError> {
        Self::with_auth(base_url, client, RayAuth::from_environment()?)
    }

    fn with_auth(
        base_url: String,
        client: reqwest::Client,
        auth: RayAuth,
    ) -> Result<Self, RayError> {
        // Strip trailing slash for consistent URL construction.
        let base_url = base_url.trim_end_matches('/').to_string();
        Ok(Self {
            base_url,
            client,
            auth,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_test_token(base_url: String, client: reqwest::Client, token: &str) -> Self {
        Self::with_auth(
            base_url,
            client,
            RayAuth::Token {
                token: token.to_string(),
                sources_consulted: vec!["RAY_AUTH_TOKEN"],
            },
        )
        .expect("test Ray client should be constructible")
    }

    fn request(&self, method: Method, url: &str) -> RequestBuilder {
        let request = self.client.request(method, url);
        match &self.auth {
            RayAuth::Token { token, .. } => request.bearer_auth(token),
            RayAuth::Disabled { .. } => request,
        }
    }

    pub(crate) fn token_mode(&self) -> bool {
        self.auth.is_token()
    }

    /// Check connectivity by querying the Ray version endpoint.
    pub async fn version(&self) -> Result<VersionResponse, RayError> {
        let url = format!("{}/api/version", self.base_url);
        let resp = self
            .request(Method::GET, &url)
            .send()
            .await
            .map_err(|e| RayError::ClusterUnreachable(format!("GET {url}: {e}")))?;
        self.check_status(&url, resp).await
    }

    /// Submit a new job to the Ray cluster.
    pub async fn submit_job(
        &self,
        request: &JobSubmitRequest,
    ) -> Result<JobSubmitResponse, RayError> {
        let url = format!("{}/api/jobs/", self.base_url);
        let resp = self
            .request(Method::POST, &url)
            .json(request)
            .send()
            .await?;
        self.check_status(&url, resp).await
    }

    /// Get the details and status of a submitted job.
    pub async fn get_job_details(
        &self,
        submission_id: &str,
    ) -> Result<JobDetailsResponse, RayError> {
        let url = format!("{}/api/jobs/{}", self.base_url, submission_id);
        let resp = self.request(Method::GET, &url).send().await?;
        self.check_status(&url, resp).await
    }

    /// Retrieve the driver's stdout and stderr from the Jobs API.
    pub async fn get_job_logs(&self, submission_id: &str) -> Result<String, RayError> {
        #[derive(Deserialize)]
        struct Logs {
            logs: String,
        }
        let url = format!("{}/api/jobs/{submission_id}/logs", self.base_url);
        let response = self.request(Method::GET, &url).send().await?;
        let logs: Logs = self.check_status(&url, response).await?;
        Ok(logs.logs)
    }

    /// Stop a running job.
    pub async fn stop_job(&self, submission_id: &str) -> Result<(), RayError> {
        let url = format!("{}/api/jobs/{}/stop", self.base_url, submission_id);
        let resp = self.request(Method::POST, &url).send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(self.response_error(resp).await)
        }
    }

    /// List all jobs on the cluster.
    pub async fn list_jobs(&self) -> Result<JobListResponse, RayError> {
        let url = format!("{}/api/jobs/", self.base_url);
        let resp = self.request(Method::GET, &url).send().await?;
        self.check_status(&url, resp).await
    }

    /// Get cluster node information (for autoscaler-aware concurrency).
    pub async fn get_nodes(&self) -> Result<NodesResponse, RayError> {
        let url = format!("{}/api/nodes", self.base_url);
        let resp = self.request(Method::GET, &url).send().await?;
        self.check_status(&url, resp).await
    }

    /// Inspect all nodes through Ray's State API with a single bounded deadline.
    /// The deadline is an argument so tests can exercise stalled connections.
    pub(crate) async fn inspect_node_capacities(
        &self,
        deadline: std::time::Duration,
    ) -> Result<Vec<crate::feasibility::NodeCapacity>, RayError> {
        let operation = async {
            let response = self
                .request(Method::GET, &format!("{}/api/v0/nodes", self.base_url))
                .query(&[("detail", "1"), ("limit", "10000"), ("timeout", "10")])
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        RayError::NodeInspectionTimeout(deadline)
                    } else {
                        RayError::NodeInspectionConnection(e.to_string())
                    }
                })?;
            if !response.status().is_success() {
                if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                    return Err(self.response_error(response).await);
                }
                return Err(RayError::ApiStatus {
                    status: response.status().as_u16(),
                    body: "Ray node inspection failed; use --ray-allow-pending to bypass node inspection and wait for future capacity".into(),
                });
            }
            let body = response.bytes().await.map_err(|e| {
                if e.is_timeout() {
                    RayError::NodeInspectionTimeout(deadline)
                } else {
                    RayError::NodeInspectionConnection(e.to_string())
                }
            })?;
            let value = serde_json::from_slice(&body)
                .map_err(|e| RayError::NodeInspectionPayload(format!("invalid JSON: {e}")))?;
            crate::feasibility::parse_nodes(value)
        };
        tokio::time::timeout(deadline, operation)
            .await
            .map_err(|_| RayError::NodeInspectionTimeout(deadline))?
    }

    /// Create a placement group.
    pub async fn create_placement_group(
        &self,
        config: &PlacementGroupConfig,
    ) -> Result<PlacementGroupResponse, RayError> {
        let url = format!("{}/api/placement_groups/", self.base_url);
        let body = config.to_ray_request();
        let resp = self
            .request(Method::POST, &url)
            .json(&body)
            .send()
            .await
            .map_err(|e| RayError::PlacementGroup(format!("POST {url}: {e}")))?;
        self.check_status(&url, resp).await
    }

    /// Get placement group status.
    pub async fn get_placement_group_status(
        &self,
        pg_id: &str,
    ) -> Result<PlacementGroupStatus, RayError> {
        let url = format!("{}/api/placement_groups/{}", self.base_url, pg_id);
        let resp = self.request(Method::GET, &url).send().await?;

        #[derive(serde::Deserialize)]
        struct PgStatusResponse {
            status: PlacementGroupStatus,
        }

        let status_resp: PgStatusResponse = self.check_status(&url, resp).await?;
        Ok(status_resp.status)
    }

    /// Remove a placement group.
    pub async fn remove_placement_group(&self, pg_id: &str) -> Result<(), RayError> {
        let url = format!("{}/api/placement_groups/{}", self.base_url, pg_id);
        let resp = self.request(Method::DELETE, &url).send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(self.response_error(resp).await)
        }
    }

    /// Check HTTP response status and deserialize the JSON body.
    async fn check_status<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        resp: reqwest::Response,
    ) -> Result<T, RayError> {
        let status = resp.status();
        if !status.is_success() {
            return Err(self.response_error(resp).await);
        }
        resp.json::<T>()
            .await
            .map_err(|e| RayError::ParseError(format!("failed to parse response from {url}: {e}")))
    }

    async fn response_error(&self, resp: reqwest::Response) -> RayError {
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return RayError::AuthRequired {
                mode: self.auth.mode_diagnostic(),
                sources: self.auth.sources_diagnostic(),
            };
        }
        RayError::ApiStatus {
            status: status.as_u16(),
            body: resp.text().await.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod inspection_tests {
    use super::*;
    use std::time::Duration;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TOKEN: &str = "oxymake-distinctive-secret-7f3a";

    fn token_auth(token: &str, sources_consulted: Vec<&'static str>) -> RayAuth {
        RayAuth::Token {
            token: token.to_string(),
            sources_consulted,
        }
    }

    fn client_with_auth(server: &MockServer, auth: RayAuth) -> RayClient {
        RayClient::with_auth(server.uri(), reqwest::Client::new(), auth).unwrap()
    }

    fn token_value(auth: &RayAuth) -> Option<&str> {
        match auth {
            RayAuth::Token { token, .. } => Some(token),
            RayAuth::Disabled { .. } => None,
        }
    }

    #[test]
    fn token_precedence_and_file_trimming() {
        let root = tempfile::tempdir().unwrap();
        let explicit = root.path().join("explicit-token");
        let default = root.path().join(".ray/auth_token");
        std::fs::create_dir_all(default.parent().unwrap()).unwrap();
        std::fs::write(&explicit, " \tpath-token\n").unwrap();
        std::fs::write(&default, "\nhome-token \t\n").unwrap();

        let from_env = RayAuth::resolve(
            Some("token".into()),
            Some(" \tenv-token\r\n".into()),
            Some(explicit.clone()),
            Some(root.path().into()),
        )
        .unwrap();
        assert_eq!(token_value(&from_env), Some("env-token"));

        let from_path = RayAuth::resolve(
            Some("token".into()),
            None,
            Some(explicit),
            Some(root.path().into()),
        )
        .unwrap();
        assert_eq!(token_value(&from_path), Some("path-token"));

        let from_home =
            RayAuth::resolve(Some("token".into()), None, None, Some(root.path().into())).unwrap();
        assert_eq!(token_value(&from_home), Some("home-token"));
    }

    #[test]
    fn token_mode_is_case_insensitive() {
        let auth =
            RayAuth::resolve(Some("TOKEN".into()), Some("secret".into()), None, None).unwrap();
        assert!(auth.is_token());
        assert_eq!(token_value(&auth), Some("secret"));
    }

    #[test]
    fn malformed_env_token_names_its_source_without_leaking_the_token() {
        let malformed = "secret\nvalue";
        let error =
            RayAuth::resolve(Some("token".into()), Some(malformed.into()), None, None).unwrap_err();
        let rendered = error.to_string();
        assert!(matches!(error, RayError::AuthTokenMalformed { .. }));
        assert!(rendered.contains("malformed"));
        assert!(rendered.contains("RAY_AUTH_TOKEN"));
        assert!(!rendered.contains(malformed));
    }

    #[test]
    fn unreadable_explicit_token_path_is_not_ignored() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("missing-explicit-token");
        let default = root.path().join(".ray/auth_token");
        std::fs::create_dir_all(default.parent().unwrap()).unwrap();
        std::fs::write(default, "different-token").unwrap();
        let error = RayAuth::resolve(
            Some("token".into()),
            None,
            Some(path.clone()),
            Some(root.path().into()),
        )
        .unwrap_err();
        let rendered = error.to_string();
        let RayError::AuthTokenRead {
            token_source,
            path: error_path,
            reason,
        } = &error
        else {
            panic!("unexpected error: {error}");
        };
        assert_eq!(token_source, "RAY_AUTH_TOKEN_PATH");
        assert_eq!(error_path, &path);
        assert!(!reason.is_empty());
        assert!(rendered.contains("RAY_AUTH_TOKEN_PATH"));
        assert!(rendered.contains(&path.display().to_string()));
        assert!(rendered.contains(reason));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_default_token_is_distinct_from_an_absent_token() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".ray/auth_token");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "secret").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        let error = RayAuth::resolve(Some("token".into()), None, None, Some(root.path().into()))
            .unwrap_err();
        let rendered = error.to_string();
        let RayError::AuthTokenRead {
            token_source,
            path: error_path,
            reason,
        } = &error
        else {
            panic!("unexpected error: {error}");
        };
        assert_eq!(token_source, "~/.ray/auth_token");
        assert_eq!(error_path, &path);
        assert!(!reason.is_empty());
        assert!(rendered.contains("~/.ray/auth_token"));
        assert!(rendered.contains(&path.display().to_string()));
        assert!(rendered.contains(reason));
    }

    #[test]
    fn token_mode_without_a_token_fails_before_a_request_can_be_built() {
        let root = tempfile::tempdir().unwrap();
        let error = RayAuth::resolve(
            Some("token".into()),
            None,
            None,
            Some(root.path().join("missing-home")),
        )
        .unwrap_err();
        let rendered = error.to_string();
        assert!(matches!(error, RayError::AuthTokenMissing));
        for source in ["RAY_AUTH_TOKEN", "RAY_AUTH_TOKEN_PATH", "~/.ray/auth_token"] {
            assert!(rendered.contains(source), "missing {source}: {rendered}");
        }

        let disabled = RayAuth::resolve(
            Some("something-else".into()),
            None,
            Some(root.path().join("missing")),
            None,
        )
        .unwrap();
        assert!(!disabled.is_token());
    }

    #[tokio::test]
    async fn all_dashboard_calls_send_the_bearer_token() {
        let server = MockServer::start().await;
        let cases = [
            (
                "POST",
                "/api/jobs/",
                serde_json::json!({"submission_id":"job"}),
            ),
            (
                "GET",
                "/api/jobs/job",
                serde_json::json!({"status":"RUNNING"}),
            ),
            (
                "GET",
                "/api/jobs/job/logs",
                serde_json::json!({"logs":"ok"}),
            ),
            ("POST", "/api/jobs/job/stop", serde_json::json!({})),
            (
                "GET",
                "/api/v0/nodes",
                serde_json::json!({"result":true,"data":{"result":{"total":0,"num_after_truncation":0,"num_filtered":0,"result":[]}}}),
            ),
        ];
        for (verb, route, body) in cases {
            Mock::given(method(verb))
                .and(path(route))
                .and(header("authorization", format!("Bearer {TOKEN}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
        }
        let client = client_with_auth(&server, token_auth(TOKEN, vec!["RAY_AUTH_TOKEN"]));
        let request = JobSubmitRequest {
            entrypoint: "true".into(),
            submission_id: None,
            entrypoint_num_cpus: None,
            entrypoint_num_gpus: None,
            entrypoint_resources: None,
            runtime_env: None,
            metadata: None,
        };
        client.submit_job(&request).await.unwrap();
        client.get_job_details("job").await.unwrap();
        client.get_job_logs("job").await.unwrap();
        client.stop_job("job").await.unwrap();
        client
            .inspect_node_capacities(Duration::from_secs(1))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn disabled_auth_sends_no_authorization_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/version"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"ray_version": "2.9.3"})),
            )
            .mount(&server)
            .await;
        let client = client_with_auth(&server, RayAuth::Disabled { mode: None });
        client.version().await.unwrap();
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(!requests[0].headers.contains_key("authorization"));
    }

    #[tokio::test]
    async fn unauthorized_diagnostic_names_mode_and_sources_but_not_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/jobs/job"))
            .respond_with(ResponseTemplate::new(401).set_body_string(TOKEN))
            .mount(&server)
            .await;
        let client = client_with_auth(
            &server,
            token_auth(TOKEN, vec!["RAY_AUTH_TOKEN", "RAY_AUTH_TOKEN_PATH"]),
        );
        let error = client.get_job_details("job").await.unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.contains("cluster requires a Ray token"));
        assert!(rendered.contains("RAY_AUTH_MODE is \"token\""));
        assert!(rendered.contains("RAY_AUTH_TOKEN_PATH"));
        assert!(!rendered.contains(TOKEN));
        assert!(!format!("{client:?}").contains(TOKEN));
    }

    #[tokio::test]
    async fn unauthorized_without_token_mode_reports_that_no_sources_were_read() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/version"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let client = client_with_auth(&server, RayAuth::Disabled { mode: None });
        let rendered = client.version().await.unwrap_err().to_string();
        assert!(rendered.contains("RAY_AUTH_MODE is unset"));
        assert!(rendered.contains("sources consulted: none"));
    }

    #[tokio::test]
    async fn inspection_diagnostics_are_distinct_and_bounded() {
        for (body, expected) in [
            ("not-json", "invalid JSON"),
            ("{}", "unknown"),
            (
                r#"{"result":true,"data":{"result":{"total":1,"num_after_truncation":1,"num_filtered":1,"result":[{"node_ip":"a","state":"ALIEN","resources_total":{"CPU":1}}]}}}"#,
                "unknown node state",
            ),
            (
                r#"{"result":true,"data":{"result":{"total":2,"num_after_truncation":1,"num_filtered":1,"result":[{}]}}}"#,
                "truncated",
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/api/v0/nodes"))
                .respond_with(ResponseTemplate::new(200).set_body_string(body))
                .mount(&server)
                .await;
            let client = RayClient::new(server.uri(), reqwest::Client::new()).unwrap();
            let error = client
                .inspect_node_capacities(Duration::from_secs(1))
                .await
                .unwrap_err();
            assert!(matches!(error, RayError::NodeInspectionPayload(_)));
            assert!(error.to_string().contains(expected), "{error}");
        }
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v0/nodes"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
            .mount(&server)
            .await;
        let client = RayClient::new(server.uri(), reqwest::Client::new()).unwrap();
        let start = std::time::Instant::now();
        assert!(matches!(
            client
                .inspect_node_capacities(Duration::from_millis(20))
                .await,
            Err(RayError::NodeInspectionTimeout(_))
        ));
        assert!(start.elapsed() < Duration::from_secs(1));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let client = RayClient::new(format!("http://{addr}"), reqwest::Client::new()).unwrap();
        assert!(matches!(
            client.inspect_node_capacities(Duration::from_secs(1)).await,
            Err(RayError::NodeInspectionConnection(_))
        ));
    }
}
