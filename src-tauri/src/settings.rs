// 本机接收偏好：默认允许，设置写入系统应用数据目录。
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::{io, path::Path};

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub receive_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            receive_enabled: true,
        }
    }
}

pub fn load(data_dir: &Path) -> io::Result<Settings> {
    match std::fs::read(data_dir.join("settings.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(e),
    }
}

pub fn set_receive_enabled(app: &AppState, enabled: bool) -> Result<(), String> {
    // 与创建新接收会话共用锁，确保关闭后不会再弹出新的确认框。
    let mut settings = app.settings.lock().unwrap();
    let next = Settings {
        receive_enabled: enabled,
    };
    let bytes = serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?;
    std::fs::write(app.data_dir.join("settings.json"), bytes)
        .map_err(|e| format!("保存接收设置失败：{e}"))?;
    *settings = next;
    if !enabled {
        let mut sessions = app.sessions.lock().unwrap();
        for (token, session) in sessions.iter_mut() {
            if session.decided.is_none() {
                session.decided = Some(false);
                session.notify.notify_one();
                (app.emitter)("incoming-closed", serde_json::json!({"token": token}));
            }
        }
    }
    (app.emitter)(
        "receive-setting-changed",
        serde_json::json!({"enabled": enabled}),
    );
    Ok(())
}
