// 接收端：HTTPS 服务（axum + rustls 自签证书）
// 端点：GET /api/hello | POST /api/prepare（长轮询等用户确认）| PUT /api/chunk/{token}/{file}/{idx} | POST /api/finalize/{token}
use crate::protocol::*;
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 一次接收会话
pub struct Session {
    pub peer_name: String,
    pub files: Vec<FileMeta>,
    pub tmp_dir: PathBuf,
    pub created: Instant,
    pub decided: Option<bool>,          // 用户是否已决定
    pub notify: Arc<tokio::sync::Notify>,
    pub finalized: bool,
}

pub struct ServerShared {
    pub app: Arc<AppState>,
}

pub async fn start(app: Arc<AppState>) -> std::io::Result<u16> {
    // rustls 需要 process-level crypto provider（与 reqwest 共用 ring）
    let _ = rustls::crypto::ring::default_provider().install_default();

    let (cert_der, key_der) = {
        let id = app.identity.read().unwrap();
        (id.cert_der.clone(), id.key_der.clone())
    };
    let tls = axum_server::tls_rustls::RustlsConfig::from_der(vec![cert_der], key_der)
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("tls: {e}")))?;

    let listener = std::net::TcpListener::bind(("0.0.0.0", app.port))?;
    let port = listener.local_addr()?.port();

    let shared = Arc::new(ServerShared { app: app.clone() });
    let router = Router::new()
        .route("/api/hello", get(hello))
        .route("/api/prepare", post(prepare))
        .route("/api/chunk/{token}/{file}/{idx}", put(chunk))
        .route("/api/finalize/{token}", post(finalize))
        .layer(DefaultBodyLimit::max((CHUNK_SIZE as usize) + 1024 * 1024))
        .with_state(shared);

    tokio::spawn(async move {
        if let Err(e) = axum_server::from_tcp_rustls(listener, tls)
            .serve(router.into_make_service())
            .await
        {
            eprintln!("[leapdrop] server error: {e}");
        }
    });
    Ok(port)
}

async fn hello(State(s): State<Arc<ServerShared>>) -> Json<HelloResp> {
    let id = s.app.identity.read().unwrap();
    Json(HelloResp {
        name: id.name.clone(),
        fp: id.fingerprint.clone(),
        v: VERSION.to_string(),
    })
}

/// 发起传输：登记会话 → 弹 UI 确认 → 长轮询等待决定 → 返回 token + 续传 bitmap
async fn prepare(
    State(s): State<Arc<ServerShared>>,
    headers: HeaderMap,
    Json(req): Json<PrepareReq>,
) -> Result<Json<PrepareResp>, StatusCode> {
    let peer_name = headers
        .get("x-leapdrop-name")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("未知设备")
        .to_string();
    let token = crate::random_token();
    let tmp_dir = s.app.download_dir.join(".leapdrop-tmp").join(&token);
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let notify = Arc::new(tokio::sync::Notify::new());
    {
        let mut sessions = s.app.sessions.lock().unwrap();
        sessions.insert(
            token.clone(),
            Session {
                peer_name: peer_name.clone(),
                files: req.files.clone(),
                tmp_dir,
                created: Instant::now(),
                decided: None,
                notify: notify.clone(),
                finalized: false,
            },
        );
    }

    // 通知 UI 弹窗（接收确认）
    (s.app.emitter)(
        "incoming",
        serde_json::json!({
            "token": token,
            "peer": peer_name,
            "files": req.files.iter().map(|f| serde_json::json!({"name": f.name, "size": f.size})).collect::<Vec<_>>(),
            "totalBytes": req.total_bytes,
        }),
    );

    // 长轮询用户决定（最多 90 秒）
    tokio::select! {
        _ = notify.notified() => {}
        _ = tokio::time::sleep(Duration::from_secs(90)) => {}
    }
    let accepted = s
        .app
        .sessions
        .lock()
        .unwrap()
        .get(&token)
        .and_then(|x| x.decided)
        .unwrap_or(false);

    if !accepted {
        s.app.sessions.lock().unwrap().remove(&token);
        (s.app.emitter)("incoming-closed", serde_json::json!({"token": token}));
        return Ok(Json(PrepareResp {
            accepted: false,
            token: String::new(),
            resume: Default::default(),
            message: "对方拒绝了传输请求".into(),
        }));
    }

    // 计算续传：扫描 .part 文件与 bitmap 边车
    let mut resume = ResumeMap::new();
    if let Some(session) = s.app.sessions.lock().unwrap().get(&token) {
        for (i, f) in session.files.iter().enumerate() {
            let part = session.tmp_dir.join(format!("{i}.part"));
            let sidecar = session.tmp_dir.join(format!("{i}.bitmap"));
            if let Ok(bm) = std::fs::read(&sidecar) {
                if let Ok(list) = serde_json::from_slice::<Vec<u64>>(&bm) {
                    if !list.is_empty() {
                        resume.insert(i, list);
                        continue;
                    }
                }
            }
            if part.exists() {
                // 无边车但有 part：按文件长度推断连续前缀
                if let Ok(meta) = std::fs::metadata(&part) {
                    let have = meta.len() / f.chunk_size;
                    if have > 0 {
                        resume.insert(i, (0..have).collect());
                    }
                }
            }
        }
    }

    Ok(Json(PrepareResp {
        accepted: true,
        token,
        resume,
        message: String::new(),
    }))
}

/// UI 命令：用户对 incoming 弹窗做出决定
pub fn respond(app: &AppState, token: &str, accept: bool) {
    let mut sessions = app.sessions.lock().unwrap();
    if let Some(session) = sessions.get_mut(token) {
        session.decided = Some(accept);
        session.notify.notify_waiters();
    }
}

/// 接收一个分块：pwrite 到 .part 固定偏移，更新 bitmap 边车
async fn chunk(
    State(s): State<Arc<ServerShared>>,
    Path((token, file_idx, chunk_idx)): Path<(String, usize, u64)>,
    body: Bytes,
) -> StatusCode {
    let (tmp_dir, file_meta, total_chunks) = {
        let sessions = s.app.sessions.lock().unwrap();
        let Some(session) = sessions.get(&token) else {
            return StatusCode::NOT_FOUND;
        };
        let Some(f) = session.files.get(file_idx) else {
            return StatusCode::NOT_FOUND;
        };
        (session.tmp_dir.clone(), f.name.clone(), f.chunks)
    };

    let part = tmp_dir.join(format!("{file_idx}.part"));
    let offset = chunk_idx * CHUNK_SIZE;

    // 打开+定位+写（tokio 文件支持 AsyncSeekExt）
    use tokio::io::{AsyncSeekExt, AsyncWriteExt};
    let mut fh = match tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&part)
        .await
    {
        Ok(f) => f,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
    };
    if fh.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    if fh.write_all(&body).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    if fh.sync_data().await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    // bitmap 边车（读改写，量小无所谓）
    let sidecar = tmp_dir.join(format!("{file_idx}.bitmap"));
    let mut have: Vec<u64> = std::fs::read(&sidecar)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if !have.contains(&chunk_idx) {
        have.push(chunk_idx);
        let _ = std::fs::write(&sidecar, serde_json::to_vec(&have).unwrap());
    }

    // 接收进度事件（节流在 UI 侧做）
    let bytes = body.len() as u64;
    let done = have.len() as u64;
    (s.app.emitter)(
        "rx-progress",
        serde_json::json!({
            "token": token, "fileIdx": file_idx, "fileName": file_meta,
            "chunksDone": done, "chunksTotal": total_chunks, "bytes": bytes,
        }),
    );

    StatusCode::NO_CONTENT
}

/// 全部传完：逐文件校验 sha256，落盘到下载目录
async fn finalize(
    State(s): State<Arc<ServerShared>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let (files, tmp_dir, peer_name) = {
        let mut sessions = s.app.sessions.lock().unwrap();
        let Some(session) = sessions.get_mut(&token) else {
            return Err(StatusCode::NOT_FOUND);
        };
        if session.finalized {
            return Err(StatusCode::CONFLICT);
        }
        session.finalized = true;
        (session.files.clone(), session.tmp_dir.clone(), session.peer_name.clone())
    };

    let mut saved = vec![];
    let mut bad = vec![];
    for (i, f) in files.iter().enumerate() {
        let part = tmp_dir.join(format!("{i}.part"));
        // 校验
        let mut fh = match tokio::fs::File::open(&part).await {
            Ok(f) => f,
            Err(_) => {
                bad.push(i);
                continue;
            }
        };
        use tokio::io::AsyncReadExt;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1024 * 1024];
        let mut size = 0u64;
        loop {
            let n = match fh.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
            };
            hasher.update(&buf[..n]);
            size += n as u64;
        }
        let hash = hex::encode(hasher.finalize());
        if size != f.size || hash != f.sha256 {
            bad.push(i);
            continue;
        }
        // 落盘（重名自动加序号）
        let mut dest = s.app.download_dir.join(&f.name);
        if dest.exists() {
            if let Some(stem) = f.name.rsplit_once('.') {
                let mut n = 1;
                loop {
                    dest = s.app.download_dir.join(format!("{} ({n}).{}", stem.0, stem.1));
                    if !dest.exists() {
                        break;
                    }
                    n += 1;
                }
            }
        }
        if tokio::fs::rename(&part, &dest).await.is_ok() {
            let _ = tokio::fs::remove_file(tmp_dir.join(format!("{i}.bitmap"))).await;
            saved.push(dest.display().to_string());
        } else {
            bad.push(i);
        }
    }
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
    s.app.sessions.lock().unwrap().remove(&token);

    let ok = bad.is_empty();
    (s.app.emitter)(
        "rx-done",
        serde_json::json!({
            "token": token, "peer": peer_name, "ok": ok,
            "saved": saved, "bad": bad,
        }),
    );
    if ok {
        Ok(Json(serde_json::json!({"ok": true, "saved": saved})))
    } else {
        Err(StatusCode::CONFLICT)
    }
}

/// 会话过期清理（由 lib.rs 周期调用）
pub fn sweep_expired(app: &AppState) {
    let mut sessions = app.sessions.lock().unwrap();
    let expired: Vec<String> = sessions
        .iter()
        .filter(|(_, s)| s.created.elapsed() > Duration::from_secs(30 * 60))
        .map(|(k, _)| k.clone())
        .collect();
    for k in expired {
        if let Some(s) = sessions.remove(&k) {
            let _ = std::fs::remove_dir_all(&s.tmp_dir);
        }
    }
}
