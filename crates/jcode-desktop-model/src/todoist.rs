//! Small synchronous client for the Todoist API v1.

use reqwest::{
    Method, StatusCode,
    blocking::{Client, Response},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{env, error::Error, fmt, time::Duration};

const DEFAULT_BASE_URL: &str = "https://api.todoist.com";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Due {
    pub string: String,
    pub date: String,
    #[serde(default)]
    pub is_recurring: bool,
    #[serde(default)]
    pub datetime: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub description: String,
    pub project_id: String,
    #[serde(default)]
    pub section_id: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub priority: u8,
    #[serde(default)]
    pub due: Option<Due>,
    #[serde(default)]
    pub is_completed: bool,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub order: Option<i64>,
    #[serde(default)]
    pub is_favorite: bool,
    #[serde(default)]
    pub is_inbox_project: bool,
    #[serde(default)]
    pub is_shared: bool,
    #[serde(default)]
    pub view_style: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CreateTask<'a> {
    pub content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_string: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_datetime: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_lang: Option<&'a str>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateTask<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_string: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_datetime: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_lang: Option<&'a str>,
}

/// A UI-independent semantic name for Todoist priorities (4 is highest).
pub fn priority_color(priority: u8) -> &'static str {
    match priority {
        4 => "red",
        3 => "orange",
        2 => "blue",
        _ => "neutral",
    }
}

#[derive(Debug)]
pub enum TodoistError {
    MissingToken,
    InvalidBaseUrl,
    Transport(String),
    Api { status: StatusCode, message: String },
    Decode(String),
}

impl fmt::Display for TodoistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingToken => write!(f, "TODOIST_API_TOKEN is not set or is empty"),
            Self::InvalidBaseUrl => write!(f, "invalid Todoist base URL"),
            Self::Transport(message) => write!(f, "Todoist request failed: {message}"),
            Self::Api { status, message } => write!(f, "Todoist API returned {status}: {message}"),
            Self::Decode(message) => write!(f, "invalid Todoist response: {message}"),
        }
    }
}

impl Error for TodoistError {}

pub type Result<T> = std::result::Result<T, TodoistError>;

#[derive(Debug, Clone)]
pub struct TodoistClient {
    client: Client,
    token: String,
    base_url: String,
}

impl TodoistClient {
    /// Builds a production client. The token is read only from `TODOIST_API_TOKEN`.
    pub fn from_env() -> Result<Self> {
        let token = env::var("TODOIST_API_TOKEN").map_err(|_| TodoistError::MissingToken)?;
        Self::new(token, DEFAULT_BASE_URL)
    }

    /// Builds a client with an alternate origin, primarily for local test servers.
    pub fn with_base_url(base_url: impl Into<String>) -> Result<Self> {
        let token = env::var("TODOIST_API_TOKEN").map_err(|_| TodoistError::MissingToken)?;
        Self::new(token, base_url)
    }

    fn new(token: String, base_url: impl Into<String>) -> Result<Self> {
        if token.trim().is_empty() {
            return Err(TodoistError::MissingToken);
        }
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        let parsed = reqwest::Url::parse(&base_url).map_err(|_| TodoistError::InvalidBaseUrl)?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.cannot_be_a_base() {
            return Err(TodoistError::InvalidBaseUrl);
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| TodoistError::Transport(sanitize(&error.to_string(), &token)))?;
        Ok(Self {
            client,
            token,
            base_url,
        })
    }

    pub fn tasks(&self) -> Result<Vec<Task>> {
        self.get_all("tasks")
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        self.get_all("projects")
    }

    pub fn create_task(&self, task: &CreateTask<'_>) -> Result<Task> {
        self.json_request(Method::POST, "tasks", Some(task))
    }

    pub fn update_task(&self, id: &str, task: &UpdateTask<'_>) -> Result<Task> {
        self.json_request(
            Method::POST,
            &format!("tasks/{}", encode_id(id)),
            Some(task),
        )
    }

    pub fn close_task(&self, id: &str) -> Result<()> {
        self.empty_request(Method::POST, &format!("tasks/{}/close", encode_id(id)))
    }

    pub fn reopen_task(&self, id: &str) -> Result<()> {
        self.empty_request(Method::POST, &format!("tasks/{}/reopen", encode_id(id)))
    }

    pub fn delete_task(&self, id: &str) -> Result<()> {
        self.empty_request(Method::DELETE, &format!("tasks/{}", encode_id(id)))
    }

    fn get_all<T: DeserializeOwned>(&self, resource: &str) -> Result<Vec<T>> {
        let mut cursor: Option<String> = None;
        let mut all = Vec::new();
        loop {
            let mut request = self.request(Method::GET, resource);
            if let Some(value) = cursor.as_deref() {
                request = request.query(&[("cursor", value)]);
            }
            let value: Value =
                self.decode_response(request.send().map_err(|e| self.transport(e))?)?;
            let page: Page<T> = serde_json::from_value(value)
                .map_err(|error| TodoistError::Decode(clean_decode_error(error)))?;
            let (results, next) = page.parts();
            all.extend(results);
            match next.filter(|value| !value.is_empty()) {
                Some(next) => cursor = Some(next),
                None => return Ok(all),
            }
        }
    }

    fn json_request<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T> {
        let mut request = self.request(method, path);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().map_err(|e| self.transport(e))?;
        self.decode_response(response)
    }

    fn empty_request(&self, method: Method, path: &str) -> Result<()> {
        let response = self
            .request(method, path)
            .send()
            .map_err(|e| self.transport(e))?;
        self.check_status(response).map(|_| ())
    }

    fn request(&self, method: Method, path: &str) -> reqwest::blocking::RequestBuilder {
        self.client
            .request(method, format!("{}/api/v1/{}", self.base_url, path))
            .bearer_auth(&self.token)
    }

    fn decode_response<T: DeserializeOwned>(&self, response: Response) -> Result<T> {
        let response = self.check_status(response)?;
        response
            .json()
            .map_err(|error| TodoistError::Decode(clean_decode_error(error)))
    }

    fn check_status(&self, response: Response) -> Result<Response> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let text = response.text().unwrap_or_default();
        Err(TodoistError::Api {
            status,
            message: api_message(&text, &self.token),
        })
    }

    fn transport(&self, error: reqwest::Error) -> TodoistError {
        TodoistError::Transport(sanitize(&error.to_string(), &self.token))
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Page<T> {
    Direct(Vec<T>),
    Wrapped {
        results: Vec<T>,
        #[serde(default)]
        next_cursor: Option<String>,
    },
}

impl<T> Page<T> {
    fn parts(self) -> (Vec<T>, Option<String>) {
        match self {
            Self::Direct(results) => (results, None),
            Self::Wrapped {
                results,
                next_cursor,
            } => (results, next_cursor),
        }
    }
}

fn encode_id(id: &str) -> String {
    id.bytes().fold(String::new(), |mut encoded, byte| {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
        encoded
    })
}

fn clean_decode_error(error: impl fmt::Display) -> String {
    sanitize(&error.to_string(), "")
}

fn api_message(body: &str, token: &str) -> String {
    let parsed = serde_json::from_str::<Value>(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|value| {
            ["error", "error_description", "message"]
                .iter()
                .find_map(|key| value.get(key)?.as_str())
        })
        .unwrap_or(body);
    let message = sanitize(message, token);
    if message.is_empty() {
        "request rejected".into()
    } else {
        message
    }
}

fn sanitize(message: &str, token: &str) -> String {
    let redacted = if token.is_empty() {
        message.to_owned()
    } else {
        message.replace(token, "[REDACTED]")
    };
    let compact = redacted.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_direct_and_wrapped_pages() {
        let direct: Page<Project> = serde_json::from_str(r#"[{"id":"1","name":"Inbox"}]"#).unwrap();
        assert_eq!(direct.parts().0[0].name, "Inbox");
        let wrapped: Page<Project> =
            serde_json::from_str(r#"{"results":[{"id":"2","name":"Work"}],"next_cursor":"abc"}"#)
                .unwrap();
        let (projects, cursor) = wrapped.parts();
        assert_eq!(projects[0].id, "2");
        assert_eq!(cursor.as_deref(), Some("abc"));
    }

    #[test]
    fn parses_current_task_and_due_fields() {
        let task: Task = serde_json::from_str(r#"{
            "id":"42","content":"Ship it","description":"carefully","project_id":"7",
            "priority":4,"labels":["release"],"due":{"string":"tomorrow","date":"2026-08-26","is_recurring":false}
        }"#).unwrap();
        assert_eq!(task.due.unwrap().date, "2026-08-26");
        assert_eq!(priority_color(task.priority), "red");
    }

    #[test]
    fn extracts_and_sanitizes_api_errors() {
        let token = "secret-token";
        let body = r#"{"error":" bad\n secret-token   request "}"#;
        assert_eq!(api_message(body, token), "bad [REDACTED] request");
        assert_eq!(api_message("", token), "request rejected");
        assert_eq!(sanitize(&"x".repeat(400), token).len(), 300);
    }

    #[test]
    fn encodes_task_ids_as_path_segments() {
        assert_eq!(encode_id("a/b ?"), "a%2Fb%20%3F");
    }

    #[test]
    fn rejects_bad_configuration_without_network() {
        assert!(matches!(
            TodoistClient::new("  ".into(), DEFAULT_BASE_URL),
            Err(TodoistError::MissingToken)
        ));
        assert!(matches!(
            TodoistClient::new("token".into(), "not a url"),
            Err(TodoistError::InvalidBaseUrl)
        ));
    }
}
