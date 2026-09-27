// mDNS/DNS-SD 设备发现：注册本机服务 + 监听局域网内对端，推送 peers 事件
use crate::identity::Identity;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::Serialize;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const SERVICE_TYPE: &str = "_leapdrop._tcp.local.";

#[derive(Serialize, Clone, Debug)]
pub struct Peer {
    pub id: String, // service instance fullname
    pub device_id: String, // 对外设备 ID（来自 mDNS TXT）
    pub name: String,
    pub os: String,
    pub fp: String,
    pub addr: String,
    pub port: u16,
    pub first_seen: u64,
    pub last_seen: u64,
}

pub struct Discovery {
    pub daemon: ServiceDaemon,
    pub instance: String,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 启动发现服务；peers 变化时调用 emitter 推全量列表
pub fn start(
    identity: &Identity,
    port: u16,
    peers: Arc<Mutex<HashMap<String, Peer>>>,
    trusted: Arc<Mutex<Vec<crate::protocol::TrustedPeer>>>,
    emitter: crate::Emitter,
) -> std::io::Result<Discovery> {
    let daemon = ServiceDaemon::new().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("mDNS daemon: {e}"))
    })?;

    let instance = format!("leapdrop-{}", &identity.fingerprint[..12]);
    let host = format!("{instance}.local.");
    let mut props = HashMap::new();
    props.insert("v".to_string(), crate::protocol::VERSION.to_string());
    props.insert("fp".to_string(), identity.fingerprint.clone());
    props.insert("name".to_string(), identity.name.clone());
    props.insert("os".to_string(), identity.os.clone());
    props.insert("id".to_string(), identity.device_id.clone());

    let ips_csv = crate::identity::local_ipv4s()
        .into_iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let info = ServiceInfo::new(SERVICE_TYPE, &instance, &host, ips_csv.as_str(), port, Some(props))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("mDNS info: {e}")))?;
    daemon
        .register(info)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("mDNS register: {e}")))?;

    let receiver = daemon
        .browse(SERVICE_TYPE)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("mDNS browse: {e}")))?;

    let my_fp = identity.fingerprint.clone();
    let my_fullname = format!("{instance}.{SERVICE_TYPE}");
    let trusted = trusted;

    // 监听循环：解析/移除对端服务，推送 UI（阻塞式 recv，独立线程）
    std::thread::spawn(move || loop {
        match receiver.recv() {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let rs = info.as_resolved_service();
                let Some(fp) = rs.txt_properties.get_property_val_str("fp") else {
                    continue;
                };
                if fp == my_fp || rs.fullname == my_fullname {
                    continue; // 自己
                }
                let addr = pick_addr(&rs.addresses);
                let peer = Peer {
                    id: rs.fullname.clone(),
                    device_id: rs
                        .txt_properties
                        .get_property_val_str("id")
                        .unwrap_or("")
                        .to_string(),
                    name: rs
                        .txt_properties
                        .get_property_val_str("name")
                        .unwrap_or("未知设备")
                        .to_string(),
                    os: rs
                        .txt_properties
                        .get_property_val_str("os")
                        .unwrap_or("")
                        .to_string(),
                    fp: fp.to_string(),
                    addr,
                    port: rs.port,
                    first_seen: peers
                        .lock()
                        .ok()
                        .and_then(|m| m.get(&rs.fullname).map(|p| p.first_seen))
                        .unwrap_or_else(now),
                    last_seen: now(),
                };
                peers.lock().ok().map(|mut m| m.insert(peer.id.clone(), peer));
                emit_peers(&peers, &trusted, &emitter);
            }
            Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                let removed = peers.lock().ok().and_then(|mut m| m.remove(&fullname)).is_some();
                if removed {
                    emit_peers(&peers, &trusted, &emitter);
                }
            }
            Ok(_) => {}
            Err(_) => {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    });

    Ok(Discovery { daemon, instance })
}

fn pick_addr(addrs: &std::collections::HashSet<IpAddr>) -> String {
    let mut list: Vec<&IpAddr> = addrs.iter().collect();
    list.sort();
    // 优先非回环 IPv4
    list.iter()
        .find(|a| a.is_ipv4() && !a.is_loopback())
        .or(list.first())
        .map(|a| a.to_string())
        .unwrap_or_else(|| "127.0.0.1".into())
}

pub fn emit_peers(
    peers: &Arc<Mutex<HashMap<String, Peer>>>,
    trusted: &Arc<Mutex<Vec<crate::protocol::TrustedPeer>>>,
    emitter: &crate::Emitter,
) {
    if let Ok(m) = peers.lock() {
        let trusted_ids: Vec<String> = trusted
            .lock()
            .map(|t| t.iter().map(|x| x.id.clone()).collect())
            .unwrap_or_default();
        let mut list: Vec<serde_json::Value> = m
            .values()
            .map(|p| {
                let mut v = serde_json::to_value(p).unwrap_or_default();
                v["paired"] = serde_json::Value::Bool(trusted_ids.contains(&p.device_id));
                v
            })
            .collect();
        list.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        emitter("peers", serde_json::Value::Array(list));
    }
}
