// 发送端：证书钉扎的 HTTPS 客户端 + 分块上传 + 进度事件
use crate::discovery::Peer;
use crate::protocol::*;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 校验"对端证书 DER == mDNS TXT 公布的指纹"，其余信任全部由这条钉扎保证
#[derive(Debug)]
struct PinVerifier {
    expected: Vec<u8>, // sha256(cert_der)
}

impl rustls::client::danger::ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls_pki_types::CertificateDer<'_>,
        _intermediates: &[rustls_pki_types::CertificateDer<'_>],
        _server_name: &rustls_pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls_pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let digest: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if digest.as_slice() == self.expected.as_slice() {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::UnknownIssuer,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls_pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls_pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

fn pinned_client(expected_fp: &str) -> Result<reqwest::Client, String> {
    let expected = hex::decode(expected_fp).map_err(|e| format!("指纹解码失败: {e}"))?;
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinVerifier { expected }))
        .with_no_client_auth();
    reqwest::Client::builder()
        .use_preconfigured_tls(config)
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| format!("构建客户端失败: {e}"))
}

/// 发起配对并等待对端用户确认；成功后把对端加入本机信任列表
async fn run_pair(
    app: &Arc<crate::AppState>,
    client: &reqwest::Client,
    base: &str,
    headers: &reqwest::header::HeaderMap,
    my_id: &str,
    my_name: &str,
    my_pk: &str,
) -> Result<bool, String> {
    use base64::Engine as _;
    let body = serde_json::to_string(&PairReq {
        device_id: my_id.to_string(),
        name: my_name.to_string(),
        pk: my_pk.to_string(),
    })
    .map_err(|e| e.to_string())?;
    let sig = base64::engine::general_purpose::STANDARD
        .encode(crate::identity::sign(&app.identity.read().unwrap(), body.as_bytes()));
    let mut h = headers.clone();
    h.insert(
        reqwest::header::HeaderName::from_static("x-leapdrop-sig"),
        reqwest::header::HeaderValue::from_str(&sig).map_err(|_| "签名编码失败".to_string())?,
    );
    let resp: PairResp = client
        .post(format!("{base}/api/pair"))
        .headers(h)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .timeout(Duration::from_secs(75))
        .send()
        .await
        .map_err(|e| format!("配对请求失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("配对请求被拒: {e}"))?
        .json()
        .await
        .map_err(|e| format!("解析配对响应失败: {e}"))?;
    if !resp.accepted {
        return Ok(false);
    }
    // 记住对端（以其响应中的 ID/公钥为准）
    crate::add_trusted(
        app,
        TrustedPeer {
            id: resp.device_id.clone(),
            name: resp.name.clone(),
            pk: resp.pk.clone(),
            paired_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        },
    );
    (app.emitter)(
        "paired",
        serde_json::json!({"deviceId": resp.device_id, "name": resp.name}),
    );
    Ok(true)
}

/// 计算文件 sha256（流式）
async fn file_sha256(path: &str) -> Result<(u64, String), String> {
    use tokio::io::AsyncReadExt;
    let mut f = tokio::fs::File::open(path).await.map_err(|e| format!("打开失败: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf).await.map_err(|e| format!("读取失败: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((size, hex::encode(hasher.finalize())))
}

/// 发送一组文件到对端。进度/结果通过 emitter 事件汇报。
pub async fn send(
    app: &Arc<crate::AppState>,
    peer: &Peer,
    paths: Vec<String>,
) -> Result<String, String> {
    // 1. 汇总文件元信息
    let mut metas = Vec::new();
    for p in &paths {
        let (size, sha) = file_sha256(p).await?;
        let name = std::path::Path::new(p)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        metas.push(FileMeta {
            name,
            size,
            sha256: sha,
            chunk_size: CHUNK_SIZE,
            chunks: chunk_count(size),
        });
    }
    let total_bytes: u64 = metas.iter().map(|m| m.size).sum();

    let base = format!("https://{}:{}", peer.addr, peer.port);
    let client = pinned_client(&peer.fp)?;
    let (my_id, my_name, my_pk) = {
        let id = app.identity.read().unwrap();
        (
            id.device_id.clone(),
            id.name.clone(),
            base64::engine::general_purpose::STANDARD.encode(&id.device_pk),
        )
    };
    use base64::Engine as _;
    let headers = reqwest::header::HeaderMap::from_iter([
        (
            reqwest::header::HeaderName::from_static("x-leapdrop-name"),
            reqwest::header::HeaderValue::from_str(&my_name)
                .map_err(|_| "设备名含非法字符".to_string())?,
        ),
        (
            reqwest::header::HeaderName::from_static("x-leapdrop-id"),
            reqwest::header::HeaderValue::from_str(&my_id)
                .map_err(|_| "设备 ID 非法".to_string())?,
        ),
    ]);

    // 2. prepare（带设备签名；未配对则自动发起配对，成功后重试一次）
    let prepare_body = serde_json::to_string(&PrepareReq {
        files: metas.clone(),
        total_bytes,
    })
    .map_err(|e| e.to_string())?;
    let mut paired_once = false;
    let resp: PrepareResp = loop {
        let sig = base64::engine::general_purpose::STANDARD
            .encode(crate::identity::sign(&app.identity.read().unwrap(), prepare_body.as_bytes()));
        let mut req_headers = headers.clone();
        req_headers.insert(
            reqwest::header::HeaderName::from_static("x-leapdrop-sig"),
            reqwest::header::HeaderValue::from_str(&sig)
                .map_err(|_| "签名编码失败".to_string())?,
        );
        let http = client
            .post(format!("{base}/api/prepare"))
            .headers(req_headers)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(prepare_body.clone())
            .timeout(Duration::from_secs(100))
            .send()
            .await
            .map_err(|e| format!("连接对端失败: {e}"))?;
        if http.status().as_u16() == 403 {
            let np: NeedPairing = http.json().await.map_err(|e| format!("解析响应失败: {e}"))?;
            if np.error != "need_pairing" {
                return Err("对端拒绝了请求（版本不兼容，请双方都升级到 v0.2+）".into());
            }
            if paired_once {
                return Err("配对后仍被拒绝，请对端检查设备列表".into());
            }
            paired_once = true;
            // 自动发起配对（对端弹确认框）
            (app.emitter)(
                "tx-pairing",
                serde_json::json!({"peer": peer.name, "peerId": np.device_id, "peerName": np.name}),
            );
            let ok = run_pair(app, &client, &base, &headers, &my_id, &my_name, &my_pk).await?;
            if !ok {
                (app.emitter)(
                    "tx-rejected",
                    serde_json::json!({"peer": peer.name, "message": "对方拒绝了配对请求"}),
                );
                return Err("对方拒绝了配对".into());
            }
            continue; // 配对成功 → 重试 prepare
        }
        let resp: PrepareResp = http
            .error_for_status()
            .map_err(|e| format!("对端拒绝请求: {e}"))?
            .json()
            .await
            .map_err(|e| format!("解析响应失败: {e}"))?;
        break resp;
    };

    if !resp.accepted {
        (app.emitter)(
            "tx-rejected",
            serde_json::json!({"peer": peer.name, "message": resp.message}),
        );
        return Err("对方拒绝了传输".into());
    }
    let token = resp.token.clone();

    // 3. 逐文件分块上传（跳过对端已有分块 → 断点续传）
    let mut t0 = Instant::now();
    let mut bytes_done = 0u64;
    let mut bytes_last = 0u64;
    for (idx, meta) in metas.iter().enumerate() {
        let have: Vec<u64> = resp.resume.get(&idx).cloned().unwrap_or_default();
        let mut f = tokio::fs::File::open(&paths[idx])
            .await
            .map_err(|e| format!("打开文件失败: {e}"))?;
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let last = meta.chunks.saturating_sub(1);
        let last_size = meta.size - last * CHUNK_SIZE;
        for c in 0..meta.chunks {
            let expect = if c == last { last_size as usize } else { CHUNK_SIZE as usize };
            if have.contains(&c) {
                // 已在对端：seek 跳过并计进度
                f.seek(std::io::SeekFrom::Current(expect as i64))
                    .await
                    .map_err(|e| format!("跳块失败: {e}"))?;
                bytes_done += expect as u64;
                continue;
            }
            let mut buf = vec![0u8; expect];
            f.read_exact(&mut buf).await.map_err(|e| format!("读取分块失败: {e}"))?;
            let url = format!("{base}/api/chunk/{token}/{idx}/{c}");
            let status = client
                .put(&url)
                .headers(headers.clone())
                .body(buf)
                .timeout(Duration::from_secs(60))
                .send()
                .await
                .map_err(|e| format!("上传中断: {e}"))?
                .status();
            if !status.is_success() {
                return Err(format!("对端写入分块失败（HTTP {status}），稍后可续传"));
            }
            bytes_done += expect as u64;

            // 进度事件节流 250ms
            if t0.elapsed() >= Duration::from_millis(250) {
                let dt = t0.elapsed().as_secs_f64().max(0.001);
                let speed = (bytes_done - bytes_last) as f64 / dt;
                (app.emitter)(
                    "tx-progress",
                    serde_json::json!({
                        "token": token, "peer": peer.name,
                        "fileIdx": idx, "fileName": meta.name,
                        "chunk": c, "chunksTotal": meta.chunks,
                        "bytesDone": bytes_done, "totalBytes": total_bytes,
                        "speedBps": speed,
                    }),
                );
                bytes_last = bytes_done;
                t0 = Instant::now();
            }
        }
    }

    // 4. finalize（对端校验 sha256 并落盘）
    let fin = client
        .post(format!("{base}/api/finalize/{token}"))
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("结束请求失败: {e}"))?;
    if !fin.status().is_success() {
        return Err("对端校验失败（sha256 不匹配）".into());
    }

    (app.emitter)(
        "tx-done",
        serde_json::json!({"token": token, "peer": peer.name, "bytes": total_bytes}),
    );
    Ok(token)
}
