use std::{collections::BTreeMap, fs, path::PathBuf, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use reqwest::{
    Method,
    blocking::{Client, Response},
    header::{ACCEPT, CONTENT_TYPE, HeaderMap},
};
use serde_json::{Map, Value, json};

const DEFAULT_BASE_URL: &str = "https://rest.budgetbakers.com/wallet";
const DEFAULT_TIMEOUT_SECONDS: u64 = 30;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    if matches!(cli.command, Commands::Schema { .. }) {
        let value = cli.command.execute_schema()?;
        write_output(&value, cli.output)?;
        return Ok(());
    }

    let client = WalletClient::new(
        cli.base_url,
        cli.token,
        Duration::from_secs(cli.timeout_seconds),
    )?;

    let value = match cli.command {
        Commands::Schema { .. } => unreachable!("schema handled before client creation"),
        Commands::List(args) => execute_list(&client, args, cli.include_headers)?,
        Commands::Create(args) => execute_create(&client, args, cli.include_headers)?,
        Commands::Patch(args) => execute_patch(&client, args, cli.include_headers)?,
        Commands::Delete(args) => execute_delete(&client, args, cli.include_headers)?,
        Commands::References(args) => execute_references(&client, args, cli.include_headers)?,
        Commands::Usage(args) => execute_usage(&client, args, cli.include_headers)?,
        Commands::Request(args) => execute_request(&client, args, cli.include_headers)?,
    };

    write_output(&value, cli.output)
}

#[derive(Debug, Parser)]
#[command(
    name = "wallet",
    version,
    about = "Agent-ready CLI for the BudgetBakers Wallet REST API"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(
        long,
        env = "WALLET_API_TOKEN",
        hide_env_values = true,
        global = true,
        help = "Wallet API bearer token. Also read from WALLET_API_TOKEN."
    )]
    token: Option<String>,

    #[arg(
        long,
        env = "WALLET_API_BASE_URL",
        default_value = DEFAULT_BASE_URL,
        global = true,
        help = "Wallet API base URL."
    )]
    base_url: String,

    #[arg(
        long,
        value_enum,
        default_value_t = OutputFormat::Pretty,
        global = true,
        help = "Output format. Use json or ndjson for automation."
    )]
    output: OutputFormat,

    #[arg(
        long,
        global = true,
        help = "Wrap HTTP responses with status and selected response headers."
    )]
    include_headers: bool,

    #[arg(
        long,
        default_value_t = DEFAULT_TIMEOUT_SECONDS,
        global = true,
        help = "HTTP timeout in seconds."
    )]
    timeout_seconds: u64,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(about = "List a Wallet resource with pagination and filters.")]
    List(ListArgs),
    #[command(about = "Create records, accounts, budgets, or custom categories from raw JSON.")]
    Create(CreateArgs),
    #[command(about = "Patch records, accounts, or categories from raw JSON.")]
    Patch(PatchArgs),
    #[command(about = "Delete deletable Wallet entities by ID.")]
    Delete(DeleteArgs),
    #[command(
        visible_alias = "refs",
        about = "Check entity references before deletion."
    )]
    References(ReferencesArgs),
    #[command(about = "Fetch API usage statistics.")]
    Usage(UsageArgs),
    #[command(about = "Make a raw request to /v1/api for endpoints not yet typed.")]
    Request(RequestArgs),
    #[command(about = "Print the machine-readable CLI schema.")]
    Schema {
        #[arg(help = "Optional command name to describe.")]
        command: Option<String>,
    },
}

impl Commands {
    fn execute_schema(&self) -> Result<Value> {
        let command = match self {
            Commands::Schema { command } => command.as_deref(),
            _ => None,
        };
        schema(command)
    }
}

#[derive(Debug, Args)]
struct ListArgs {
    #[arg(value_enum)]
    resource: ListResource,

    #[command(flatten)]
    pagination: PaginationArgs,

    #[arg(
        long = "filter",
        value_name = "NAME=VALUE",
        help = "Repeatable query filter. Example: --filter recordDate=gte.2026-01-01"
    )]
    filters: Vec<String>,

    #[arg(
        long,
        value_name = "CSV",
        help = "Project item fields locally before output."
    )]
    fields: Option<String>,
}

#[derive(Debug, Args)]
struct CreateArgs {
    #[arg(value_enum)]
    resource: CreateResource,

    #[command(flatten)]
    payload: JsonPayloadArgs,

    #[arg(
        long,
        help = "Validate locally and print the request without sending it."
    )]
    dry_run: bool,

    #[arg(long, help = "Ask the API to include agentHints=true.")]
    agent_hints: bool,
}

#[derive(Debug, Args)]
struct PatchArgs {
    #[arg(value_enum)]
    resource: PatchResource,

    #[command(flatten)]
    payload: JsonPayloadArgs,

    #[arg(
        long,
        help = "Validate locally and print the request without sending it."
    )]
    dry_run: bool,

    #[arg(long, help = "Use validation=strict when supported by the endpoint.")]
    strict: bool,

    #[arg(long, help = "Ask the API to include agentHints=true.")]
    agent_hints: bool,
}

#[derive(Debug, Args)]
struct DeleteArgs {
    #[arg(value_enum)]
    resource: DeleteResource,

    #[arg(
        long = "id",
        required = true,
        help = "Entity ID. Repeat for up to 10 IDs."
    )]
    ids: Vec<String>,

    #[arg(
        long,
        help = "Validate locally and print the request without sending it."
    )]
    dry_run: bool,

    #[arg(long, help = "Ask the API to include agentHints=true.")]
    agent_hints: bool,
}

#[derive(Debug, Args)]
struct ReferencesArgs {
    #[arg(value_enum)]
    resource: ReferenceResource,

    #[arg(
        long = "id",
        required = true,
        help = "Entity ID. Repeat for up to 10 IDs."
    )]
    ids: Vec<String>,
}

#[derive(Debug, Args)]
struct UsageArgs {
    #[arg(long, help = "API-defined period such as 30days or 4weeks.")]
    period: Option<String>,

    #[arg(
        long,
        help = "API-defined granularity such as daily, weekly, or monthly."
    )]
    granularity: Option<String>,
}

#[derive(Debug, Args)]
struct RequestArgs {
    #[arg(value_enum)]
    method: HttpMethod,

    #[arg(help = "Path under /v1/api, without query string. Example: /v1/api/records/by-id")]
    path: String,

    #[arg(
        long = "param",
        value_name = "NAME=VALUE",
        help = "Repeatable query parameter. Values are URL-encoded by the HTTP client."
    )]
    params: Vec<String>,

    #[command(flatten)]
    payload: JsonPayloadArgs,

    #[arg(
        long,
        help = "Validate locally and print the request without sending it."
    )]
    dry_run: bool,

    #[arg(
        long,
        value_name = "CSV",
        help = "Project top-level fields locally before output."
    )]
    fields: Option<String>,
}

#[derive(Debug, Args)]
struct PaginationArgs {
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..=200))]
    limit: Option<u16>,

    #[arg(long, value_parser = clap::value_parser!(u64).range(0..))]
    offset: Option<u64>,

    #[arg(
        long,
        help = "Follow nextOffset until the API stops returning more pages."
    )]
    all_pages: bool,

    #[arg(long, help = "Ask the API to include agentHints=true.")]
    agent_hints: bool,

    #[arg(long, help = "Ask the API to include total counts where supported.")]
    with_total: bool,
}

#[derive(Debug, Args)]
struct JsonPayloadArgs {
    #[arg(
        long = "json",
        conflicts_with = "json_file",
        help = "Raw JSON request body."
    )]
    json: Option<String>,

    #[arg(
        long = "json-file",
        value_name = "PATH",
        conflicts_with = "json",
        help = "Read raw JSON request body from a file."
    )]
    json_file: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum OutputFormat {
    Pretty,
    Json,
    Ndjson,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum ListResource {
    Records,
    Accounts,
    Categories,
    Budgets,
    Goals,
    Labels,
    RecordRules,
    StandingOrders,
    StandingOrderItems,
}

impl ListResource {
    fn endpoint(self) -> ResourceEndpoint {
        match self {
            Self::Records => ResourceEndpoint::new("/v1/api/records", "records"),
            Self::Accounts => ResourceEndpoint::new("/v1/api/accounts", "accounts"),
            Self::Categories => ResourceEndpoint::new("/v1/api/categories", "categories"),
            Self::Budgets => ResourceEndpoint::new("/v1/api/budgets", "budgets"),
            Self::Goals => ResourceEndpoint::new("/v1/api/goals", "goals"),
            Self::Labels => ResourceEndpoint::new("/v1/api/labels", "labels"),
            Self::RecordRules => ResourceEndpoint::new("/v1/api/record-rules", "recordRules"),
            Self::StandingOrders => {
                ResourceEndpoint::new("/v1/api/standing-orders", "standingOrders")
            }
            Self::StandingOrderItems => {
                ResourceEndpoint::new("/v1/api/standing-orders/items", "standingOrderItems")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum CreateResource {
    Records,
    Accounts,
    Budgets,
    CustomCategory,
}

impl CreateResource {
    fn endpoint(self) -> &'static str {
        match self {
            Self::Records => "/v1/api/records",
            Self::Accounts => "/v1/api/accounts",
            Self::Budgets => "/v1/api/budgets",
            Self::CustomCategory => "/v1/api/categories/custom",
        }
    }

    fn max_items(self) -> Option<usize> {
        match self {
            Self::Records => Some(20),
            Self::Accounts | Self::Budgets | Self::CustomCategory => None,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum PatchResource {
    Records,
    Accounts,
    Categories,
}

impl PatchResource {
    fn endpoint(self) -> &'static str {
        match self {
            Self::Records => "/v1/api/records",
            Self::Accounts => "/v1/api/accounts",
            Self::Categories => "/v1/api/categories",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum DeleteResource {
    Records,
    Budgets,
    StandingOrders,
    RecordRules,
    Categories,
}

impl DeleteResource {
    fn path_type(self) -> &'static str {
        match self {
            Self::Records => "records",
            Self::Budgets => "budgets",
            Self::StandingOrders => "standing-orders",
            Self::RecordRules => "record-rules",
            Self::Categories => "categories",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum ReferenceResource {
    Records,
    Categories,
}

impl ReferenceResource {
    fn path_type(self) -> &'static str {
        match self {
            Self::Records => "records",
            Self::Categories => "categories",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum HttpMethod {
    Get,
    Post,
    Patch,
    Delete,
}

impl From<HttpMethod> for Method {
    fn from(value: HttpMethod) -> Self {
        match value {
            HttpMethod::Get => Method::GET,
            HttpMethod::Post => Method::POST,
            HttpMethod::Patch => Method::PATCH,
            HttpMethod::Delete => Method::DELETE,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ResourceEndpoint {
    path: &'static str,
    array_key: &'static str,
}

impl ResourceEndpoint {
    const fn new(path: &'static str, array_key: &'static str) -> Self {
        Self { path, array_key }
    }
}

#[derive(Debug)]
struct WalletClient {
    base_url: String,
    token: Option<String>,
    client: Client,
}

impl WalletClient {
    fn new(base_url: String, token: Option<String>, timeout: Duration) -> Result<Self> {
        validate_base_url(&base_url)?;
        if let Some(token) = token.as_deref() {
            reject_control_chars(token, "token")?;
            if token.trim().is_empty() {
                bail!("token must not be empty");
            }
        }

        let client = Client::builder()
            .timeout(timeout)
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            token,
            client,
        })
    }

    fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        body: Option<Value>,
        include_headers: bool,
    ) -> Result<Value> {
        let token = self
            .token
            .as_deref()
            .ok_or_else(|| anyhow!("missing --token or WALLET_API_TOKEN"))?;
        validate_api_path(path)?;

        let url = self.url_for(path);
        let mut request = self
            .client
            .request(method, &url)
            .bearer_auth(token)
            .header(ACCEPT, "application/json")
            .query(query);

        if let Some(body) = body {
            request = request.header(CONTENT_TYPE, "application/json").json(&body);
        }

        let response = request
            .send()
            .with_context(|| format!("request failed for {url}"))?;
        response_to_json(response, include_headers)
    }

    fn dry_run(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        body: Option<Value>,
    ) -> Result<Value> {
        validate_api_path(path)?;
        Ok(json!({
            "dryRun": true,
            "request": {
                "method": method.as_str(),
                "url": self.url_for(path),
                "query": query_as_object(query),
                "headers": {
                    "Authorization": "Bearer ***",
                    "Accept": "application/json",
                    "Content-Type": body.as_ref().map(|_| "application/json")
                },
                "body": body
            }
        }))
    }

    fn url_for(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

fn execute_list(client: &WalletClient, args: ListArgs, include_headers: bool) -> Result<Value> {
    let endpoint = args.resource.endpoint();
    let fields = parse_fields(args.fields.as_deref())?;
    let mut query = pagination_query(&args.pagination);
    query.extend(parse_key_values(&args.filters, "filter")?);

    let value = if args.pagination.all_pages {
        list_all_pages(client, endpoint, query, include_headers)?
    } else {
        client.send(Method::GET, endpoint.path, &query, None, include_headers)?
    };

    if let Some(fields) = fields {
        project_collection(value, endpoint.array_key, &fields)
    } else {
        Ok(value)
    }
}

fn execute_create(client: &WalletClient, args: CreateArgs, include_headers: bool) -> Result<Value> {
    let mut body = read_json_payload(&args.payload)?.ok_or_else(|| {
        anyhow!("create requires --json or --json-file so the request body maps to the API schema")
    })?;
    reject_control_chars_in_json(&body, "$")?;
    validate_create_payload_shape(args.resource, &mut body)?;

    let mut query = Vec::new();
    if args.agent_hints {
        query.push(("agentHints".to_owned(), "true".to_owned()));
    }

    if args.dry_run {
        client.dry_run(Method::POST, args.resource.endpoint(), &query, Some(body))
    } else {
        client.send(
            Method::POST,
            args.resource.endpoint(),
            &query,
            Some(body),
            include_headers,
        )
    }
}

fn execute_patch(client: &WalletClient, args: PatchArgs, include_headers: bool) -> Result<Value> {
    let body = read_json_payload(&args.payload)?.ok_or_else(|| {
        anyhow!("patch requires --json or --json-file so the request body maps to the API schema")
    })?;
    reject_control_chars_in_json(&body, "$")?;
    ensure_array_len(&body, 1, 10, "patch body")?;

    let mut query = Vec::new();
    if args.strict {
        query.push(("validation".to_owned(), "strict".to_owned()));
    }
    if args.agent_hints {
        query.push(("agentHints".to_owned(), "true".to_owned()));
    }

    if args.dry_run {
        client.dry_run(Method::PATCH, args.resource.endpoint(), &query, Some(body))
    } else {
        client.send(
            Method::PATCH,
            args.resource.endpoint(),
            &query,
            Some(body),
            include_headers,
        )
    }
}

fn execute_delete(client: &WalletClient, args: DeleteArgs, include_headers: bool) -> Result<Value> {
    validate_ids(&args.ids, 1, 10)?;
    let body = json!({ "ids": args.ids });
    let path = format!("/v1/api/{}", args.resource.path_type());

    let mut query = Vec::new();
    if args.agent_hints {
        query.push(("agentHints".to_owned(), "true".to_owned()));
    }

    if args.dry_run {
        client.dry_run(Method::DELETE, &path, &query, Some(body))
    } else {
        client.send(Method::DELETE, &path, &query, Some(body), include_headers)
    }
}

fn execute_references(
    client: &WalletClient,
    args: ReferencesArgs,
    include_headers: bool,
) -> Result<Value> {
    validate_ids(&args.ids, 1, 10)?;
    let path = format!("/v1/api/{}/references", args.resource.path_type());
    let query = vec![("id".to_owned(), args.ids.join(","))];
    client.send(Method::GET, &path, &query, None, include_headers)
}

fn execute_usage(client: &WalletClient, args: UsageArgs, include_headers: bool) -> Result<Value> {
    let mut query = Vec::new();
    if let Some(period) = args.period {
        reject_control_chars(&period, "period")?;
        query.push(("period".to_owned(), period));
    }
    if let Some(granularity) = args.granularity {
        reject_control_chars(&granularity, "granularity")?;
        query.push(("granularity".to_owned(), granularity));
    }

    client.send(
        Method::GET,
        "/v1/api/api-usage/stats",
        &query,
        None,
        include_headers,
    )
}

fn execute_request(
    client: &WalletClient,
    args: RequestArgs,
    include_headers: bool,
) -> Result<Value> {
    validate_api_path(&args.path)?;
    let method = Method::from(args.method);
    let query = parse_key_values(&args.params, "param")?;
    let body = read_json_payload(&args.payload)?;
    if let Some(body) = body.as_ref() {
        reject_control_chars_in_json(body, "$")?;
    }

    let value = if args.dry_run {
        client.dry_run(method, &args.path, &query, body)?
    } else {
        client.send(method, &args.path, &query, body, include_headers)?
    };

    if let Some(fields) = parse_fields(args.fields.as_deref())? {
        project_object(value, &fields)
    } else {
        Ok(value)
    }
}

fn list_all_pages(
    client: &WalletClient,
    endpoint: ResourceEndpoint,
    mut query: Vec<(String, String)>,
    include_headers: bool,
) -> Result<Value> {
    upsert_query(&mut query, "limit", "200");
    let mut next_offset = query
        .iter()
        .find(|(key, _)| key == "offset")
        .and_then(|(_, value)| value.parse::<u64>().ok())
        .unwrap_or(0);
    let mut merged_items = Vec::new();
    let mut pages = 0_u64;

    let last_body = loop {
        upsert_query(&mut query, "offset", &next_offset.to_string());
        let response = client.send(Method::GET, endpoint.path, &query, None, include_headers)?;
        let body = unwrap_body(response);
        pages += 1;

        if let Some(items) = body.get(endpoint.array_key).and_then(Value::as_array) {
            merged_items.extend(items.iter().cloned());
        } else {
            bail!(
                "response did not contain expected array key '{}'",
                endpoint.array_key
            );
        }

        let upcoming = body.get("nextOffset").and_then(Value::as_u64);

        let Some(offset) = upcoming else {
            break body;
        };
        if offset == next_offset {
            bail!("API returned the same nextOffset twice; refusing infinite pagination loop");
        }
        next_offset = offset;
    };

    let mut object = match last_body {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    object.insert(endpoint.array_key.to_owned(), Value::Array(merged_items));
    object.insert("pagesFetched".to_owned(), Value::from(pages));
    object.remove("nextOffset");
    Ok(Value::Object(object))
}

fn response_to_json(response: Response, include_headers: bool) -> Result<Value> {
    let status = response.status();
    let headers = selected_headers(response.headers());
    let text = response.text().context("failed to read response body")?;
    let body = if text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text)
            .with_context(|| format!("response was not valid JSON: {}", truncate(&text, 300)))?
    };

    if !status.is_success() {
        bail!("Wallet API returned HTTP {}: {}", status.as_u16(), body);
    }

    if include_headers {
        Ok(json!({
            "status": status.as_u16(),
            "headers": headers,
            "body": body,
        }))
    } else {
        Ok(body)
    }
}

fn selected_headers(headers: &HeaderMap) -> BTreeMap<String, String> {
    [
        "x-ratelimit-limit",
        "x-ratelimit-remaining",
        "retry-after",
        "x-last-data-change-at",
        "x-last-data-change-rev",
        "x-sync-in-progress",
        "x-correlation-id",
    ]
    .into_iter()
    .filter_map(|name| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(|value| (name.to_owned(), value.to_owned()))
    })
    .collect()
}

fn read_json_payload(args: &JsonPayloadArgs) -> Result<Option<Value>> {
    let Some(raw) = read_raw_json(args)? else {
        return Ok(None);
    };
    let value = serde_json::from_str(&raw).context("request body is not valid JSON")?;
    Ok(Some(value))
}

fn read_raw_json(args: &JsonPayloadArgs) -> Result<Option<String>> {
    if let Some(raw) = args.json.as_deref() {
        reject_control_chars(raw, "json")?;
        return Ok(Some(raw.to_owned()));
    }

    let Some(path) = args.json_file.as_deref() else {
        return Ok(None);
    };
    let canonical = path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", path.display()))?;
    let metadata = fs::metadata(&canonical)
        .with_context(|| format!("failed to inspect {}", canonical.display()))?;
    if !metadata.is_file() {
        bail!("json-file must point to a regular file");
    }
    fs::read_to_string(&canonical)
        .with_context(|| format!("failed to read {}", canonical.display()))
        .map(Some)
}

fn pagination_query(args: &PaginationArgs) -> Vec<(String, String)> {
    let mut query = Vec::new();
    if let Some(limit) = args.limit {
        query.push(("limit".to_owned(), limit.to_string()));
    }
    if let Some(offset) = args.offset {
        query.push(("offset".to_owned(), offset.to_string()));
    }
    if args.agent_hints {
        query.push(("agentHints".to_owned(), "true".to_owned()));
    }
    if args.with_total {
        query.push(("withTotal".to_owned(), "true".to_owned()));
    }
    query
}

fn parse_key_values(values: &[String], label: &str) -> Result<Vec<(String, String)>> {
    values
        .iter()
        .map(|raw| {
            reject_control_chars(raw, label)?;
            let (key, value) = raw
                .split_once('=')
                .ok_or_else(|| anyhow!("{label} must be NAME=VALUE, got '{raw}'"))?;
            validate_query_key(key)?;
            if value.contains('%') {
                bail!("{label} value must not contain pre-encoded '%' bytes");
            }
            Ok((key.to_owned(), value.to_owned()))
        })
        .collect()
}

fn parse_fields(raw: Option<&str>) -> Result<Option<Vec<String>>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    reject_control_chars(raw, "fields")?;
    let fields = raw
        .split(',')
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .map(|field| {
            validate_query_key(field)?;
            Ok(field.to_owned())
        })
        .collect::<Result<Vec<_>>>()?;
    if fields.is_empty() {
        bail!("fields must include at least one field name");
    }
    Ok(Some(fields))
}

fn validate_create_payload_shape(resource: CreateResource, body: &mut Value) -> Result<()> {
    match resource.max_items() {
        Some(max) => ensure_array_len(body, 1, max, "create body"),
        None => {
            if !body.is_object() {
                bail!("create body for this resource must be a JSON object");
            }
            Ok(())
        }
    }
}

fn ensure_array_len(body: &Value, min: usize, max: usize, label: &str) -> Result<()> {
    let Some(items) = body.as_array() else {
        bail!("{label} must be a JSON array");
    };
    if items.len() < min || items.len() > max {
        bail!("{label} must contain between {min} and {max} items");
    }
    Ok(())
}

fn validate_ids(ids: &[String], min: usize, max: usize) -> Result<()> {
    if ids.len() < min || ids.len() > max {
        bail!("expected between {min} and {max} IDs");
    }
    for id in ids {
        validate_id(id)?;
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<()> {
    reject_control_chars(id, "id")?;
    if id.is_empty() || id.len() > 128 {
        bail!("id must be between 1 and 128 characters");
    }
    if id.contains(['?', '#', '/', '%', '&', '=']) {
        bail!("id must be a plain identifier without query fragments or encoded bytes");
    }
    if !id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        bail!("id contains unsupported characters");
    }
    Ok(())
}

fn validate_query_key(key: &str) -> Result<()> {
    if key.is_empty() {
        bail!("query key must not be empty");
    }
    if !key
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        bail!("query key '{key}' contains unsupported characters");
    }
    Ok(())
}

fn validate_api_path(path: &str) -> Result<()> {
    reject_control_chars(path, "path")?;
    if !path.starts_with("/v1/api/") {
        bail!("path must start with /v1/api/");
    }
    if path.contains(['?', '#', '%']) || path.contains("..") || path.contains("//") {
        bail!("path must be an unencoded API path without query strings or traversal");
    }
    if !path
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_'))
    {
        bail!("path contains unsupported characters");
    }
    Ok(())
}

fn validate_base_url(base_url: &str) -> Result<()> {
    reject_control_chars(base_url, "base-url")?;
    if !(base_url.starts_with("https://") || base_url.starts_with("http://localhost")) {
        bail!("base-url must be https:// or http://localhost for local testing");
    }
    if base_url.contains(['?', '#']) {
        bail!("base-url must not contain query strings or fragments");
    }
    Ok(())
}

fn reject_control_chars(value: &str, label: &str) -> Result<()> {
    if value.chars().any(char::is_control) {
        bail!("{label} must not contain control characters");
    }
    Ok(())
}

fn reject_control_chars_in_json(value: &Value, path: &str) -> Result<()> {
    match value {
        Value::String(text) => reject_control_chars(text, path),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                reject_control_chars_in_json(item, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        Value::Object(object) => {
            for (key, item) in object {
                reject_control_chars(key, "json object key")?;
                reject_control_chars_in_json(item, &format!("{path}.{key}"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn project_collection(mut value: Value, array_key: &str, fields: &[String]) -> Result<Value> {
    let body = unwrap_body_mut(&mut value);
    let Some(items) = body.get_mut(array_key).and_then(Value::as_array_mut) else {
        bail!("cannot project fields because response has no '{array_key}' array");
    };
    for item in items {
        *item = project_object(item.take(), fields)?;
    }
    Ok(value)
}

fn project_object(value: Value, fields: &[String]) -> Result<Value> {
    let Value::Object(object) = value else {
        bail!("field projection requires a JSON object");
    };
    let mut projected = Map::new();
    for field in fields {
        if let Some(value) = object.get(field) {
            projected.insert(field.to_owned(), value.clone());
        }
    }
    Ok(Value::Object(projected))
}

fn unwrap_body(value: Value) -> Value {
    match value {
        Value::Object(mut object)
            if object.contains_key("body") && object.contains_key("status") =>
        {
            object.remove("body").unwrap_or(Value::Null)
        }
        value => value,
    }
}

fn unwrap_body_mut(value: &mut Value) -> &mut Value {
    let is_wrapped = value
        .as_object()
        .map(|object| object.contains_key("body") && object.contains_key("status"))
        .unwrap_or(false);
    if is_wrapped {
        value.get_mut("body").expect("checked body key")
    } else {
        value
    }
}

fn upsert_query(query: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some((_, existing)) = query.iter_mut().find(|(candidate, _)| candidate == key) {
        *existing = value.to_owned();
    } else {
        query.push((key.to_owned(), value.to_owned()));
    }
}

fn query_as_object(query: &[(String, String)]) -> Value {
    let mut object = Map::new();
    for (key, value) in query {
        match object.get_mut(key) {
            Some(Value::Array(items)) => items.push(Value::String(value.clone())),
            Some(existing) => {
                let previous = existing.take();
                *existing = Value::Array(vec![previous, Value::String(value.clone())]);
            }
            None => {
                object.insert(key.clone(), Value::String(value.clone()));
            }
        }
    }
    Value::Object(object)
}

fn write_output(value: &Value, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Pretty => {
            println!("{}", serde_json::to_string_pretty(value)?);
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string(value)?);
        }
        OutputFormat::Ndjson => write_ndjson(value)?,
    }
    Ok(())
}

fn write_ndjson(value: &Value) -> Result<()> {
    if let Some(items) = value.as_array() {
        for item in items {
            println!("{}", serde_json::to_string(item)?);
        }
        return Ok(());
    }

    if let Some(object) = value.as_object() {
        for key in [
            "records",
            "accounts",
            "categories",
            "budgets",
            "goals",
            "labels",
            "recordRules",
            "standingOrders",
            "standingOrderItems",
            "results",
        ] {
            if let Some(items) = object.get(key).and_then(Value::as_array) {
                for item in items {
                    println!("{}", serde_json::to_string(item)?);
                }
                return Ok(());
            }
        }
    }

    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

fn schema(command: Option<&str>) -> Result<Value> {
    let schema = json!({
        "name": "wallet",
        "api": {
            "baseUrl": DEFAULT_BASE_URL,
            "auth": "Bearer token via --token or WALLET_API_TOKEN",
            "docs": "https://budgetbakers.com/en/products/wallet/integrations/rest-api/",
            "openapi": "https://rest.budgetbakers.com/wallet/openapi",
            "pagination": {
                "limit": { "default": 30, "max": 200 },
                "offset": 0,
                "nextOffset": "Follow with --all-pages when list commands should fetch every page."
            }
        },
        "agentGuarantees": {
            "machineReadableOutput": ["--output json", "--output ndjson"],
            "rawPayloadPath": ["--json", "--json-file", "request METHOD /v1/api/..."],
            "runtimeIntrospection": "wallet schema [command] --output json",
            "responseMinimization": ["list RESOURCE --fields id,name", "list RESOURCE --limit N", "--output ndjson"],
            "mutationSafety": "create, patch, delete, and request support --dry-run",
            "inputHardening": [
                "rejects control characters",
                "rejects encoded or query-polluted IDs",
                "requires raw request paths under /v1/api without query strings",
                "URL query encoding is performed by the HTTP client"
            ]
        },
        "commands": {
            "list": {
                "resources": ["records", "accounts", "categories", "budgets", "goals", "labels", "record-rules", "standing-orders", "standing-order-items"],
                "flags": ["--limit 1..200", "--offset N", "--all-pages", "--agent-hints", "--with-total", "--filter NAME=VALUE", "--fields CSV"],
                "example": "wallet list records --limit 10 --filter recordDate=gte.2026-01-01 --fields id,amount,recordDate --output json"
            },
            "create": {
                "resources": ["records", "accounts", "budgets", "custom-category"],
                "flags": ["--json JSON", "--json-file PATH", "--dry-run", "--agent-hints"],
                "requestBodies": {
                    "records": "array, 1..20 CreateRecordRequest objects",
                    "accounts": "single CreateAccountRequest object",
                    "budgets": "single CreateBudgetRequest object",
                    "custom-category": "single CreateSubcategoryRequest object"
                }
            },
            "patch": {
                "resources": ["records", "accounts", "categories"],
                "flags": ["--json JSON", "--json-file PATH", "--dry-run", "--strict", "--agent-hints"],
                "requestBody": "array, 1..10 patch items"
            },
            "delete": {
                "resources": ["records", "budgets", "standing-orders", "record-rules", "categories"],
                "flags": ["--id ID", "--dry-run", "--agent-hints"],
                "requestBody": { "ids": ["1..10 IDs"] }
            },
            "references": {
                "resources": ["records", "categories"],
                "flags": ["--id ID"],
                "purpose": "Pre-check references before deleting records or categories."
            },
            "usage": {
                "endpoint": "/v1/api/api-usage/stats",
                "flags": ["--period VALUE", "--granularity VALUE"]
            },
            "request": {
                "purpose": "Lossless escape hatch for any current or future Wallet API endpoint.",
                "shape": "wallet request get /v1/api/records --param limit=10 --output json",
                "flags": ["--param NAME=VALUE", "--json JSON", "--json-file PATH", "--dry-run", "--fields CSV"]
            }
        }
    });

    let Some(command) = command else {
        return Ok(schema);
    };
    validate_query_key(command)?;
    schema
        .get("commands")
        .and_then(|commands| commands.get(command))
        .cloned()
        .ok_or_else(|| anyhow!("unknown schema command '{command}'"))
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_query_polluted_ids() {
        assert!(validate_id("abc?limit=10").is_err());
        assert!(validate_id("abc%2Fdef").is_err());
        assert!(validate_id("5c5c03e8-000a-8000-8000-000000000000").is_ok());
    }

    #[test]
    fn rejects_unsafe_raw_paths() {
        assert!(validate_api_path("/v1/api/records").is_ok());
        assert!(validate_api_path("/v1/api/records?limit=10").is_err());
        assert!(validate_api_path("/v1/api/../accounts").is_err());
        assert!(validate_api_path("/wallet/v1/api/records").is_err());
    }

    #[test]
    fn projects_collection_items_without_dropping_pagination() {
        let value = json!({
            "records": [
                { "id": "one", "amount": -10, "note": "hidden" }
            ],
            "nextOffset": 20
        });
        let projected =
            project_collection(value, "records", &["id".into(), "amount".into()]).unwrap();
        assert_eq!(
            projected,
            json!({
                "records": [
                    { "id": "one", "amount": -10 }
                ],
                "nextOffset": 20
            })
        );
    }

    #[test]
    fn dry_run_query_preserves_repeated_params() {
        let query = vec![
            ("amount".to_owned(), "gte.10".to_owned()),
            ("amount".to_owned(), "lte.20".to_owned()),
        ];
        assert_eq!(
            query_as_object(&query),
            json!({ "amount": ["gte.10", "lte.20"] })
        );
    }
}
