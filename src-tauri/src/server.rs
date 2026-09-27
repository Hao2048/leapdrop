// 接收端：HTTPS 服务（axum + rustls 自签证书）
// 端点：GET /api/hello | POST /api/pair（配对确认，长轮询）| POST /api/prepare（配对+验签+长轮询确认）
//       | PUT /api/chunk/{token}/{file}/{idx} | POST /api/finalize/{token}
use crate::protocol::*;
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use base64::Engine as _;
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

/// 一次配对请求会话
pub struct PairSession {
    pub req: PairReq,
    pub created: Instant,
    pub decided: Option<bool>,
    pub notify: Arc<tokio::sync::Notify>,
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
        .route("/api/pair", post(pair))
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
        id: id.device_id.clone(),
        v: VERSION.to_string(),
    })
}

/// 我方信息（用于配对响应 / 403 提示）
fn my_info(app: &AppState) -> (String, String, String) {
    let id = app.identity.read().unwrap();
    (
        id.device_id.clone(),
        id.name.clone(),
        base64::engine::general_purpose::STANDARD.encode(&id.device_pk),
    )
}

/// 配对：验证签名（绑定 ID+公钥）→ 已配对直接成功；否则弹 UI 确认（长轮询 ≤60s）
async fn pair(
    State(s): State<Arc<ServerShared>>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<PairResp>, (StatusCode, Json<serde_json::Value>)> {
    let req: PairReq = serde_json::from_str(&body)
        .map_err(|_| (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "bad_body"}))))?;
    let sig = headers
        .get("x-leapdrop-sig")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| base64::engine::general_purpose::STANDARD.decode(v).ok())
        .unwrap_or_default();
    let pk = base64::engine::general_purpose::STANDARD
        .decode(&req.pk)
        .unwrap_or_default();
    // 签名覆盖整个请求体原文，证明请求方持有该公钥对应的私钥
    if pk.is_empty() || sig.is_empty() || !crate::identity::verify(&pk, body.as_bytes(), &sig) {
        return Err((StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "bad_signature"}))));
    }
    if crate::identity::device_id_of(&pk) != req.device_id {
        return Err((StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "id_mismatch"}))));
    }

    // 已配对：幂等返回
    if s.app.trusted.lock().unwrap().iter().any(|t| t.id == req.device_id && t.pk == req.pk) {
        let (id, name, pk) = my_info(&s.app);
        return Ok(Json(PairResp { accepted: true, status: "already".into(), device_id: id, name, pk }));
    }

    let token = crate::random_token();
    let notify = Arc::new(tokio::sync::Notify::new());
    {
        let mut ps = s.app.pair_sessions.lock().unwrap();
        ps.insert(
            token.clone(),
            PairSession { req: req.clone(), created: Instant::now(), decided: None, notify: notify.clone() },
        );
    }

    (s.app.emitter)(
        "pair-incoming",
        serde_json::json!({"token": token, "deviceId": req.device_id, "name": req.name}),
    );

    tokio::select! {
        _ = notify.notified() => {}
        _ = tokio::time::sleep(Duration::from_secs(60)) => {}
    }
    let accepted = s.app
        .pair_sessions
        .lock()
        .unwrap()
        .get(&token)
        .and_then(|x| x.decided)
        .unwrap_or(false);
    s.app.pair_sessions.lock().unwrap().remove(&token);
    if !accepted {
        (s.app.emitter)("pair-closed", serde_json::json!({"token": token}));
        return Ok(Json(PairResp { accepted: false, status: String::new(), device_id: String::new(), name: String::new(), pk: String::new() }));
    }

    // 记住对方
    crate::add_trusted(&s.app, TrustedPeer {
        id: req.device_id.clone(),
        name: req.name.clone(),
        pk: req.pk.clone(),
        paired_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
    });
    let (id, name, pk) = my_info(&s.app);
    Ok(Json(PairResp { accepted: true, status: "ok".into(), device_id: id, name, pk }))
}

/// UI 命令：用户对配对弹窗做出决定
pub fn respond_pair(app: &AppState, token: &str, accept: bool) {
    let mut ps = app.pair_sessions.lock().unwrap();
    if let Some(session) = ps.get_mut(token) {
        session.decided = Some(accept);
        session.notify.notify_waiters();
    }
}

/// 发起传输：配对门禁（ID 在信任列表 + 签名验证）→ 弹 UI 确认 → 长轮询 → 返回 token + 续传 bitmap
async fn prepare(
    State(s): State<Arc<ServerShared>>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<PrepareResp>, (StatusCode, Json<NeedPairing>)> {
    let need_pairing = |app: &AppState| {
        let (id, name, pk) = my_info(app);
        (StatusCode::FORBIDDEN, Json(NeedPairing { error: "need_pairing".into(), device_id: id, name, pk }))
    };

    let sender_id = headers
        .get("x-leapdrop-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let trusted = {
        let list = s.app.trusted.lock().unwrap();
        list.iter().find(|t| t.id == sender_id).cloned()
    };
    let Some(trusted) = trusted else {
        return Err(need_pairing(&s.app));
    };
    // 验签：签名覆盖 prepare 请求体原文
    let sig = headers
        .get("x-leapdrop-sig")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| base64::engine::general_purpose::STANDARD.decode(v).ok())
        .unwrap_or_default();
    let pk = base64::engine::general_purpose::STANDARD.decode(&trusted.pk).unwrap_or_default();
    if pk.is_empty() || !crate::identity::verify(&pk, body.as_bytes(), &sig) {
        return Err(need_pairing(&s.app));
    }

    let req: PrepareReq = serde_json::from_str(&body)
        .map_err(|_| (StatusCode::BAD_REQUEST, Json(NeedPairing { error: "bad_body".into(), device_id: String::new(), name: String::new(), pk: String::new() })))?;
    let peer_name = trusted.name.clone();
    let token = crate::random_token();
    let tmp_dir = s.app.download_dir.join(".leapdrop-tmp").join(&token);
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(NeedPairing { error: "io".into(), device_id: String::new(), name: String::new(), pk: String::new() })))?;

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
    let mut ps = app.pair_sessions.lock().unwrap();
    ps.retain(|_, s| s.created.elapsed() < Duration::from_secs(10 * 60));
}
