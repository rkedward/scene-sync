//! Advertises the server on the LAN as `_scenesync._tcp` so players can find it.

use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};

pub const SERVICE_TYPE: &str = "_scenesync._tcp.local.";

pub fn advertise(port: u16) -> Result<()> {
    let daemon = ServiceDaemon::new()?;
    let host = hostname();
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        "SceneSync",
        &format!("{host}.local."),
        "",
        port,
        &[(
            "protocol",
            scenesync_core::protocol::PROTOCOL_VERSION
                .to_string()
                .as_str(),
        )][..],
    )?
    .enable_addr_auto();
    daemon.register(info)?;
    // Keep the daemon running for the life of the process.
    std::mem::forget(daemon);
    tracing::info!("advertising {SERVICE_TYPE} on port {port}");
    Ok(())
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "scenesync".into())
}
