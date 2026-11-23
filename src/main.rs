use std::{
    fs,
    io::{self, Read},
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use aq_graph::{service, store::Store};
use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[path = "cli/commands.rs"]
mod commands;
#[path = "cli/startup.rs"]
mod startup;
use startup::ServeOptions;

const LOCAL_WORKSPACE: &str = "00000000-0000-4000-8000-000000000001";

#[derive(Parser)]
#[command(
    version,
    about = "Standalone task and knowledge graph client and service"
)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true, env = "AQ_GRAPH_CONFIG")]
    config: Option<PathBuf>,
    #[arg(long, global = true, env = "AQ_GRAPH_ENDPOINT")]
    endpoint: Option<String>,
    #[arg(long, global = true, env = "AQ_GRAPH_WORKSPACE")]
    workspace: Option<Uuid>,
    #[arg(long, global = true, env = "AQ_GRAPH_TOKEN_FILE")]
    token_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write local client configuration. Credentials come from a file or AQ_GRAPH_TOKEN.
    Init {
        /// Generate a local credential if no credential source exists. Does not create a database.
        #[arg(long)]
        local: bool,
        #[arg(long)]
        force: bool,
    },
    /// Run migrations and start the HTTP service against DATABASE_URL.
    Serve(ServeOptions),
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    Knowledge {
        #[command(subcommand)]
        command: KnowledgeCommand,
    },
    Link {
        #[command(subcommand)]
        command: LinkCommand,
    },
    Search {
        query: String,
        #[arg(long, value_parser = ["task", "knowledge"])]
        kind: Option<String>,
        #[command(flatten)]
        filters: DateFilters,
        #[arg(long)]
        include_disputed: bool,
        #[command(flatten)]
        page: Paging,
    },
    /// Attach or clear a summary derived from the current logical revision.
    Summary {
        id: Uuid,
        #[command(flatten)]
        change: ChangeOptions,
        #[arg(long)]
        source_revision: i64,
        #[arg(long, required_unless_present = "clear", conflicts_with = "clear")]
        file: Option<PathBuf>,
        #[arg(long)]
        clear: bool,
    },
    History {
        id: Uuid,
        #[command(flatten)]
        page: Paging,
    },
    /// Read an exact task or knowledge revision and its recorded relationships.
    Revision {
        id: Uuid,
        #[arg(long)]
        revision: i64,
    },
    Diff {
        id: Uuid,
        #[arg(long)]
        from: i64,
        #[arg(long)]
        to: i64,
    },
    /// Submit an atomic MutationRequest JSON document; use - for stdin.
    Mutate {
        #[arg(long)]
        file: PathBuf,
    },
    #[command(alias = "draft")]
    Changeset {
        #[command(subcommand)]
        command: DraftCommand,
    },
    /// Inspect HTTP and database health without authentication.
    Health,
}
#[derive(Args, Default)]
struct Paging {
    #[arg(long, default_value_t = 50)]
    limit: u32,
    #[arg(long)]
    cursor: Option<String>,
}
#[derive(Args, Default)]
struct DateFilters {
    /// Keep records valid at this descriptive instant (not an execution deadline).
    #[arg(long, value_parser = parse_timestamp)]
    valid_at: Option<String>,
    /// Keep records with a nonempty review marker due by this instant.
    #[arg(long, value_parser = parse_timestamp)]
    review_due_at: Option<String>,
    #[arg(long)]
    include_retired: bool,
}
#[derive(Args)]
struct ChangeOptions {
    /// Expected current version; stale updates return a conflict.
    #[arg(long)]
    expected_version: i64,
    #[arg(long)]
    reason: Option<String>,
    #[arg(long)]
    idempotency_key: Option<String>,
}
#[derive(Args, Default)]
struct SharedFields {
    /// Content classification, independent of task versus knowledge kind.
    #[arg(long = "type", value_parser = ["fact", "decision", "policy", "procedure", "incident", "reference", "note"])]
    content_type: Option<String>,
    /// JSON array of typed source descriptors.
    #[arg(long, alias = "sources-file")]
    provenance_file: Option<PathBuf>,
    #[arg(long, value_parser = parse_timestamp)]
    valid_from: Option<String>,
    #[arg(long, value_parser = parse_timestamp)]
    valid_until: Option<String>,
    #[arg(long, value_parser = parse_timestamp)]
    review_at: Option<String>,
    #[arg(long, value_parser = ["unverified"])]
    verification: Option<String>,
}
#[derive(Args, Default)]
struct SharedPatch {
    #[command(flatten)]
    fields: SharedFields,
    #[arg(long, conflicts_with = "valid_from")]
    clear_valid_from: bool,
    #[arg(long, conflicts_with = "valid_until")]
    clear_valid_until: bool,
    #[arg(long, conflicts_with = "review_at")]
    clear_review_at: bool,
    #[arg(long)]
    retired: Option<bool>,
    #[arg(long)]
    retirement_reason: Option<String>,
}
#[derive(Args)]
struct CreateOptions {
    #[arg(long)]
    title: String,
    #[arg(long, conflicts_with = "file")]
    body: Option<String>,
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long = "label")]
    labels: Vec<String>,
    #[command(flatten)]
    fields: SharedFields,
    #[arg(long)]
    reason: Option<String>,
    #[arg(long)]
    idempotency_key: Option<String>,
}
#[derive(Args)]
struct UpdateOptions {
    #[command(flatten)]
    change: ChangeOptions,
    #[arg(long)]
    title: Option<String>,
    #[arg(long, conflicts_with = "file")]
    body: Option<String>,
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long = "label")]
    labels: Option<Vec<String>>,
    #[command(flatten)]
    shared: SharedPatch,
}
#[derive(Args)]
struct VerifyOptions {
    #[arg(long, default_value = "verified", value_parser = ["verified", "disputed"])]
    verification: String,
    #[arg(long)]
    evidence_file: PathBuf,
    #[command(flatten)]
    change: ChangeOptions,
}
#[derive(Args)]
struct RetireOptions {
    #[command(flatten)]
    change: ChangeOptions,
    /// Existing successor record, with a supersedes edge from successor to this record.
    #[arg(long)]
    successor: Option<Uuid>,
    /// Atomically add a new successor→record supersedes edge before retiring.
    #[arg(long, requires = "successor")]
    link_successor: bool,
}
#[derive(Args)]
struct ShowTask {
    id: Uuid,
    #[arg(long)]
    context: bool,
    #[arg(long, default_value_t = 1)]
    depth: u32,
    #[arg(long, default_value_t = 50)]
    limit: u32,
    #[arg(long, default_value_t = 262144)]
    max_bytes: u32,
    #[arg(long, default_value = "both", value_parser = ["both", "incoming", "outgoing"])]
    direction: String,
}

#[derive(Args)]
struct RestoreOptions {
    id: Uuid,
    #[arg(long)]
    revision: i64,
    /// Restore historical lifecycle and tombstone state too.
    #[arg(long)]
    include_lifecycle: bool,
    #[command(flatten)]
    change: ChangeOptions,
}

#[derive(Args)]
struct CreateTask {
    #[command(flatten)]
    options: CreateOptions,
    #[arg(long, default_value = "")]
    acceptance_criteria: String,
    #[arg(long, default_value = "draft", value_parser = ["draft", "active"])]
    state: String,
}

#[derive(Args)]
struct UpdateTask {
    id: Uuid,
    #[command(flatten)]
    options: UpdateOptions,
    #[arg(long)]
    acceptance_criteria: Option<String>,
}

#[derive(Args)]
struct TaskTree {
    id: Uuid,
    #[arg(long, default_value_t = 2)]
    depth: u32,
    #[arg(long, default_value_t = 50)]
    limit: u32,
    #[arg(long, default_value_t = 262144)]
    max_bytes: u32,
}

#[derive(Args)]
struct ParentOptions {
    id: Uuid,
    #[arg(long, required_unless_present = "root", conflicts_with = "root")]
    parent: Option<Uuid>,
    #[arg(long)]
    root: bool,
    #[command(flatten)]
    change: ChangeOptions,
}

#[derive(Subcommand)]
enum TaskCommand {
    Create(CreateTask),
    Show(ShowTask),
    List {
        #[command(flatten)]
        page: Paging,
        #[command(flatten)]
        filters: DateFilters,
    },
    Ready {
        #[command(flatten)]
        page: Paging,
    },
    /// Retrieve structural children within record, depth and response-byte bounds.
    Tree(TaskTree),
    /// Atomically reparent a child or detach it to the root.
    SetParent(ParentOptions),
    Update(UpdateTask),
    Verify {
        id: Uuid,
        #[command(flatten)]
        options: VerifyOptions,
    },
    Retire {
        id: Uuid,
        #[command(flatten)]
        options: RetireOptions,
    },
    Activate {
        id: Uuid,
        #[command(flatten)]
        change: ChangeOptions,
    },
    Complete {
        id: Uuid,
        #[command(flatten)]
        change: ChangeOptions,
    },
    Reopen {
        id: Uuid,
        #[command(flatten)]
        change: ChangeOptions,
    },
    Cancel {
        id: Uuid,
        #[command(flatten)]
        change: ChangeOptions,
    },
    Delete {
        id: Uuid,
        #[command(flatten)]
        change: ChangeOptions,
    },
    Restore(RestoreOptions),
}
#[derive(Subcommand)]
enum KnowledgeCommand {
    Create {
        #[command(flatten)]
        options: CreateOptions,
    },
    Show {
        id: Uuid,
    },
    List {
        #[command(flatten)]
        page: Paging,
        #[command(flatten)]
        filters: DateFilters,
    },
    Update {
        id: Uuid,
        #[command(flatten)]
        options: UpdateOptions,
    },
    Verify {
        id: Uuid,
        #[command(flatten)]
        options: VerifyOptions,
    },
    Retire {
        id: Uuid,
        #[command(flatten)]
        options: RetireOptions,
    },
    Restore(RestoreOptions),
}
#[derive(Subcommand)]
enum LinkCommand {
    /// A blocks B means B depends on A. Revision pins never fall back to current content.
    Add {
        source: Uuid,
        target: Uuid,
        #[arg(long = "type", value_parser = ["blocks", "parent-child", "references", "motivated_by", "produces", "supports", "contradicts", "supersedes"])]
        edge_type: String,
        #[arg(long)]
        source_revision: Option<i64>,
        #[arg(long)]
        target_revision: Option<i64>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Remove {
        id: Uuid,
        #[command(flatten)]
        change: ChangeOptions,
    },
}
#[derive(Subcommand)]
enum DraftCommand {
    /// Read a DraftRequest JSON object, including name, operations and idempotency_key.
    Create {
        #[arg(long)]
        file: PathBuf,
    },
    #[command(alias = "show")]
    Inspect {
        id: Uuid,
    },
    Validate {
        id: Uuid,
    },
    Apply {
        id: Uuid,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
}
#[derive(Deserialize, Serialize, Default)]
struct Config {
    endpoint: String,
    workspace: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_file: Option<PathBuf>,
}
struct Client {
    http: reqwest::Client,
    endpoint: String,
    workspace: Uuid,
    token: String,
}
#[derive(Debug)]
struct Failure {
    code: String,
    message: String,
    exit: i32,
    idempotency_key: Option<String>,
}
type Result<T> = std::result::Result<T, Failure>;
fn fail(code: &str, message: impl Into<String>, exit: i32) -> Failure {
    Failure {
        code: code.into(),
        message: message.into(),
        exit,
        idempotency_key: None,
    }
}
fn local_error(e: impl std::fmt::Display) -> Failure {
    fail("local_error", e.to_string(), 1)
}
fn invalid(e: impl std::fmt::Display) -> Failure {
    fail("validation", e.to_string(), 2)
}
fn parse_timestamp(value: &str) -> std::result::Result<String, String> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|date| date.to_rfc3339())
        .map_err(|_| "expected an RFC3339 timestamp with timezone".into())
}
fn migration_failure(error: aq_graph::domain::Error) -> Failure {
    let message = match error {
        aq_graph::domain::Error::Internal(detail)
            if detail.contains("graph contract migration refused") =>
        {
            "migration 0004 refused existing graph data: inspect active endpoint kinds, multiple parents, hierarchy depth and combined cycles; reconcile explicitly on a restored copy before upgrading"
        }
        aq_graph::domain::Error::Internal(detail)
            if detail.contains("knowledge compatibility preflight failed") =>
        {
            "migration 0005 requires explicit legacy knowledge reconciliation: verified/disputed records need recorded verification evidence and actor attribution, and retired records need reasons; follow the upgrade runbook before retrying"
        }
        _ => {
            "database migration failed; verify schema/release compatibility, migration checksums and database availability using the operations runbook"
        }
    };
    fail("migration", message, 1)
}
fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_str(&read_text(path)?).map_err(invalid)
}
fn read_text(path: &Path) -> Result<String> {
    if path == Path::new("-") {
        let mut text = String::new();
        io::stdin().read_to_string(&mut text).map_err(local_error)?;
        Ok(text)
    } else {
        fs::read_to_string(path)
            .map_err(|e| local_error(format!("cannot read {}: {e}", path.display())))
    }
}
fn config_path(cli: &Cli) -> Result<PathBuf> {
    if let Some(path) = &cli.config {
        return Ok(path.clone());
    }
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .ok_or_else(|| invalid("set --config or HOME"))?;
    Ok(root.join("aq-graph/config.json"))
}
fn load_config(cli: &Cli) -> Result<Config> {
    let path = config_path(cli).ok();
    let existing = path.as_ref().filter(|p| p.exists());
    let mut config: Config = match existing {
        Some(path) => serde_json::from_str(&read_text(path)?).map_err(invalid)?,
        None => default_config(cli)?,
    };
    if let Some(endpoint) = &cli.endpoint {
        config.endpoint = endpoint.clone();
    }
    if let Some(workspace) = cli.workspace {
        config.workspace = workspace;
    }
    if let Some(token_file) = &cli.token_file {
        config.token_file = Some(token_file.clone());
    }
    validate_endpoint(&config.endpoint)?;
    Ok(config)
}
fn default_config(cli: &Cli) -> Result<Config> {
    if !matches!(cli.command, Command::Init { .. })
        && let Some(path) = &cli.config
    {
        return Err(local_error(format!(
            "configuration file {} does not exist",
            path.display()
        )));
    }
    Ok(Config {
        endpoint: "http://127.0.0.1:8741".into(),
        workspace: Uuid::parse_str(LOCAL_WORKSPACE).unwrap(),
        token_file: None,
    })
}
fn validate_endpoint(endpoint: &str) -> Result<()> {
    let url = reqwest::Url::parse(endpoint).map_err(invalid)?;
    let http_host = matches!(url.scheme(), "http" | "https") && url.host_str().is_some();
    let credentials = !url.username().is_empty() || url.password().is_some();
    let extra_suffix = url.query().is_some() || url.fragment().is_some();
    let plain_authority = http_host && !credentials;
    if !plain_authority || extra_suffix {
        return Err(invalid(
            "endpoint must be an HTTP(S) URL without embedded credentials, query or fragment",
        ));
    }
    Ok(())
}
fn secret_file(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path).map_err(local_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(invalid(format!(
                "credential file {} must have owner-only permissions (chmod 600)",
                path.display()
            )));
        }
    }
    if !metadata.is_file() {
        return Err(invalid("credential source must be a regular file"));
    }
    let token = fs::read_to_string(path)
        .map_err(local_error)?
        .trim()
        .to_owned();
    if token.is_empty() {
        return Err(invalid("credential source is empty"));
    }
    Ok(token)
}
fn token(file: Option<&Path>, variable: &str) -> Result<String> {
    if let Some(path) = file {
        secret_file(path)
    } else {
        std::env::var(variable)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_owned())
            .ok_or_else(|| {
                fail(
                    "unauthorized",
                    format!("set {variable} or configure a credential file"),
                    5,
                )
            })
    }
}
fn private_write(path: &Path, bytes: &[u8], overwrite: bool) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(local_error)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true);
    if overwrite {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(local_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(local_error)?;
    }
    use std::io::Write;
    file.write_all(bytes).map_err(local_error)?;
    file.sync_all().map_err(local_error)
}
impl Client {
    fn new(config: Config) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(local_error)?,
            token: token(config.token_file.as_deref(), "AQ_GRAPH_TOKEN")?,
            endpoint: config.endpoint.trim_end_matches('/').into(),
            workspace: config.workspace,
        })
    }
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}/v1/workspaces/{}{}", self.endpoint, self.workspace, path);
        self.http
            .request(method, url)
            .bearer_auth(&self.token)
            .header("x-aq-graph-api-version", "1")
    }
    async fn send(request: reqwest::RequestBuilder) -> Result<Value> {
        let response = request.send().await.map_err(|e| {
            fail(
                if e.is_timeout() {
                    "timeout"
                } else {
                    "unavailable"
                },
                "service request failed; check endpoint and service health",
                7,
            )
        })?;
        let status = response.status();
        if status.is_success()
            && response
                .headers()
                .get("x-aq-graph-api-version")
                .is_none_or(|v| v != "1")
        {
            return Err(fail(
                "protocol",
                "service API version is missing or incompatible; this client requires version 1",
                7,
            ));
        }
        let value: Value = response
            .json()
            .await
            .map_err(|_| fail("protocol", "service returned a non-JSON response", 7))?;
        if !status.is_success() {
            let exit = match status.as_u16() {
                400 | 422 => 2,
                404 => 3,
                409 => 4,
                401 => 5,
                403 => 6,
                429 | 503 | 504 => 7,
                _ => 1,
            };
            return Err(fail(
                value["error"]["code"].as_str().unwrap_or("service_error"),
                value["error"]["message"]
                    .as_str()
                    .unwrap_or("service rejected request"),
                exit,
            ));
        }
        Ok(value)
    }
    async fn get(&self, path: &str, query: Vec<(&str, String)>) -> Result<Value> {
        Self::send(self.request(reqwest::Method::GET, path).query(&query)).await
    }
    async fn post(&self, path: &str, value: Value) -> Result<Value> {
        let key = value
            .get("idempotency_key")
            .and_then(Value::as_str)
            .map(str::to_owned);
        Self::send(self.request(reqwest::Method::POST, path).json(&value))
            .await
            .map_err(|mut error| {
                error.idempotency_key = key;
                error
            })
    }
    async fn mutation(
        &self,
        operations: Vec<Value>,
        reason: Option<String>,
        key: Option<String>,
    ) -> Result<Value> {
        self.post("/mutations", json!({"idempotency_key": key.unwrap_or_else(|| Uuid::new_v4().to_string()), "reason": reason, "operations": operations})).await
    }
    async fn patch(&self, id: Uuid, change: ChangeOptions, patch: Value) -> Result<Value> {
        self.mutation(vec![json!({"op":"update_node", "id":id, "expected_version":change.expected_version, "patch":patch})], change.reason, change.idempotency_key).await
    }
    async fn verify(&self, id: Uuid, options: VerifyOptions) -> Result<Value> {
        if options
            .change
            .reason
            .as_deref()
            .is_none_or(|v| v.trim().is_empty())
        {
            return Err(invalid("verify requires a nonempty --reason"));
        }
        let evidence = read_json(&options.evidence_file)?;
        if !evidence.is_array() {
            return Err(invalid(
                "evidence file must contain a JSON array of named source descriptors",
            ));
        }
        self.mutation(vec![json!({"op":"verify_node","id":id,"expected_version":options.change.expected_version,"verification":options.verification,"evidence":evidence})],options.change.reason,options.change.idempotency_key).await
    }
    async fn retire(&self, id: Uuid, options: RetireOptions) -> Result<Value> {
        let reason = options
            .change
            .reason
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| invalid("retire requires a nonempty --reason"))?;
        let mut patch = json!({"retired":true,"retirement_reason":reason});
        let mut operations = vec![];
        if let Some(successor) = options.successor {
            if options.link_successor {
                operations.push(json!({"op":"add_edge","source":successor,"target":id,"edge_type":"supersedes"}));
            }
            patch["successor_id"] = json!(successor);
        }
        operations.push(json!({"op":"update_node","id":id,"expected_version":options.change.expected_version,"patch":patch}));
        self.mutation(
            operations,
            options.change.reason,
            options.change.idempotency_key,
        )
        .await
    }
}
fn page_query(page: Paging) -> Vec<(&'static str, String)> {
    let mut q = vec![("limit", page.limit.to_string())];
    if let Some(cursor) = page.cursor {
        q.push(("cursor", cursor));
    }
    q
}
fn date_query(query: &mut Vec<(&'static str, String)>, filters: DateFilters) {
    if let Some(value) = filters.valid_at {
        query.push(("valid_at", value));
    }
    if let Some(value) = filters.review_due_at {
        query.push(("review_due_at", value));
    }
    if filters.include_retired {
        query.push(("include_retired", "true".into()));
    }
}
fn body_text(body: Option<String>, file: Option<PathBuf>) -> Result<String> {
    match file {
        Some(file) => read_text(&file),
        None => Ok(body.unwrap_or_default()),
    }
}
fn create_operation(kind: &str, options: &mut CreateOptions) -> Result<Value> {
    let mut op = json!({"op":"create_node","kind":kind,"title":options.title,"body":body_text(options.body.take(),options.file.take())?,"labels":options.labels,"metadata":{},"provenance":[],"content_type":"note","verification":"unverified"});
    for (key, value) in shared_fields(std::mem::take(&mut options.fields))? {
        op[key] = value;
    }
    Ok(op)
}
fn shared_fields(fields: SharedFields) -> Result<serde_json::Map<String, Value>> {
    let mut values = serde_json::Map::new();
    for (key, value) in [
        ("content_type", fields.content_type),
        ("valid_from", fields.valid_from),
        ("valid_until", fields.valid_until),
        ("review_at", fields.review_at),
        ("verification", fields.verification),
    ] {
        if let Some(value) = value {
            values.insert(key.into(), json!(value));
        }
    }
    if let Some(file) = fields.provenance_file {
        let provenance = read_provenance(&file)?;
        values.insert("provenance".into(), provenance);
    }
    Ok(values)
}
fn read_provenance(path: &Path) -> Result<Value> {
    let provenance = read_json(path)?;
    if !provenance.is_array() {
        return Err(invalid(
            "provenance file must contain a JSON array of source descriptors",
        ));
    }
    Ok(provenance)
}
fn update_patch(options: &mut UpdateOptions) -> Result<serde_json::Map<String, Value>> {
    let mut shared = std::mem::take(&mut options.shared);
    let mut patch = shared_fields(std::mem::take(&mut shared.fields))?;
    if let Some(value) = options.title.take() {
        patch.insert("title".into(), json!(value));
    }
    if options.body.is_some() || options.file.is_some() {
        patch.insert(
            "body".into(),
            json!(body_text(options.body.take(), options.file.take())?),
        );
    }
    if let Some(value) = options.labels.take() {
        patch.insert("labels".into(), json!(value));
    }
    apply_shared_patch(&mut patch, shared, &options.change);
    Ok(patch)
}
fn apply_shared_patch(
    patch: &mut serde_json::Map<String, Value>,
    shared: SharedPatch,
    change: &ChangeOptions,
) {
    for (field, clear) in [
        ("valid_from", shared.clear_valid_from),
        ("valid_until", shared.clear_valid_until),
        ("review_at", shared.clear_review_at),
    ] {
        if clear {
            patch.insert(field.into(), Value::Null);
        }
    }
    if let Some(value) = shared.retired {
        patch.insert("retired".into(), json!(value));
    }
    let default_reason = change
        .reason
        .as_ref()
        .filter(|_| shared.retired == Some(true));
    if let Some(value) = shared.retirement_reason.as_ref().or(default_reason) {
        patch.insert("retirement_reason".into(), json!(value));
    }
}
async fn run(cli: Cli) -> Result<Value> {
    if let Command::Serve(options) = &cli.command {
        return startup::serve(options).await;
    }
    let config = load_config(&cli)?;
    match cli.command {
        Command::Init { local, force } => startup::init(&cli, config, local, force),
        Command::Health => startup::health(config).await,
        command => commands::run(Client::new(config)?, command).await,
    }
}

fn parse_cli() -> Cli {
    let json_requested = std::env::args().any(|arg| arg == "--json");
    match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            if error.exit_code() == 0 || !json_requested {
                let _ = error.print();
            } else {
                eprintln!(
                    "{}",
                    json!({"error":{"code":"validation","message":error.to_string()}})
                );
            }
            std::process::exit(error.exit_code());
        }
    }
}
#[tokio::main]
async fn main() {
    let cli = parse_cli();
    let json_output = cli.json;
    // No request bodies, credential values, URLs or database errors enter logs.
    let _ = tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aq_graph=info".into()),
        )
        .try_init();
    match run(cli).await {
        Ok(value) => println!(
            "{}",
            if json_output {
                serde_json::to_string(&value).unwrap()
            } else {
                serde_json::to_string_pretty(&value).unwrap()
            }
        ),
        Err(error) => {
            report_failure(&error, json_output);
            std::process::exit(error.exit);
        }
    }
}

fn report_failure(error: &Failure, json_output: bool) {
    if json_output {
        let mut detail = json!({"code":error.code,"message":error.message});
        if let Some(key) = &error.idempotency_key {
            detail["idempotency_key"] = json!(key);
        }
        eprintln!("{}", json!({"error":detail}));
    } else {
        eprintln!("{}: {}", error.code, error.message);
        if let Some(key) = &error.idempotency_key {
            eprintln!("Idempotency key: {key}. Retry the identical operation with this key.");
        }
    }
}

#[cfg(test)]
#[path = "cli/tests.rs"]
mod tests;
