// 设备身份：自签 TLS 证书 + 指纹 + 设备密钥/ID + 持久化
use ring::signature::KeyPair as _;
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, TcpListener, UdpSocket};
use std::path::Path;

#[derive(Clone)]
pub struct Identity {
    pub name: String,
    pub os: String,
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
    /// sha256(cert_der) 的 hex。mDNS TXT 公布该指纹，
    /// 发送方连接时校验对端证书 DER 哈希与 TXT 一致（防中间人冒充）。
    pub fingerprint: String,
    /// 设备身份密钥（Ed25519，pkcs8 DER）——首次生成后永不变更
    pub device_pkcs8: Vec<u8>,
    /// 设备公钥（32 字节原始字节），配对时交给对端保存用于验签
    pub device_pk: Vec<u8>,
    /// 设备 ID：sha256(公钥) 前 4 字节 hex，形如 "3F7A-92C1"，人可读、全局稳定
    pub device_id: String,
}

fn os_name() -> String {
    std::env::consts::OS
        .replace("macos", "macOS")
        .replace("windows", "Windows")
        .replace("linux", "Linux")
        .replace("android", "Android")
        .replace("ios", "iOS")
}

fn default_name() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "LeapDrop 设备".into())
}

fn e2io(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}

/// 生成覆盖本机全部网卡 IP 的自签证书（IP SAN），供局域网内 https 直连
fn make_cert(key: &rcgen::KeyPair, ips: &[IpAddr]) -> io::Result<rcgen::Certificate> {
    let mut sans: Vec<String> = ips.iter().map(|ip| ip.to_string()).collect();
    sans.push("localhost".into());
    sans.push("leapdrop.local".into());
    let params = rcgen::CertificateParams::new(sans).map_err(e2io)?;
    params.self_signed(key).map_err(e2io)
}

/// 生成设备身份密钥对（Ed25519）与设备 ID
fn generate_device_key() -> io::Result<(Vec<u8>, Vec<u8>, String)> {
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng)
        .map_err(|_| io::Error::new(io::ErrorKind::Other, "生成设备密钥失败"))?;
    let kp = load_device_key(pkcs8.as_ref())?;
    let pk = kp.public_key().as_ref().to_vec();
    let id = device_id_of(&pk);
    Ok((pkcs8.as_ref().to_vec(), pk, id))
}

fn load_device_key(pkcs8: &[u8]) -> io::Result<ring::signature::Ed25519KeyPair> {
    ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "设备密钥无效"))
}

pub fn device_id_of(public_key: &[u8]) -> String {
    let h = hex::encode(Sha256::digest(public_key));
    format!("{}-{}", &h[..4].to_uppercase(), &h[4..8].to_uppercase())
}

fn save(file: &Path, id: &Identity, ips: &[IpAddr]) -> io::Result<()> {
    let v = serde_json::json!({
        "name": id.name,
        "fingerprint": id.fingerprint,
        "cert": hex::encode(&id.cert_der),
        "key": hex::encode(&id.key_der),
        "device_key": hex::encode(&id.device_pkcs8),
        "device_id": id.device_id,
        "ips": ips.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
    });
    fs::write(file, serde_json::to_vec_pretty(&v).unwrap())
}

/// 读取或创建身份。网卡 IP 集合变化时用原密钥重签证书（指纹随证书变化，mDNS 会重新公布）。
/// 设备密钥与 ID 永不变更（老身份文件没有设备密钥时补生成，ID 从此稳定）。
pub fn load_or_create(dir: &Path) -> io::Result<Identity> {
    fs::create_dir_all(dir)?;
    let file = dir.join("identity.json");
    let ips = local_ipv4s();

    if let Ok(text) = fs::read_to_string(&file) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            let name = v["name"].as_str().unwrap_or(&default_name()).to_string();
            let cert_hex = v["cert"].as_str().unwrap_or("");
            let key_hex = v["key"].as_str().unwrap_or("");
            let dkey_hex = v["device_key"].as_str().unwrap_or("");
            let saved_ips: Vec<String> = v["ips"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let now_ips: Vec<String> = ips.iter().map(|i| i.to_string()).collect();
            let same_net = saved_ips == now_ips;

            if let (Ok(cert_der), Ok(key_der), Ok(device_pkcs8)) =
                (hex::decode(cert_hex), hex::decode(key_hex), hex::decode(dkey_hex))
            {
                if !cert_der.is_empty() && !key_der.is_empty() && !device_pkcs8.is_empty() {
                    let (device_pk, device_id) = match load_device_key(&device_pkcs8) {
                        Ok(kp) => (kp.public_key().as_ref().to_vec(), device_id_of(kp.public_key().as_ref())),
                        Err(_) => {
                            let (_, pk, id) = generate_device_key()?;
                            (pk, id)
                        }
                    };
                    let fp = fingerprint_of(&cert_der);
                    if same_net {
                        return Ok(Identity {
                            name,
                            os: os_name(),
                            cert_der,
                            key_der,
                            fingerprint: fp,
                            device_pkcs8,
                            device_pk,
                            device_id,
                        });
                    }
                    // 网络变了：用原密钥重签证书，覆盖新 IP
                    if let Ok(key) = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(
                        &rustls_pki_types::PrivatePkcs8KeyDer::from(key_der.as_slice()),
                        &rcgen::PKCS_ECDSA_P256_SHA256,
                    ) {
                        if let Ok(cert) = make_cert(&key, &ips) {
                            let cert_der = cert.der().to_vec();
                            let id = Identity {
                                name,
                                os: os_name(),
                                fingerprint: fingerprint_of(&cert_der),
                                cert_der: cert_der.clone(),
                                key_der: key.serialize_der(),
                                device_pkcs8,
                                device_pk,
                                device_id,
                            };
                            save(&file, &id, &ips)?;
                            return Ok(id);
                        }
                    }
                    // 重签失败则退回旧证书（可能 SAN 不覆盖，直连会失败但服务可起）
                    return Ok(Identity {
                        name,
                        os: os_name(),
                        cert_der,
                        key_der,
                        fingerprint: fp,
                        device_pkcs8,
                        device_pk,
                        device_id,
                    });
                }
            }
        }
    }

    // 首次生成
    let key = rcgen::KeyPair::generate().map_err(e2io)?;
    let cert = make_cert(&key, &ips)?;
    let cert_der = cert.der().to_vec();
    let (device_pkcs8, device_pk, device_id) = generate_device_key()?;
    let id = Identity {
        name: default_name(),
        os: os_name(),
        cert_der: cert_der.clone(),
        key_der: key.serialize_der(),
        fingerprint: fingerprint_of(&cert_der),
        device_pkcs8,
        device_pk,
        device_id,
    };
    save(&file, &id, &ips)?;
    Ok(id)
}

/// 用设备密钥对消息签名（返回 64 字节签名）
pub fn sign(identity: &Identity, msg: &[u8]) -> Vec<u8> {
    let kp = load_device_key(&identity.device_pkcs8).expect("设备密钥必须可用");
    kp.sign(msg).as_ref().to_vec()
}

/// 用配对时保存的对端公钥验签
pub fn verify(peer_pk: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    let key = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, peer_pk);
    key.verify(msg, sig).is_ok()
}

pub fn fingerprint_of(cert_der: &[u8]) -> String {
    hex::encode(Sha256::digest(cert_der))
}

/// 仅更新设备显示名（保留证书与密钥）
pub fn save_name(dir: &Path, name: &str) -> io::Result<()> {
    let file = dir.join("identity.json");
    let text = fs::read_to_string(&file)?;
    let mut v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    v["name"] = serde_json::Value::String(name.to_string());
    fs::write(&file, serde_json::to_vec_pretty(&v).unwrap())
}

/// 枚举本机 IPv4（含回环，排除链路本地 169.254.*）
pub fn local_ipv4s() -> Vec<IpAddr> {
    let mut out = vec![];
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for a in ifaces {
            if let IpAddr::V4(v4) = a.ip() {
                if !v4.is_link_local() {
                    out.push(IpAddr::V4(v4));
                }
            }
        }
    }
    out.push(IpAddr::V4(Ipv4Addr::LOCALHOST));
    out
}

/// mDNS 注册用的本机 IPv4（优先非回环）
pub fn primary_ipv4() -> IpAddr {
    local_ipv4s()
        .into_iter()
        .find(|ip| !ip.is_loopback())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

/// 探测可绑定端口：优先 52920，被占用则由系统分配
pub fn pick_port(preferred: u16) -> io::Result<u16> {
    let try_bind = |p: u16| -> io::Result<u16> {
        TcpListener::bind(("0.0.0.0", p)).and_then(|l| l.local_addr()).map(|a| a.port())
    };
    try_bind(preferred).or_else(|_| try_bind(0))
}

#[allow(dead_code)]
pub fn probe_udp() -> io::Result<std::net::SocketAddr> {
    let s = UdpSocket::bind(("0.0.0.0", 0))?;
    Ok(s.local_addr()?)
}
