//! CLI commands for the resources the console manages beside Applications:
//! Virtual machines, routes and databases. Each one speaks to the Operator
//! API, the way the console does.

pub mod databases;
pub mod routes;
pub mod vms;

use reqwest::Method;
use self_host::config::CliConfig;
use self_host::error::ErrorReport;
use serde_json::Value;

/// The Operator API, as the CLI config points at it.
pub struct Api {
    pub client: reqwest::Client,
    pub config: CliConfig,
}

impl Api {
    pub fn connect() -> anyhow::Result<Self> {
        let config = CliConfig::load()?.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "CLI config not found; run 'self-host init' first",
            )
        })?;
        Ok(Self {
            client: crate::platform_api_client()?,
            config,
        })
    }

    pub async fn get(&self, path: &str, what: &str) -> anyhow::Result<Value> {
        self.send(Method::GET, path, None, what).await
    }

    /// Sends one request and answers its JSON body, or the failure the
    /// platform reported. An empty body reads as `null`.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        what: &str,
    ) -> anyhow::Result<Value> {
        let url = format!("{}{path}", self.config.api_base_url.trim_end_matches('/'));
        let mut request = self
            .client
            .request(method, url)
            .bearer_auth(&self.config.api_key);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            return Err(crate::api_error(what, status, &text));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// The Application with this exact name, as `GET /apps` lists it.
    pub async fn application(&self, name: &str) -> anyhow::Result<Value> {
        let apps = self.get("/apps", "List failed").await?;
        apps.as_array()
            .into_iter()
            .flatten()
            .find(|app| app["name"].as_str() == Some(name))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Application '{name}' not found"))
    }
}

/// Whether a task has finished: `None` while it is queued or running, then its
/// event, or the failure it recorded.
pub async fn task_outcome(api: &Api, task_id: &str, what: &str) -> anyhow::Result<Option<Value>> {
    let events = api.get("/events", "Events unavailable").await?;
    let Some(event) = events
        .as_array()
        .into_iter()
        .flatten()
        .find(|event| event["id"].as_str() == Some(task_id))
    else {
        return Ok(None);
    };
    match event["status"].as_str() {
        Some("completed") => Ok(Some(event.clone())),
        Some("failed") => {
            let report = serde_json::from_value(event["error"].clone())
                .unwrap_or_else(|_| ErrorReport::plain("unknown error"));
            Err(crate::chain_from(report).context(what.to_owned()))
        }
        _ => Ok(None),
    }
}

/// Waits for the task an accepted request queued, by its event. There is no
/// upper bound: Ctrl-C leaves the task running on the Platform.
pub async fn wait_for_task(api: &Api, accepted: &Value, what: &str) -> anyhow::Result<Value> {
    let task_id = accepted["task_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("the Platform did not return a task id"))?;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if let Some(event) = task_outcome(api, task_id, what).await? {
            return Ok(event);
        }
    }
}

/// One path segment, percent-encoded, for an id or a name in a URL path.
pub fn segment(value: &str) -> String {
    let mut url = reqwest::Url::parse("http://x/").expect("static URL");
    url.path_segments_mut()
        .expect("http URL has a path")
        .push(value);
    url.path().trim_start_matches('/').to_owned()
}

/// Asks a yes-or-no question on the terminal; anything but `y` is no.
pub fn confirm(question: &str) -> anyhow::Result<bool> {
    print!("{question} [y/N] ");
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(input.trim().eq_ignore_ascii_case("y"))
}

pub fn str<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("-")
}
