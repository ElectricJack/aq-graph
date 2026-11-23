use super::*;

#[derive(Args)]
pub(super) struct ServeOptions {
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
}

async fn connect(pool_size: u32) -> Result<Store> {
    if !(1..=128).contains(&pool_size) {
        return Err(invalid("pool size must be 1..128"));
    }
    let database_url =
        std::env::var("DATABASE_URL").map_err(|_| invalid("DATABASE_URL is required"))?;
    for attempt in 1..=10 {
        match Store::connect(&database_url, pool_size).await {
            Ok(store) => return Ok(store),
            Err(_) if attempt < 10 => tokio::time::sleep(Duration::from_secs(1)).await,
            Err(_) => break,
        }
    }
    Err(fail(
        "unavailable",
        "cannot connect to PostgreSQL after 10 attempts",
        7,
    ))
}

pub(super) async fn serve(options: &ServeOptions) -> Result<Value> {
    let store = connect(options.pool_size).await?;
    store.migrate().await.map_err(migration_failure)?;
    if let Some(workspace) = options.bootstrap_workspace {
        let credential = token(
            options.bootstrap_token_file.as_deref(),
            "AQ_GRAPH_BOOTSTRAP_TOKEN",
        )?;
        store
            .bootstrap(workspace, &options.bootstrap_actor, &credential)
            .await
            .map_err(|_| fail("bootstrap", "workspace bootstrap failed", 1))?;
    } else if options.bootstrap_token_file.is_some()
        || std::env::var_os("AQ_GRAPH_BOOTSTRAP_TOKEN").is_some()
    {
        return Err(invalid(
            "set AQ_GRAPH_BOOTSTRAP_WORKSPACE when providing a bootstrap credential",
        ));
    }
    service::serve(store, options.bind)
        .await
        .map_err(|_| fail("service", "HTTP service stopped with an error", 1))?;
    Ok(json!({"stopped":true}))
}

pub(super) fn init(cli: &Cli, mut config: Config, local: bool, force: bool) -> Result<Value> {
    let path = config_path(cli)?;
    if path.exists() && !force {
        return Err(invalid(
            "configuration already exists; use --force to replace it",
        ));
    }
    if local {
        prepare_local_token(&mut config, &path)?;
    }
    token(config.token_file.as_deref(), "AQ_GRAPH_TOKEN")?;
    if let Some(secret_path) = &config.token_file {
        config.token_file = Some(fs::canonicalize(secret_path).map_err(local_error)?);
    }
    private_write(
        &path,
        &serde_json::to_vec_pretty(&config).map_err(local_error)?,
        force,
    )?;
    Ok(
        json!({"config":path,"endpoint":config.endpoint,"workspace":config.workspace,"token_file":config.token_file,"next":"Start the configured service; see README.md for Docker Compose or native PostgreSQL setup. Init never creates a second database or changes a hosted workspace."}),
    )
}

fn prepare_local_token(config: &mut Config, path: &Path) -> Result<()> {
    if config.token_file.is_none() && std::env::var_os("AQ_GRAPH_TOKEN").is_none() {
        config.token_file = Some(path.parent().unwrap_or(Path::new(".")).join("token"));
    }
    if let Some(secret_path) = &config.token_file
        && !secret_path.exists()
    {
        let credential = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        private_write(secret_path, credential.as_bytes(), false)?;
    }
    Ok(())
}

pub(super) async fn health(config: Config) -> Result<Value> {
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
    response
        .json()
        .await
        .map_err(|_| fail("protocol", "invalid health response", 7))
}
