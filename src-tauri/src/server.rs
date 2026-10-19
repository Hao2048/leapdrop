// 接收端：HTTPS 服务（axum + rustls 自签证书）
// 端点：POST /api/prepare（接收开关 + 长轮询确认）
//       | PUT /api/chunk/{token}/{file}/{idx} | POST /api/finalize/{token}
use crate::protocol::*;
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::routing::{post, put};
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
    pub decided: Option<bool>, // 用户是否已决定
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
        .route("/api/prepare", post(http_prepare))
        .route("/api/chunk/{token}/{file}/{idx}", put(http_chunk))
        .route("/api/finalize/{token}", post(http_finalize))
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

fn declined(message: &str) -> PrepareResp {
    PrepareResp {
        accepted: false,
        token: String::new(),
        message: message.into(),
    }
}

/// 发现设备后直接请求传输；接收开关开启时等待用户确认。
pub async fn core_prepare(app: &Arc<AppState>, req: PrepareReq) -> Result<PrepareResp, String> {
    if !app.settings.lock().unwrap().receive_enabled {
        return Ok(declined("对方已关闭文件接收"));
    }
    if req.files.is_empty()
        || req.files.iter().any(|f| {
            f.name.is_empty()
                || f.name == "."
                || f.name == ".."
                || f.name.contains(['/', '\\', ':', '\0'])
                || f.chunk_size != CHUNK_SIZE
                || f.chunks != chunk_count(f.size)
        })
    {
        return Err("文件信息无效".into());
    }
    let peer_name = req.sender_name.clone();
    let token = crate::random_token();
    let tmp_dir = app.download_dir.join(".leapdrop-tmp").join(&token);
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .map_err(|_| "io error".to_string())?;

    let notify = Arc::new(tokio::sync::Notify::new());
    {
        let settings = app.settings.lock().unwrap();
        if !settings.receive_enabled {
            drop(settings);
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Ok(declined("对方已关闭文件接收"));
        }
        let mut sessions = app.sessions.lock().unwrap();
        sessions.insert(
            token.clone(),
            Session {
                peer_name: peer_name.clone(),
                files: req.files.clone(),
                tmp_dir: tmp_dir.clone(),
                created: Instant::now(),
                decided: None,
                notify: notify.clone(),
                finalized: false,
            },
        );
        drop(sessions);

        // 通知 UI 弹窗（接收确认）
        (app.emitter)(
            "incoming",
            serde_json::json!({
                "token": token,
                "peer": peer_name,
                "files": req.files.iter().map(|f| serde_json::json!({"name": f.name, "size": f.size})).collect::<Vec<_>>(),
                "totalBytes": req.total_bytes,
            }),
        );
    }

    // 长轮询用户决定（最多 90 秒）
    tokio::select! {
        _ = notify.notified() => {}
        _ = tokio::time::sleep(Duration::from_secs(90)) => {}
    }
    let accepted = app
        .sessions
        .lock()
        .unwrap()
        .get(&token)
        .and_then(|x| x.decided)
        .unwrap_or(false);

    if !accepted {
        app.sessions.lock().unwrap().remove(&token);
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
        (app.emitter)("incoming-closed", serde_json::json!({"token": token}));
        return Ok(PrepareResp {
            accepted: false,
            token: String::new(),
            message: if app.settings.lock().unwrap().receive_enabled {
                "对方拒绝了传输请求"
            } else {
                "对方已关闭文件接收"
            }
            .into(),
        });
    }

    for (i, f) in req.files.iter().enumerate() {
        if f.size == 0 {
            tokio::fs::File::create(tmp_dir.join(format!("{i}.part")))
                .await
                .map_err(|_| "io error".to_string())?;
        }
    }

    Ok(PrepareResp {
        accepted: true,
        token,
        message: String::new(),
    })
}

/// UI 命令：用户对 incoming 弹窗做出决定
pub fn respond(app: &AppState, token: &str, accept: bool) {
    let settings = app.settings.lock().unwrap();
    let mut sessions = app.sessions.lock().unwrap();
    if let Some(session) = sessions.get_mut(token) {
        if session.decided.is_some() {
            return;
        }
        session.decided = Some(accept && settings.receive_enabled);
        session.notify.notify_one();
    }
}

/// 接收一个分块：写入 .part 的固定偏移
pub async fn core_chunk(
    app: &Arc<AppState>,
    token: &str,
    file_idx: usize,
    chunk_idx: u64,
    body: Bytes,
) -> Result<(), u16> {
    let (tmp_dir, file_meta, total_chunks) = {
        let sessions = app.sessions.lock().unwrap();
        let Some(session) = sessions.get(token) else {
            return Err(404);
        };
        if session.decided != Some(true) || session.finalized {
            return Err(403);
        }
        let Some(f) = session.files.get(file_idx) else {
            return Err(404);
        };
        if chunk_idx >= f.chunks {
            return Err(400);
        }
        let expected = (f.size - chunk_idx * CHUNK_SIZE).min(CHUNK_SIZE);
        if body.len() as u64 != expected {
            return Err(400);
        }
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
        Err(_) => return Err(500),
    };
    if fh.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
        return Err(500);
    }
    if fh.write_all(&body).await.is_err() {
        return Err(500);
    }
    if fh.sync_data().await.is_err() {
        return Err(500);
    }

    // 接收进度事件（节流在 UI 侧做）
    let bytes = body.len() as u64;
    let done = chunk_idx + 1;
    (app.emitter)(
        "rx-progress",
        serde_json::json!({
            "token": token, "fileIdx": file_idx, "fileName": file_meta,
            "chunksDone": done, "chunksTotal": total_chunks, "bytes": bytes,
        }),
    );

    Ok(())
}

/// 全部传完：逐文件校验 sha256，落盘到下载目录
pub async fn core_finalize(app: &Arc<AppState>, token: &str) -> Result<serde_json::Value, u16> {
    let (files, tmp_dir, peer_name) = {
        let mut sessions = app.sessions.lock().unwrap();
        let Some(session) = sessions.get_mut(token) else {
            return Err(404);
        };
        if session.decided != Some(true) {
            return Err(403);
        }
        if session.finalized {
            return Err(409);
        }
        session.finalized = true;
        (
            session.files.clone(),
            session.tmp_dir.clone(),
            session.peer_name.clone(),
        )
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
                Err(_) => return Err(500),
            };
            hasher.update(&buf[..n]);
            size += n as u64;
        }
        let hash = hex::encode(hasher.finalize());
        if size != f.size || hash != f.sha256 {
            bad.push(i);
            continue;
        }
        drop(fh);
        // 重名自动加序号，兼容无扩展名文件。
        let name = std::path::Path::new(&f.name);
        let stem = name.file_stem().and_then(|v| v.to_str()).unwrap_or(&f.name);
        let extension = name.extension().and_then(|v| v.to_str());
        let mut dest = app.download_dir.join(&f.name);
        let mut n = 1;
        while dest.exists() {
            let candidate = match extension {
                Some(ext) => format!("{stem} ({n}).{ext}"),
                None => format!("{stem} ({n})"),
            };
            dest = app.download_dir.join(candidate);
            n += 1;
        }
        if tokio::fs::rename(&part, &dest).await.is_ok() {
            saved.push(dest.display().to_string());
        } else {
            bad.push(i);
        }
    }
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
    app.sessions.lock().unwrap().remove(token);

    let ok = bad.is_empty();
    (app.emitter)(
        "rx-done",
        serde_json::json!({
            "token": token, "peer": peer_name, "ok": ok,
            "saved": saved, "bad": bad,
        }),
    );
    if ok {
        Ok(serde_json::json!({"ok": true, "saved": saved}))
    } else {
        Err(409)
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

// HTTP 请求适配
async fn http_prepare(
    State(s): State<Arc<ServerShared>>,
    Json(req): Json<PrepareReq>,
) -> Result<Json<PrepareResp>, (StatusCode, String)> {
    core_prepare(&s.app, req)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))
}

async fn http_chunk(
    State(s): State<Arc<ServerShared>>,
    Path((token, file_idx, chunk_idx)): Path<(String, usize, u64)>,
    body: Bytes,
) -> StatusCode {
    match core_chunk(&s.app, &token, file_idx, chunk_idx, body).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(c) => StatusCode::from_u16(c).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn http_finalize(
    State(s): State<Arc<ServerShared>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    core_finalize(&s.app, &token)
        .await
        .map(Json)
        .map_err(|c| StatusCode::from_u16(c).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
}
