use std::collections::HashMap;
use std::env;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

const DEFAULT_INITIAL_ASSET_NAME: &str = "appstore/Expo-Go-54.0.6.tar.gz";

#[derive(Serialize)]
struct CreateIosInstanceRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<CreateMetadata>,
    spec: CreateSpec,
}

#[derive(Serialize)]
struct CreateMetadata {
    labels: HashMap<String, String>,
}

#[derive(Serialize)]
struct CreateSpec {
    #[serde(rename = "initialAssets")]
    initial_assets: Vec<InitialAsset>,
}

#[derive(Serialize)]
struct InitialAsset {
    kind: &'static str,
    source: &'static str,
    #[serde(rename = "assetName")]
    asset_name: String,
}

#[derive(Deserialize)]
struct IosInstance {
    metadata: IosMetadata,
    status: IosStatus,
}

#[derive(Deserialize)]
struct IosMetadata {
    id: String,
}

#[derive(Deserialize)]
struct IosStatus {
    token: String,
    #[serde(rename = "apiUrl")]
    api_url: Option<String>,
    #[serde(rename = "mcpUrl")]
    mcp_url: Option<String>,
}

#[derive(Serialize)]
struct Output {
    #[serde(rename = "instanceId")]
    instance_id: String,
    #[serde(rename = "openedUrl")]
    opened_url: String,
    #[serde(rename = "mcpUrl")]
    mcp_url: String,
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("Error: {err:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let (open_url, label) = parse_args()?;
    let api_key = env::var("LIM_API_KEY").context("LIM_API_KEY is required")?;
    let base_url = env::var("LIMRUN_BASE_URL").unwrap_or_else(|_| "https://api.limrun.com".to_string());
    let initial_asset_name =
        env::var("LIM_INITIAL_ASSET_NAME").unwrap_or_else(|_| DEFAULT_INITIAL_ASSET_NAME.to_string());

    let http = Client::builder().build().context("failed to build HTTP client")?;
    let instance =
        create_ios_instance(&http, &base_url, &api_key, label.as_deref(), &initial_asset_name).await?;

    let api_url = instance
        .status
        .api_url
        .as_deref()
        .context("status.apiUrl is missing from iOS instance")?;
    let token = &instance.status.token;

    let ws_url = build_signaling_ws_url(api_url, token)?;
    send_open_url(&ws_url, &open_url).await?;

    let mcp_url = instance
        .status
        .mcp_url
        .context("status.mcpUrl is missing from iOS instance")?;

    let output = Output {
        instance_id: instance.metadata.id,
        opened_url: open_url,
        mcp_url,
    };

    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

fn parse_args() -> Result<(String, Option<String>)> {
    let mut args = env::args().skip(1);
    let open_url = args
        .next()
        .context("missing URL argument\nusage: cargo run -- <url-to-open> [label]")?;
    let label = args.next();

    if args.next().is_some() {
        bail!("too many arguments\nusage: cargo run -- <url-to-open> [label]");
    }

    Ok((open_url, label))
}

async fn create_ios_instance(
    http: &Client,
    base_url: &str,
    api_key: &str,
    label: Option<&str>,
    initial_asset_name: &str,
) -> Result<IosInstance> {
    let url = format!("{}/v1/ios_instances", base_url.trim_end_matches('/'));

    let body = CreateIosInstanceRequest {
        metadata: label.map(|value| {
            let mut labels = HashMap::new();
            labels.insert("name".to_string(), value.to_string());
            CreateMetadata { labels }
        }),
        spec: CreateSpec {
            initial_assets: vec![InitialAsset {
                kind: "App",
                source: "AssetName",
                asset_name: initial_asset_name.to_string(),
            }],
        },
    };

    let response = http
        .post(url)
        .query(&[("wait", "true"), ("reuseIfExists", "true")])
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .context("failed to call create iOS instance API")?
        .error_for_status()
        .context("create iOS instance request failed")?;

    response
        .json::<IosInstance>()
        .await
        .context("failed to parse create iOS instance response")
}

fn build_signaling_ws_url(api_url: &str, token: &str) -> Result<String> {
    let ws_base = if let Some(rest) = api_url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = api_url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        bail!("status.apiUrl has unsupported scheme: {api_url}");
    };

    Ok(format!(
        "{}/signaling?token={}",
        ws_base.trim_end_matches('/'),
        token
    ))
}

async fn send_open_url(ws_url: &str, open_url: &str) -> Result<()> {
    let (mut ws, _) = connect_async(ws_url)
        .await
        .with_context(|| format!("failed to connect websocket: {ws_url}"))?;

    let request_id = Uuid::new_v4().to_string();
    let request = json!({
        "type": "openUrl",
        "id": request_id,
        "url": open_url
    });

    ws.send(Message::Text(request.to_string().into()))
        .await
        .context("failed to send openUrl request")?;

    timeout(Duration::from_secs(20), async {
        while let Some(frame) = ws.next().await {
            let frame = frame.context("failed reading websocket frame")?;
            let Some(text) = frame_to_text(frame)? else {
                continue;
            };

            let message: Value =
                serde_json::from_str(&text).context("failed to parse websocket JSON message")?;
            let Some(id) = message.get("id").and_then(Value::as_str) else {
                continue;
            };

            if id != request_id {
                continue;
            }

            if let Some(error) = message.get("error").and_then(Value::as_str) {
                bail!("openUrl failed: {error}");
            }

            let message_type = message.get("type").and_then(Value::as_str).unwrap_or("");
            if message_type == "openUrlResult" {
                return Ok(());
            }

            bail!("unexpected websocket response type for openUrl: {message_type}");
        }

        bail!("websocket closed before openUrlResult was received");
    })
    .await
    .context("timed out waiting for openUrlResult")?
}

fn frame_to_text(frame: Message) -> Result<Option<String>> {
    match frame {
        Message::Text(text) => Ok(Some(text.to_string())),
        Message::Binary(bytes) => Ok(Some(
            String::from_utf8(bytes.to_vec()).context("binary websocket frame was not UTF-8")?,
        )),
        Message::Ping(_) | Message::Pong(_) => Ok(None),
        Message::Close(_) => Ok(None),
        Message::Frame(_) => Ok(None),
    }
}
