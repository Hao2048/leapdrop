// 集成测试：免配对 HTTPS 文件传输、接收确认与本机接收设置。
// 不依赖 GUI，直接驱动 server::start + sender::send。
use leapdrop::discovery::Peer;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

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
        settings: Mutex::new(leapdrop::settings::load(dir).unwrap()),
        emitter,
        discovery: std::sync::OnceLock::new(),
    })
}

fn peer_of(state: &Arc<leapdrop::AppState>, port: u16) -> Peer {
    let id = state.identity.read().unwrap();
    Peer {
        id: "test-instance".into(),
        device_id: id.device_id.clone(),
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

// 测试数据只写系统临时目录，退出时清理；后台任务随测试结束取消。
struct TestDir(std::path::PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("leapdrop-test-{}", leapdrop::random_token()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct AutoRespond(tokio::task::JoinHandle<()>);

impl Drop for AutoRespond {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn auto_respond(state: Arc<leapdrop::AppState>, receive: bool) -> AutoRespond {
    AutoRespond(tokio::spawn(async move {
        loop {
            let tokens: Vec<_> = state
                .sessions
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, s)| s.decided.is_none())
                .map(|(t, _)| t.clone())
                .collect();
            for token in tokens {
                leapdrop::server::respond(&state, &token, receive);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }))
}

async fn send(
    state: &Arc<leapdrop::AppState>,
    peer: &Peer,
    paths: &[std::path::PathBuf],
) -> Result<String, String> {
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        leapdrop::sender::send(
            state,
            peer,
            paths.iter().map(|p| p.display().to_string()).collect(),
        ),
    )
    .await
    .expect("传输应在 15 秒内结束")
}

#[tokio::test]
async fn full_transfer_roundtrip() {
    let tmp = TestDir::new();
    let receiver = make_state(&tmp.0.join("receiver"), 0);
    std::fs::create_dir_all(&receiver.download_dir).unwrap();
    let port = leapdrop::server::start(receiver.clone()).await.unwrap();
    let sender = make_state(&tmp.0.join("sender"), 0);
    sender.identity.write().unwrap().name = "测试 Mac 电脑".into();
    let peer = peer_of(&receiver, port);
    let _auto = auto_respond(receiver.clone(), true);
    let large = rand_file(&tmp.0, "测试文件.bin", 9 * 1024 * 1024 + 12345);
    let empty = rand_file(&tmp.0, "空文件.txt", 0);
    let no_ext = rand_file(&tmp.0, "LICENSE", 32);
    std::fs::write(receiver.download_dir.join("LICENSE"), b"original").unwrap();
    std::fs::write(receiver.download_dir.join("测试文件.bin"), b"original").unwrap();

    send(&sender, &peer, &[large.clone(), empty, no_ext.clone()])
        .await
        .unwrap();

    assert_eq!(
        std::fs::read(receiver.download_dir.join("测试文件 (1).bin")).unwrap(),
        std::fs::read(large).unwrap()
    );
    assert_eq!(
        std::fs::read(receiver.download_dir.join("LICENSE (1)")).unwrap(),
        std::fs::read(no_ext).unwrap()
    );
    assert_eq!(
        std::fs::read(receiver.download_dir.join("LICENSE")).unwrap(),
        b"original"
    );
    assert_eq!(
        std::fs::read(receiver.download_dir.join("测试文件.bin")).unwrap(),
        b"original"
    );
    assert_eq!(
        std::fs::metadata(receiver.download_dir.join("空文件.txt"))
            .unwrap()
            .len(),
        0
    );
    assert!(receiver.sessions.lock().unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(receiver.download_dir.join(".leapdrop-tmp"))
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn device_can_decline_file() {
    let tmp = TestDir::new();
    let receiver = make_state(&tmp.0.join("receiver"), 0);
    std::fs::create_dir_all(&receiver.download_dir).unwrap();
    let port = leapdrop::server::start(receiver.clone()).await.unwrap();
    let sender = make_state(&tmp.0.join("sender"), 0);
    let _auto = auto_respond(receiver.clone(), false);
    let src = rand_file(&tmp.0, "拒收.txt", 100);

    let error = send(&sender, &peer_of(&receiver, port), &[src])
        .await
        .unwrap_err();

    assert!(error.contains("拒绝了传输"), "{error}");
    assert!(receiver.sessions.lock().unwrap().is_empty());
    assert!(!receiver.download_dir.join("拒收.txt").exists());
    assert_eq!(
        std::fs::read_dir(receiver.download_dir.join(".leapdrop-tmp"))
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn incorrect_certificate_fingerprint_is_rejected() {
    let tmp = TestDir::new();
    let receiver = make_state(&tmp.0.join("receiver"), 0);
    let port = leapdrop::server::start(receiver.clone()).await.unwrap();
    let sender = make_state(&tmp.0.join("sender"), 0);
    let mut peer = peer_of(&receiver, port);
    peer.fp = "00".repeat(32);
    let src = rand_file(&tmp.0, "test.txt", 10);

    assert!(send(&sender, &peer, &[src]).await.is_err());
    assert!(receiver.sessions.lock().unwrap().is_empty());
}

#[tokio::test]
async fn receive_setting_defaults_on_and_persists() {
    let tmp = TestDir::new();
    let receiver = make_state(&tmp.0.join("receiver"), 0);
    assert!(receiver.settings.lock().unwrap().receive_enabled);
    leapdrop::settings::set_receive_enabled(&receiver, false).unwrap();
    assert!(
        !make_state(&receiver.data_dir, 0)
            .settings
            .lock()
            .unwrap()
            .receive_enabled
    );
    leapdrop::settings::set_receive_enabled(&receiver, true).unwrap();
    assert!(
        make_state(&receiver.data_dir, 0)
            .settings
            .lock()
            .unwrap()
            .receive_enabled
    );
}

#[tokio::test]
async fn disabled_receiver_rejects_immediately_then_can_receive_again() {
    let tmp = TestDir::new();
    let receiver = make_state(&tmp.0.join("receiver"), 0);
    std::fs::create_dir_all(&receiver.download_dir).unwrap();
    let port = leapdrop::server::start(receiver.clone()).await.unwrap();
    let sender = make_state(&tmp.0.join("sender"), 0);
    let peer = peer_of(&receiver, port);
    let src = rand_file(&tmp.0, "test.txt", 32);
    leapdrop::settings::set_receive_enabled(&receiver, false).unwrap();
    let error = send(&sender, &peer, &[src.clone()]).await.unwrap_err();
    assert!(error.contains("关闭文件接收"), "{error}");
    assert!(receiver.sessions.lock().unwrap().is_empty());
    assert!(!receiver.download_dir.join(".leapdrop-tmp").exists());
    leapdrop::settings::set_receive_enabled(&receiver, true).unwrap();
    // 发送方关闭接收时仍然可以发送。
    leapdrop::settings::set_receive_enabled(&sender, false).unwrap();
    let _auto = auto_respond(receiver.clone(), true);
    send(&sender, &peer, &[src]).await.unwrap();
    assert!(receiver.download_dir.join("test.txt").exists());
}

#[tokio::test]
async fn disabling_receiver_declines_pending_request() {
    let tmp = TestDir::new();
    let receiver = make_state(&tmp.0.join("receiver"), 0);
    std::fs::create_dir_all(&receiver.download_dir).unwrap();
    let port = leapdrop::server::start(receiver.clone()).await.unwrap();
    let sender = make_state(&tmp.0.join("sender"), 0);
    let peer = peer_of(&receiver, port);
    let src = rand_file(&tmp.0, "pending.txt", 32);
    let task = tokio::spawn(async move { send(&sender, &peer, &[src]).await });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while receiver.sessions.lock().unwrap().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let token = receiver
        .sessions
        .lock()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    leapdrop::settings::set_receive_enabled(&receiver, false).unwrap();
    // 关闭后迟到的“接受”不能重新放行待确认请求。
    leapdrop::server::respond(&receiver, &token, true);
    let error = task.await.unwrap().unwrap_err();
    assert!(error.contains("关闭文件接收"), "{error}");
    assert!(receiver.sessions.lock().unwrap().is_empty());
    assert!(!receiver.download_dir.join("pending.txt").exists());
}

#[tokio::test]
async fn failed_settings_save_keeps_previous_value() {
    let tmp = TestDir::new();
    let receiver = make_state(&tmp.0.join("receiver"), 0);
    std::fs::create_dir(receiver.data_dir.join("settings.json")).unwrap();
    assert!(leapdrop::settings::set_receive_enabled(&receiver, false).is_err());
    assert!(receiver.settings.lock().unwrap().receive_enabled);
}

#[tokio::test]
async fn disabling_receive_does_not_interrupt_accepted_transfer() {
    use leapdrop::protocol::{FileMeta, PrepareReq, CHUNK_SIZE};
    use sha2::{Digest, Sha256};
    let tmp = TestDir::new();
    let receiver = make_state(&tmp.0.join("receiver"), 0);
    std::fs::create_dir_all(&receiver.download_dir).unwrap();
    let body = b"accepted transfer";
    let req = PrepareReq {
        sender_name: "附近的电脑".into(),
        total_bytes: body.len() as u64,
        files: vec![FileMeta {
            name: "accepted.txt".into(),
            size: body.len() as u64,
            sha256: hex::encode(Sha256::digest(body)),
            chunk_size: CHUNK_SIZE,
            chunks: 1,
        }],
    };
    let _auto = auto_respond(receiver.clone(), true);
    let response = leapdrop::server::core_prepare(&receiver, req)
        .await
        .unwrap();
    assert!(response.accepted);
    leapdrop::settings::set_receive_enabled(&receiver, false).unwrap();
    leapdrop::server::core_chunk(&receiver, &response.token, 0, 0, body.to_vec().into())
        .await
        .unwrap();
    leapdrop::server::core_finalize(&receiver, &response.token)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(receiver.download_dir.join("accepted.txt")).unwrap(),
        body
    );
}
