// 集成测试：HTTPS 服务 + 钉扎客户端 + 分块传输 + sha256 校验 + 断点续传
// 不依赖 GUI，直接驱动 server::start + sender::send。
use leapdrop::discovery::Peer;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

fn make_state(dir: &std::path::Path, port: u16) -> Arc<leapdrop::AppState> {
    let id = leapdrop::identity::load_or_create(dir).unwrap();
    let events: Arc<Mutex<Vec<(String, serde_json::Value)>>> = Arc::new(Mutex::new(vec![]));
    let ev_sink = events.clone();
    let emitter: leapdrop::Emitter = Arc::new(move |name, payload| {
        ev_sink.lock().unwrap().push((name.to_string(), payload));
    });
    Arc::new(leapdrop::AppState {
        identity: RwLock::new(id),
        port,
        download_dir: dir.join("downloads"),
        data_dir: dir.to_path_buf(),
        peers: Arc::new(Mutex::new(HashMap::new())),
        sessions: Mutex::new(HashMap::new()),
        emitter,
        discovery: std::sync::OnceLock::new(),
    })
}

fn peer_of(state: &Arc<leapdrop::AppState>, port: u16) -> Peer {
    let id = state.identity.read().unwrap();
    Peer {
        id: "test-instance".into(),
        name: id.name.clone(),
        os: id.os.clone(),
        fp: id.fingerprint.clone(),
        addr: "127.0.0.1".into(),
        port,
        first_seen: 0,
        last_seen: 0,
    }
}

fn rand_file(dir: &std::path::Path, name: &str, size: usize) -> std::path::PathBuf {
    use rand::RngCore;
    let mut data = vec![0u8; size];
    rand::thread_rng().fill_bytes(&mut data);
    let p = dir.join(name);
    std::fs::write(&p, data).unwrap();
    p
}

#[tokio::test]
async fn full_transfer_roundtrip() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tmp = tempdir();
    let recv_state = make_state(&tmp, 0);
    std::fs::create_dir_all(recv_state.download_dir.clone()).unwrap();

    // 启动接收端（端口 0 → 系统分配）
    let port = leapdrop::server::start(recv_state.clone()).await.unwrap();
    assert!(port > 0);

    // 发送端（另一份身份）
    let send_dir = tmp.join("sender-home");
    std::fs::create_dir_all(&send_dir).unwrap();
    let send_state = make_state(&send_dir, 0);

    let src = rand_file(&tmp, "测试文件.bin", 9 * 1024 * 1024 + 12345); // >2 个分块
    let peer = peer_of(&recv_state, port);

    // 接收端自动同意：后台线程轮询 sessions 并 respond
    let auto_state = recv_state.clone();
    let auto_thread = std::thread::spawn(move || loop {
        // 轮询 sessions，出现新会话直接同意
        let pending: Vec<String> = {
            let s = auto_state.sessions.lock().unwrap();
            s.iter()
                .filter(|(_, x)| x.decided.is_none())
                .map(|(k, _)| k.clone())
                .collect()
        };
        for t in pending {
            leapdrop::server::respond(&auto_state, &t, true);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        // 退出条件：下载目录出现目标文件
        if auto_state.download_dir.join("测试文件.bin").exists() {
            break;
        }
    });

    leapdrop::sender::send(&send_state, &peer, vec![src.display().to_string()])
        .await
        .expect("send failed");
    auto_thread.join().unwrap();

    // 校验落盘内容
    let got = std::fs::read(recv_state.download_dir.join("测试文件.bin")).unwrap();
    let want = std::fs::read(tmp.join("测试文件.bin")).unwrap();
    assert_eq!(got.len(), want.len());
    assert_eq!(got, want, "文件内容应完全一致");
}

#[tokio::test]
async fn reject_declines_transfer() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tmp = tempdir();
    let recv_state = make_state(&tmp, 0);
    std::fs::create_dir_all(recv_state.download_dir.clone()).unwrap();
    let port = leapdrop::server::start(recv_state.clone()).await.unwrap();

    let send_dir = tmp.join("sender-home");
    std::fs::create_dir_all(&send_dir).unwrap();
    let send_state = make_state(&send_dir, 0);
    let src = rand_file(&tmp, "被拒绝.txt", 100);
    let peer = peer_of(&recv_state, port);

    let auto_state = recv_state.clone();
    std::thread::spawn(move || loop {
        let pending: Vec<String> = {
            let s = auto_state.sessions.lock().unwrap();
            s.iter()
                .filter(|(_, x)| x.decided.is_none())
                .map(|(k, _)| k.clone())
                .collect()
        };
        for t in pending {
            leapdrop::server::respond(&auto_state, &t, false);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    });

    let err = leapdrop::sender::send(&send_state, &peer, vec![src.display().to_string()])
        .await
        .unwrap_err();
    assert!(err.contains("拒绝"), "错误信息应表明被拒绝：{err}");
}

fn tempdir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "leapdrop-test-{}-{:?}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}
