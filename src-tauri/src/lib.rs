// 跃传 LeapDrop：macOS / Windows 局域网设备发现与文件互传
pub mod discovery;
pub mod identity;
pub mod protocol;
pub mod sender;
pub mod server;
pub mod settings;

use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use tauri::{Emitter as _, Manager, State};

/// 事件推送回调（Tauri 里转发到 webview；测试里可为空实现）
pub type Emitter = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

pub struct AppState {
    pub identity: RwLock<identity::Identity>,
    pub port: u16,
    pub download_dir: PathBuf,
    pub data_dir: PathBuf,
    pub peers: Arc<Mutex<HashMap<String, discovery::Peer>>>,
    pub sessions: Mutex<HashMap<String, server::Session>>,
    pub settings: Mutex<settings::Settings>,
    pub emitter: Emitter,
    pub discovery: OnceLock<Arc<discovery::Discovery>>,
}

pub fn random_token() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

#[derive(Serialize)]
pub struct StateResp {
    pub name: String,
    pub os: String,
    pub fp: String,
    pub device_id: String,
    pub port: u16,
    pub download_dir: String,
    pub receive_enabled: bool,
}

#[tauri::command]
fn get_state(state: State<'_, Arc<AppState>>) -> StateResp {
    let id = state.identity.read().unwrap();
    StateResp {
        name: id.name.clone(),
        os: id.os.clone(),
        fp: format!("{}…{}", &id.fingerprint[..8], &id.fingerprint[56..]),
        device_id: id.device_id.clone(),
        port: state.port,
        download_dir: state.download_dir.display().to_string(),
        receive_enabled: state.settings.lock().unwrap().receive_enabled,
    }
}

/// 选择文件（原生对话框）
#[tauri::command]
fn pick_files(app: tauri::AppHandle) -> Vec<String> {
    use tauri_plugin_dialog::DialogExt;
    match app.dialog().file().blocking_pick_files() {
        Some(list) => list
            .into_iter()
            .filter_map(|p| p.into_path().ok().map(|p| p.display().to_string()))
            .collect(),
        None => vec![],
    }
}

/// 发送文件到指定设备（异步任务，进度经 tx-* 事件汇报）
#[tauri::command]
async fn send_files(
    state: State<'_, Arc<AppState>>,
    target: String,
    paths: Vec<String>,
) -> Result<String, String> {
    if paths.is_empty() {
        return Err("没有选择文件".into());
    }
    let peer = {
        let peers = state.peers.lock().map_err(|e| e.to_string())?;
        peers
            .get(&target)
            .cloned()
            .ok_or_else(|| "目标设备已离线".to_string())?
    };
    let app_state = state.inner().clone();
    let peer_name = peer.name.clone();
    (state.emitter)(
        "tx-start",
        serde_json::json!({"peer": peer_name, "files": paths.len()}),
    );
    tauri::async_runtime::spawn(async move {
        if let Err(e) = sender::send(&app_state, &peer, paths).await {
            (app_state.emitter)(
                "tx-error",
                serde_json::json!({"peer": peer_name, "message": e}),
            );
        }
    });
    Ok("started".into())
}

/// 用户对接收请求的决定
#[tauri::command]
fn respond_incoming(state: State<'_, Arc<AppState>>, token: String, accept: bool) {
    server::respond(&state, &token, accept);
}

/// 持久化本机是否允许接收文件。
#[tauri::command]
fn set_receive_enabled(state: State<'_, Arc<AppState>>, enabled: bool) -> Result<(), String> {
    settings::set_receive_enabled(&state, enabled)
}

/// 修改设备名：落盘 + 更新内存 + 重新注册 mDNS TXT
#[tauri::command]
fn set_device_name(state: State<'_, Arc<AppState>>, name: String) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() || name.len() > 60 {
        return Err("设备名需为 1~60 个字符".into());
    }
    identity::save_name(&state.data_dir, &name).map_err(|e| e.to_string())?;
    state.identity.write().unwrap().name = name.clone();

    if let Some(d) = state.discovery.get() {
        let id = state.identity.read().unwrap();
        let _ = d.daemon.unregister(&d.fullname());
        let mut props = HashMap::new();
        props.insert("v".to_string(), protocol::VERSION.to_string());
        props.insert("fp".to_string(), id.fingerprint.clone());
        props.insert("name".to_string(), name.clone());
        props.insert("os".to_string(), id.os.clone());
        props.insert("id".to_string(), id.device_id.clone());
        let ips_csv = identity::local_ipv4s()
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        if let Ok(info) = mdns_sd::ServiceInfo::new(
            discovery::SERVICE_TYPE,
            &d.instance,
            &format!("{}.local.", d.instance),
            ips_csv.as_str(),
            state.port,
            Some(props),
        ) {
            let _ = d.daemon.register(info);
        }
    }
    (state.emitter)("name-changed", serde_json::json!({"name": name}));
    Ok(())
}

impl discovery::Discovery {
    pub fn fullname(&self) -> String {
        format!("{}.{}", self.instance, discovery::SERVICE_TYPE)
    }
}

pub fn run() {
    // rustls 全局 crypto provider（reqwest/rustls/axum-server 共用 ring）
    let _ = rustls::crypto::ring::default_provider().install_default();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let data_dir = handle.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;

            let id = identity::load_or_create(&data_dir)?;
            let port = identity::pick_port(52920)?;
            let download_dir = dirs::download_dir()
                .or_else(dirs::home_dir)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("LeapDrop");
            std::fs::create_dir_all(&download_dir)?;

            let emitter: Emitter = Arc::new(move |name, payload| {
                let _ = handle.emit(name, payload);
            });

            let settings = settings::load(&data_dir)?;
            let state = Arc::new(AppState {
                identity: RwLock::new(id),
                port,
                download_dir,
                data_dir,
                peers: Arc::new(Mutex::new(HashMap::new())),
                sessions: Mutex::new(HashMap::new()),
                settings: Mutex::new(settings),
                emitter: emitter.clone(),
                discovery: OnceLock::new(),
            });

            // 接收端 HTTPS 服务
            let actual = tauri::async_runtime::block_on(server::start(state.clone()))?;
            debug_assert_eq!(actual, port);

            // mDNS 发现（注意：需要 identity 的快照，rename 后 TXT 单独重注册）
            let id_snapshot = state.identity.read().unwrap().clone();
            let disc =
                discovery::start(&id_snapshot, actual, state.peers.clone(), emitter.clone())?;
            let _ = state.discovery.set(Arc::new(disc));

            // 会话过期清理
            let sweep_state = state.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    server::sweep_expired(&sweep_state);
                }
            });

            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            pick_files,
            send_files,
            respond_incoming,
            set_receive_enabled,
            set_device_name
        ])
        .run(tauri::generate_context!())
        .expect("error while running leapdrop");
}
