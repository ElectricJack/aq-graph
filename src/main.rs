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
    Serve {
        #[arg(long, env = "AQ_GRAPH_BIND", default_value = "127.0.0.1:8741")]
        bind: SocketAddr,
        #[arg(long, env = "AQ_GRAPH_POOL_SIZE", default_value_t = 20)]
        pool_size: u32,
        #[arg(long, env = "AQ_GRAPH_BOOTSTRAP_WORKSPACE")]
        bootstrap_workspace: Option<Uuid>,
        #[arg(long, env = "AQ_GRAPH_BOOTSTRAP_ACTOR", default_value = "local-admin")]
        bootstrap_actor: String,
        #[arg(long, env = "AQ_GRAPH_BOOTSTRAP_TOKEN_FILE")]
        bootstrap_token_file: Option<PathBuf>,
    },
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
        #[command(flatten)]
        page: Paging,
    },
    History {
        id: Uuid,
        #[command(flatten)]
        page: Paging,
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
#[derive(Args)]
struct CreateOptions {
    #[arg(long)]
    title: String,
    #[arg(long, conflicts_with = "file")]
    body: Option<String>,
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long, default_value = "")]
    acceptance_criteria: String,
    #[arg(long = "label")]
    labels: Vec<String>,
    #[arg(long)]
    reason: Option<String>,
    #[arg(long)]
    idempotency_key: Option<String>,
}
#[derive(Subcommand)]
enum TaskCommand {
    Create {
        #[command(flatten)]
        options: CreateOptions,
        #[arg(long, default_value = "draft", value_parser = ["draft", "active"])]
        state: String,
    },
    Show {
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
    },
    List {
        #[command(flatten)]
        page: Paging,
    },
    Ready {
        #[command(flatten)]
        page: Paging,
    },
    Update {
        id: Uuid,
        #[command(flatten)]
        change: ChangeOptions,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, conflicts_with = "file")]
        body: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        acceptance_criteria: Option<String>,
        #[arg(long = "label")]
        labels: Option<Vec<String>>,
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
    Restore {
        id: Uuid,
        #[arg(long)]
        revision: i64,
        #[command(flatten)]
        change: ChangeOptions,
    },
}
#[derive(Subcommand)]
enum KnowledgeCommand {
    Create {
        #[command(flatten)]
        options: CreateOptions,
        #[arg(long = "type", default_value = "note", value_parser = ["fact", "decision", "policy", "procedure", "incident", "reference", "note"])]
        knowledge_type: String,
        #[arg(long, default_value = "unverified", value_parser = ["unverified", "verified", "disputed"])]
        verification: String,
        /// JSON array of attributed source references.
        #[arg(long)]
        provenance_file: Option<PathBuf>,
    },
    Show {
        id: Uuid,
    },
    List {
        #[command(flatten)]
        page: Paging,
    },
    Update {
        id: Uuid,
        #[command(flatten)]
        change: ChangeOptions,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, conflicts_with = "file")]
        body: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long, value_parser = ["unverified", "verified", "disputed"])]
        verification: Option<String>,
        #[arg(long)]
        retired: Option<bool>,
        #[arg(long)]
        provenance_file: Option<PathBuf>,
    },
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
    if let Some(path) = &path
        && cli.config.is_some()
        && !path.exists()
        && !matches!(cli.command, Command::Init { .. })
    {
        return Err(local_error(format!(
            "configuration file {} does not exist",
            path.display()
        )));
    }
    let mut config: Config = if let Some(path) = path.filter(|p| p.exists()) {
        serde_json::from_str(&read_text(&path)?).map_err(invalid)?
    } else {
        Config {
            endpoint: "http://127.0.0.1:8741".into(),
            workspace: Uuid::parse_str(LOCAL_WORKSPACE).unwrap(),
            token_file: None,
        }
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
    let url = reqwest::Url::parse(&config.endpoint).map_err(invalid)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "endpoint must be an HTTP(S) URL without embedded credentials, query or fragment",
        ));
    }
    Ok(config)
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
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<Value> {
        let url = format!("{}/v1/workspaces/{}{}", self.endpoint, self.workspace, path);
        let mut request = self
            .http
            .request(method, url)
            .bearer_auth(&self.token)
            .header("x-aq-graph-api-version", "1")
            .query(query);
        if let Some(body) = body {
            request = request.json(&body);
        }
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
        self.request(reqwest::Method::GET, path, &query, None).await
    }
    async fn post(&self, path: &str, value: Value) -> Result<Value> {
        let key = value
            .get("idempotency_key")
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.request(reqwest::Method::POST, path, &[], Some(value))
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
}
fn page_query(page: Paging) -> Vec<(&'static str, String)> {
    let mut q = vec![("limit", page.limit.to_string())];
    if let Some(cursor) = page.cursor {
        q.push(("cursor", cursor));
    }
    q
}
fn body_text(body: Option<String>, file: Option<PathBuf>) -> Result<String> {
    match file {
        Some(file) => read_text(&file),
        None => Ok(body.unwrap_or_default()),
    }
}
fn create_operation(kind: &str, options: &mut CreateOptions) -> Result<Value> {
    Ok(
        json!({"op":"create_node", "kind":kind, "title":options.title, "body":body_text(options.body.take(), options.file.take())?, "acceptance_criteria":options.acceptance_criteria, "labels":options.labels, "metadata":{}, "provenance":[]}),
    )
}
async fn run(cli: Cli) -> Result<Value> {
    if let Command::Serve {
        bind,
        pool_size,
        bootstrap_workspace,
        bootstrap_actor,
        bootstrap_token_file,
    } = &cli.command
    {
        if *pool_size == 0 || *pool_size > 128 {
            return Err(invalid("pool size must be 1..128"));
        }
        let database_url =
            std::env::var("DATABASE_URL").map_err(|_| invalid("DATABASE_URL is required"))?;
        let mut store = None;
        for attempt in 0..10 {
            match Store::connect(&database_url, *pool_size).await {
                Ok(connected) => {
                    store = Some(connected);
                    break;
                }
                Err(_) if attempt < 9 => tokio::time::sleep(Duration::from_secs(1)).await,
                Err(_) => {
                    return Err(fail(
                        "unavailable",
                        "cannot connect to PostgreSQL after 10 attempts",
                        7,
                    ));
                }
            }
        }
        let store = store.unwrap();
        store.migrate().await.map_err(|_| {
            fail(
                "migration",
                "database migration failed; inspect schema compatibility and database availability",
                1,
            )
        })?;
        if let Some(workspace) = bootstrap_workspace {
            let credential = token(bootstrap_token_file.as_deref(), "AQ_GRAPH_BOOTSTRAP_TOKEN")?;
            store
                .bootstrap(*workspace, bootstrap_actor, &credential)
                .await
                .map_err(|_| fail("bootstrap", "workspace bootstrap failed", 1))?;
        } else if bootstrap_token_file.is_some()
            || std::env::var_os("AQ_GRAPH_BOOTSTRAP_TOKEN").is_some()
        {
            return Err(invalid(
                "set AQ_GRAPH_BOOTSTRAP_WORKSPACE when providing a bootstrap credential",
            ));
        }
        service::serve(store, *bind)
            .await
            .map_err(|_| fail("service", "HTTP service stopped with an error", 1))?;
        return Ok(json!({"stopped":true}));
    }
    let mut config = load_config(&cli)?;
    if let Command::Init { local, force } = &cli.command {
        let path = config_path(&cli)?;
        if path.exists() && !force {
            return Err(invalid(
                "configuration already exists; use --force to replace it",
            ));
        }
        if *local && config.token_file.is_none() && std::env::var_os("AQ_GRAPH_TOKEN").is_none() {
            config.token_file = Some(path.parent().unwrap_or(Path::new(".")).join("token"));
        }
        if *local
            && let Some(secret_path) = &config.token_file
            && !secret_path.exists()
        {
            let credential = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
            private_write(secret_path, credential.as_bytes(), false)?;
        }
        token(config.token_file.as_deref(), "AQ_GRAPH_TOKEN")?;
        if let Some(secret_path) = &config.token_file {
            config.token_file = Some(fs::canonicalize(secret_path).map_err(local_error)?);
        }
        private_write(
            &path,
            &serde_json::to_vec_pretty(&config).map_err(local_error)?,
            *force,
        )?;
        return Ok(
            json!({"config":path,"endpoint":config.endpoint,"workspace":config.workspace,"token_file":config.token_file,"next":"Start the configured service; see README.md for Docker Compose or native PostgreSQL setup. Init never creates a second database or changes a hosted workspace."}),
        );
    }
    if matches!(cli.command, Command::Health) {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(local_error)?;
        let response = http
            .get(format!(
                "{}/health/ready",
                config.endpoint.trim_end_matches('/')
            ))
            .send()
            .await
            .map_err(|_| fail("unavailable", "health request failed", 7))?;
        if !response.status().is_success() {
            return Err(fail("unavailable", "service is not ready", 7));
        }
        return response
            .json()
            .await
            .map_err(|_| fail("protocol", "invalid health response", 7));
    }
    let client = Client::new(config)?;
    match cli.command {
        Command::Task { command } => match command {
            TaskCommand::Create { mut options, state } => { let mut op = create_operation("task", &mut options)?; op["task_state"] = json!(state); client.mutation(vec![op], options.reason, options.idempotency_key).await },
            TaskCommand::Show { id, context, depth, limit, max_bytes, direction } => { if context { client.get(&format!("/nodes/{id}/context"), vec![("depth",depth.to_string()),("limit",limit.to_string()),("max_bytes",max_bytes.to_string()),("direction",direction)]).await } else { client.get(&format!("/nodes/{id}"), vec![]).await } },
            TaskCommand::List { page } => { let mut query = page_query(page); query.push(("kind", "task".into())); client.get("/nodes", query).await },
            TaskCommand::Ready { page } => client.get("/ready", page_query(page)).await,
            TaskCommand::Update { id, change, title, body, file, acceptance_criteria, labels } => {
                let mut patch = serde_json::Map::new();
                if let Some(v) = title { patch.insert("title".into(), json!(v)); }
                if body.is_some() || file.is_some() { patch.insert("body".into(), json!(body_text(body,file)?)); }
                if let Some(v) = acceptance_criteria { patch.insert("acceptance_criteria".into(),json!(v)); }
                if let Some(v) = labels { patch.insert("labels".into(),json!(v)); } client.patch(id,change,Value::Object(patch)).await
            },
            TaskCommand::Activate { id, change } | TaskCommand::Reopen { id, change } => client.patch(id,change,json!({"task_state":"active"})).await,
            TaskCommand::Complete { id, change } => client.patch(id,change,json!({"task_state":"completed"})).await,
            TaskCommand::Cancel { id, change } => client.patch(id,change,json!({"task_state":"canceled"})).await,
            TaskCommand::Delete { id, change } => client.mutation(vec![json!({"op":"delete_node","id":id,"expected_version":change.expected_version})],change.reason,change.idempotency_key).await,
            TaskCommand::Restore { id, revision, change } => client.mutation(vec![json!({"op":"restore_node","id":id,"expected_version":change.expected_version,"revision":revision})],change.reason,change.idempotency_key).await,
        },
        Command::Knowledge { command } => match command {
            KnowledgeCommand::Create { mut options, knowledge_type, verification, provenance_file } => { let mut op = create_operation("knowledge", &mut options)?; op["knowledge_type"] = json!(knowledge_type); op["verification"] = json!(verification);
                if let Some(file) = provenance_file { op["provenance"] = read_json(&file)?; } client.mutation(vec![op],options.reason,options.idempotency_key).await },
            KnowledgeCommand::Show { id } => client.get(&format!("/nodes/{id}"),vec![]).await,
            KnowledgeCommand::List { page } => { let mut query = page_query(page); query.push(("kind","knowledge".into())); client.get("/nodes",query).await },
            KnowledgeCommand::Update { id, change, title, body, file, verification, retired, provenance_file } => { let mut patch = serde_json::Map::new();
                if let Some(v) = title { patch.insert("title".into(),json!(v)); }
                if body.is_some() || file.is_some() { patch.insert("body".into(),json!(body_text(body,file)?)); }
                if let Some(v) = verification { patch.insert("verification".into(),json!(v)); }
                if let Some(v) = retired { patch.insert("retired".into(),json!(v)); }
                if let Some(file) = provenance_file { patch.insert("provenance".into(),read_json(&file)?); } client.patch(id,change,Value::Object(patch)).await },
        },
        Command::Link { command } => match command {
            LinkCommand::Add { source,target,edge_type,source_revision,target_revision,reason,idempotency_key } => client.mutation(vec![json!({"op":"add_edge","source":source,"target":target,"edge_type":edge_type,"source_revision":source_revision,"target_revision":target_revision})],reason,idempotency_key).await,
            LinkCommand::Remove { id,change } => client.mutation(vec![json!({"op":"remove_edge","id":id,"expected_version":change.expected_version})],change.reason,change.idempotency_key).await,
        },
        Command::Search { query,page } => { let mut q = page_query(page); q.push(("q",query)); client.get("/search",q).await },
        Command::History { id,page } => client.get(&format!("/nodes/{id}/history"),page_query(page)).await,
        Command::Diff { id,from,to } => client.get(&format!("/nodes/{id}/diff"),vec![("from",from.to_string()),("to",to.to_string())]).await,
        Command::Mutate { file } => client.post("/mutations",read_json(&file)?).await,
        Command::Changeset { command } => match command {
            DraftCommand::Create { file } => client.post("/changesets",read_json(&file)?).await,
            DraftCommand::Inspect { id } => client.get(&format!("/changesets/{id}"),vec![]).await,
            DraftCommand::Validate { id } => client.post(&format!("/changesets/{id}/validate"),json!({})).await,
            DraftCommand::Apply { id,idempotency_key } => client.post(&format!("/changesets/{id}/apply"),json!({"idempotency_key":idempotency_key.unwrap_or_else(|| Uuid::new_v4().to_string())})).await,
        },
        Command::Init { .. } | Command::Serve { .. } | Command::Health => unreachable!(),
    }
}
#[tokio::main]
async fn main() {
    let json_requested = std::env::args().any(|arg| arg == "--json");
    let cli = match Cli::try_parse() {
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
    };
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
            if json_output {
                let mut detail = json!({"code":error.code,"message":error.message});
                if let Some(key) = &error.idempotency_key {
                    detail["idempotency_key"] = json!(key);
                }
                eprintln!("{}", json!({"error":detail}));
            } else {
                eprintln!("{}: {}", error.code, error.message);
                if let Some(key) = error.idempotency_key {
                    eprintln!(
                        "Idempotency key: {key}. Retry the identical operation with this key."
                    );
                }
            }
            std::process::exit(error.exit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        routing::post,
    };
    use std::sync::{Arc, Mutex};

    fn init_cli(config: PathBuf, token: PathBuf) -> Cli {
        Cli {
            json: true,
            config: Some(config),
            endpoint: Some("http://127.0.0.1:8741".into()),
            workspace: Some(Uuid::new_v4()),
            token_file: Some(token),
            command: Command::Init {
                local: true,
                force: false,
            },
        }
    }

    #[tokio::test]
    async fn init_preserves_credentials_and_refuses_config_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.json");
        let token = directory.path().join("token");
        run(init_cli(config.clone(), token.clone())).await.unwrap();
        let original_config = fs::read(&config).unwrap();
        let original_token = fs::read(&token).unwrap();
        assert!(original_token.len() >= 32);
        let result = run(init_cli(config.clone(), token.clone()))
            .await
            .unwrap_err();
        assert_eq!(result.exit, 2);
        assert_eq!(fs::read(&config).unwrap(), original_config);
        assert_eq!(fs::read(&token).unwrap(), original_token);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&config).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&token).unwrap().permissions().mode() & 0o777,
                0o600
            );
            fs::set_permissions(&token, fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(secret_file(&token).unwrap_err().exit, 2);
        }
    }

    type Observed = Arc<Mutex<Vec<(HeaderMap, Value)>>>;
    async fn mock_server(
        status: StatusCode,
        version: Option<&'static str>,
    ) -> (String, Observed, tokio::task::JoinHandle<()>) {
        let observed: Observed = Arc::default();
        let capture = observed.clone();
        let app = Router::new().route(
            "/v1/workspaces/{workspace}/mutations",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let capture = capture.clone();
                async move {
                    capture.lock().unwrap().push((headers, body));
                    let response = Json(if status.is_success() {
                        json!({"change_set_id":Uuid::new_v4(),"results":[]})
                    } else {
                        json!({"error":{"code":"conflict","message":"stale version"}})
                    });
                    let mut headers = HeaderMap::new();
                    if let Some(version) = version {
                        headers.insert("x-aq-graph-api-version", version.parse().unwrap());
                    }
                    (status, headers, response)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (endpoint, observed, handle)
    }

    fn test_client(directory: &Path, endpoint: String) -> Client {
        let token = directory.join("token");
        private_write(&token, b"test-only-credential-0123456789abcdef", false).unwrap();
        Client::new(Config {
            endpoint,
            workspace: Uuid::new_v4(),
            token_file: Some(token),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn client_authenticates_negotiates_version_and_preserves_generated_retry_key() {
        let directory = tempfile::tempdir().unwrap();
        let (endpoint, observed, handle) = mock_server(StatusCode::CONFLICT, Some("1")).await;
        let client = test_client(directory.path(), endpoint);
        let error = client
            .mutation(vec![json!({"op":"example"})], None, None)
            .await
            .unwrap_err();
        assert_eq!(error.code, "conflict");
        assert_eq!(error.exit, 4);
        let captured = observed.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(
            captured[0].0["authorization"],
            "Bearer test-only-credential-0123456789abcdef"
        );
        assert_eq!(captured[0].0["x-aq-graph-api-version"], "1");
        let key = captured[0].1["idempotency_key"].as_str().unwrap();
        assert_eq!(error.idempotency_key.as_deref(), Some(key));
        assert!(Uuid::parse_str(key).is_ok());
        handle.abort();
    }

    #[tokio::test]
    async fn client_rejects_incompatible_success_and_keeps_explicit_retry_key() {
        let directory = tempfile::tempdir().unwrap();
        let (endpoint, _, handle) = mock_server(StatusCode::OK, Some("2")).await;
        let client = test_client(directory.path(), endpoint);
        let error = client
            .mutation(vec![], None, Some("known-retry-key".into()))
            .await
            .unwrap_err();
        assert_eq!(error.code, "protocol");
        assert_eq!(error.exit, 7);
        assert_eq!(error.idempotency_key.as_deref(), Some("known-retry-key"));
        handle.abort();
    }

    #[tokio::test]
    async fn service_statuses_map_to_stable_exit_categories() {
        for (status, expected) in [
            (StatusCode::BAD_REQUEST, 2),
            (StatusCode::NOT_FOUND, 3),
            (StatusCode::CONFLICT, 4),
            (StatusCode::UNAUTHORIZED, 5),
            (StatusCode::FORBIDDEN, 6),
            (StatusCode::SERVICE_UNAVAILABLE, 7),
            (StatusCode::INTERNAL_SERVER_ERROR, 1),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let (endpoint, _, handle) = mock_server(status, Some("1")).await;
            let client = test_client(directory.path(), endpoint);
            let error = client
                .mutation(vec![], None, Some("test-retry-key".into()))
                .await
                .unwrap_err();
            assert_eq!(error.exit, expected, "HTTP {status}");
            assert_eq!(error.idempotency_key.as_deref(), Some("test-retry-key"));
            handle.abort();
        }
    }
}
