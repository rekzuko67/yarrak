// Vanity Sniper - Amaç: Discord'a en hızlı ve en yakın istek atıp URL'yi kapmak.
// Bağlantılar: Sadece Discord API + senin webhook. Zararlı yazılım yok.

use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio_tungstenite::{connect_async, tungstenite::Message};

const CONFIG: Config = Config {
    server_id: "guild_id",
    host: "https://canary.discord.com",
    token: "your_token",
    webhook_url: "webhook_url",
    user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    super_props: "eyJicm93c2VyIjoiQ2hyb21lIiwiYnJvd3Nlcl91c2VyX2FnZW50IjoiQ2hyb21lIiwiY2xpZW50X2J1aWxkX251bWJlciI6MzU1NjI0fQ==",
    claim_parallel: 40,
};

#[derive(Clone)]
struct Config {
    server_id: &'static str,
    host: &'static str,
    token: &'static str,
    webhook_url: &'static str,
    user_agent: &'static str,
    super_props: &'static str,
    claim_parallel: usize,
}

#[derive(Deserialize)]
struct GatewayMessage {
    op: Option<u8>,
    t: Option<String>,
    d: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ReadyGuild {
    id: String,
    vanity_url_code: Option<String>,
}

#[derive(Deserialize)]
struct ReadyData {
    guilds: Vec<ReadyGuild>,
}

#[derive(Serialize)]
struct ClaimBody {
    code: String,
}

#[derive(Serialize)]
struct WebhookBody {
    content: String,
}

fn load_mfa() -> String {
    let paths = ["mfa.txt", "../mfa.txt"];
    for p in paths {
        if let Ok(s) = std::fs::read_to_string(p) {
            let t = s.trim().to_string();
            if !t.is_empty() {
                return t;
            }
        }
    }
    String::new()
}

fn log(msg: &str, typ: &str) {
    let time = chrono::Local::now().format("%H:%M:%S");
    let symbol = match typ {
        "success" => "\x1b[32m✓\x1b[0m",
        "error" => "\x1b[31m✗\x1b[0m",
        "warn" => "\x1b[33m!\x1b[0m",
        _ => "\x1b[34m•\x1b[0m",
    };
    println!("[{}] {} {}", time, symbol, msg);
}

async fn notify(client: &Client, content: &str) {
    if CONFIG.webhook_url.is_empty() || CONFIG.webhook_url == "webhook_url" {
        return;
    }
    let _ = client
        .post(CONFIG.webhook_url)
        .json(&WebhookBody {
            content: content.to_string(),
        })
        .send()
        .await;
}

async fn claim(client: &Client, url: &str, code: &str, mfa_token: &str) {
    if mfa_token.is_empty() {
        log("MFA token empty - cannot claim", "error");
        return;
    }

    let n = CONFIG.claim_parallel;
    for _ in 0..n {
        let code = code.to_string();
        let url = url.to_string();
        let mfa_token = mfa_token.to_string();
        let client = client.clone();
        tokio::spawn(async move {
            let req = client
                .patch(&url)
                .header("Authorization", CONFIG.token)
                .header("x-discord-mfa-authorization", mfa_token)
                .header("user-agent", CONFIG.user_agent)
                .header("x-super-properties", CONFIG.super_props)
                .json(&ClaimBody { code: code.clone() });
            let start = Instant::now();
            match req.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let ms = start.elapsed().as_millis();
                    if status == 200 || status == 204 {
                        log(&format!("Claimed: {} ({}ms)", code, ms), "success");
                        notify(&client, &format!("@everyone Claimed: {} ({}ms)", code, ms)).await;
                    } else {
                        let body = resp.text().await.unwrap_or_default();
                        let api_code: Option<i64> = serde_json::from_str(&body)
                            .ok()
                            .and_then(|v: serde_json::Value| v.get("code").and_then(|c| c.as_i64()));
                        if api_code == Some(60003) {
                            log("MFA expired/invalid (60003) - update mfa.txt with fresh token", "error");
                        } else {
                            log(&format!("Failed: {} - Status {}", code, status), "error");
                        }
                    }
                }
                Err(_) => {}
            }
        });
    }
    log(&format!("Claiming: {}", code), "info");
}

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async_main());
}

async fn async_main() {
    let client = Client::builder()
        .pool_idle_timeout(Duration::from_secs(120))
        .pool_max_idle_per_host(48)
        .build()
        .expect("http client");

    let vanity_url = format!(
        "{}/api/v9/guilds/{}/vanity-url",
        CONFIG.host.trim_end_matches('/'),
        CONFIG.server_id
    );

    let mfa = Arc::new(RwLock::new(load_mfa()));
    let servers: Arc<RwLock<HashMap<String, String>>> = Arc::new(RwLock::new(HashMap::new()));
    let monitoring = Arc::new(AtomicBool::new(false));

    tokio::spawn({
        let mfa = Arc::clone(&mfa);
        async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                let new_mfa = load_mfa();
                if !new_mfa.is_empty() {
                    let mut g = mfa.write().await;
                    *g = new_mfa;
                }
            }
        }
    });

    let keepalive_client = client.clone();
    let keepalive_mfa = Arc::clone(&mfa);
    tokio::spawn(async move {
        let url = format!("{}/api/v9/users/@me", CONFIG.host.trim_end_matches('/'));
        let mut interval = tokio::time::interval(Duration::from_millis(1900));
        loop {
            interval.tick().await;
            let mfa_token = keepalive_mfa.read().await.clone();
            let req = keepalive_client
                .get(&url)
                .header("Authorization", CONFIG.token)
                .header("x-discord-mfa-authorization", mfa_token)
                .header("user-agent", CONFIG.user_agent)
                .header("x-super-properties", CONFIG.super_props);
            let _ = req.send().await;
        }
    });

    loop {
        match run_gateway(&client, &vanity_url, Arc::clone(&mfa), Arc::clone(&servers), Arc::clone(&monitoring)).await {
            Ok(()) => {}
            Err(e) => log(&format!("Gateway error: {}", e), "error"),
        }
        log("Reconnecting in 5s...", "warn");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn run_gateway(
    client: &Client,
    vanity_url: &str,
    mfa: Arc<RwLock<String>>,
    servers: Arc<RwLock<HashMap<String, String>>>,
    monitoring: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (ws, _) = connect_async("wss://gateway.discord.gg/?v=9&encoding=json").await?;
    let (mut write, mut read) = ws.split();

    let identify = serde_json::json!({
        "op": 2,
        "d": {
            "token": CONFIG.token,
            "intents": 1,
            "properties": { "os": "linux", "browser": "chrome", "device": "mehdiffer" }
        }
    });
    write.send(Message::Text(identify.to_string())).await?;

    let mut heartbeat_interval = Duration::from_millis(41250);
    let mut interval = tokio::time::interval(heartbeat_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            msg_opt = read.next() => {
                let msg = match msg_opt {
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => continue,
                };

                if msg.len() < 30 && msg.contains("\"op\":11") {
                    continue;
                }

                let gw: GatewayMessage = match serde_json::from_str(&msg) {
                    Ok(g) => g,
                    Err(_) => continue,
                };

                if let Some(10) = gw.op {
                    if let Some(d) = gw.d {
                        if let Some(hi) = d.get("heartbeat_interval").and_then(|v| v.as_u64()) {
                            heartbeat_interval = Duration::from_millis(hi);
                            interval = tokio::time::interval(heartbeat_interval);
                            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                        }
                    }
                    continue;
                }

                let t = gw.t.as_deref().unwrap_or("");
                let d = match gw.d {
                    Some(v) => v,
                    None => continue,
                };

                if t == "GUILD_UPDATE" || t == "GUILD_DELETE" {
                    let guild_id = d.get("guild_id").and_then(|v| v.as_str()).or_else(|| d.get("id").and_then(|v| v.as_str())).unwrap_or("").to_string();
                    let new_vanity = d.get("vanity_url_code").and_then(|v| v.as_str()).map(String::from);

                    let old_vanity = servers.read().await.get(&guild_id).cloned();
                    let should_claim = old_vanity.as_ref().map_or(false, |old| {
                        t == "GUILD_DELETE" || new_vanity.as_deref() != Some(old.as_str())
                    });

                    if should_claim {
                        let code = old_vanity.unwrap();
                        let mfa_token = mfa.read().await.clone();
                        let client = client.clone();
                        let vanity_url = vanity_url.to_string();
                        tokio::spawn(async move {
                            claim(&client, &vanity_url, &code, &mfa_token).await;
                        });
                    }

                    if let Some(v) = new_vanity {
                        servers.write().await.insert(guild_id, v);
                    } else {
                        servers.write().await.remove(&guild_id);
                    }
                } else if t == "READY" {
                    let ready: ReadyData = match serde_json::from_value(d) {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    let mut s = servers.write().await;
                    for g in ready.guilds {
                        if let Some(v) = g.vanity_url_code {
                            s.insert(g.id, v);
                        }
                    }

                    if !monitoring.swap(true, Ordering::SeqCst) {
                        println!("\nVanity Sniper (Rust) • Online");
                        println!("Guilds: {} | Webhook: Active\n", s.len());
                        log("Gateway connected", "success");
                        log("READY - Monitoring active", "success");

                        let mfa_token = mfa.read().await.clone();
                        if !mfa_token.is_empty() {
                            let me_url = format!("{}/api/v9/users/@me", CONFIG.host.trim_end_matches('/'));
                            for _ in 0..16 {
                                let c = client.clone();
                                let m = mfa_token.clone();
                                let u = me_url.clone();
                                tokio::spawn(async move {
                                    let _ = c.get(&u).header("Authorization", CONFIG.token).header("x-discord-mfa-authorization", m).send().await;
                                });
                            }
                        }
                    }
                }
            }
            _ = interval.tick() => {
                let _ = write.send(Message::Text(r#"{"op":1,"d":null}"#.to_string())).await;
            }
        }
    }

    monitoring.store(false, Ordering::SeqCst);
    Ok(())
}
