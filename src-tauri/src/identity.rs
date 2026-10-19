// 设备身份：自签 TLS 证书、指纹、显示名称与设备 ID
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, TcpListener};
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
    /// 稳定的本机标识，用于区分同名设备。
    pub device_id: String,
}

fn os_name() -> String {
    std::env::consts::OS
        .replace("macos", "macOS")
        .replace("windows", "Windows")
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

fn save(file: &Path, id: &Identity, ips: &[IpAddr]) -> io::Result<()> {
    let v = serde_json::json!({
        "name": id.name,
        "fingerprint": id.fingerprint,
        "cert": hex::encode(&id.cert_der),
        "key": hex::encode(&id.key_der),
        "device_id": id.device_id,
        "ips": ips.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
    });
    fs::write(file, serde_json::to_vec_pretty(&v).unwrap())
}

/// 读取或创建身份；保留旧版本的设备名称和 ID，网卡变化时重签证书。
pub fn load_or_create(dir: &Path) -> io::Result<Identity> {
    fs::create_dir_all(dir)?;
    let file = dir.join("identity.json");
    let saved: serde_json::Value = fs::read_to_string(&file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let ips = local_ipv4s();
    let now_ips: Vec<String> = ips.iter().map(|i| i.to_string()).collect();
    let saved_ips: Vec<String> = saved["ips"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let key_der = hex::decode(saved["key"].as_str().unwrap_or("")).unwrap_or_default();
    let key = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(
        &rustls_pki_types::PrivatePkcs8KeyDer::from(key_der.as_slice()),
        &rcgen::PKCS_ECDSA_P256_SHA256,
    )
    .or_else(|_| rcgen::KeyPair::generate())
    .map_err(e2io)?;
    let old_cert = hex::decode(saved["cert"].as_str().unwrap_or("")).unwrap_or_default();
    let cert_der = if saved_ips == now_ips && !old_cert.is_empty() && key.serialize_der() == key_der
    {
        old_cert
    } else {
        make_cert(&key, &ips)?.der().to_vec()
    };
    let device_id = saved["device_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| {
            let token = crate::random_token().to_uppercase();
            format!("{}-{}", &token[..4], &token[4..8])
        });
    let id = Identity {
        name: saved["name"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(default_name),
        os: os_name(),
        fingerprint: fingerprint_of(&cert_der),
        cert_der,
        key_der: key.serialize_der(),
        device_id,
    };
    save(&file, &id, &ips)?;
    Ok(id)
}

pub fn fingerprint_of(cert_der: &[u8]) -> String {
    hex::encode(Sha256::digest(cert_der))
}

/// 仅更新设备显示名（保留证书与密钥）
pub fn save_name(dir: &Path, name: &str) -> io::Result<()> {
    let file = dir.join("identity.json");
    let text = fs::read_to_string(&file)?;
    let mut v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
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

/// 探测可绑定端口：优先 52920，被占用则由系统分配
pub fn pick_port(preferred: u16) -> io::Result<u16> {
    let try_bind = |p: u16| -> io::Result<u16> {
        TcpListener::bind(("0.0.0.0", p))
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
    };
    try_bind(preferred).or_else(|_| try_bind(0))
}
