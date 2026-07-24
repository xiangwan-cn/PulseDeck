use std::net::{SocketAddr, UdpSocket};

use crate::model::card_model::{CardValue, StatusLevel};
use crate::model::metric_result::{MetricResult, MetricState};

use super::traits::MetricContext;

/// Persistent NetworkManager D-Bus reader. Unlike the previous implementation,
/// one card refresh never forks three separate `nmcli` processes.
pub struct NetworkMetric {
    connection: Option<zbus::blocking::Connection>,
}

impl NetworkMetric {
    pub fn new() -> Self {
        Self {
            connection: zbus::blocking::Connection::system().ok(),
        }
    }

    pub fn collect(&mut self, _ctx: &MetricContext) -> MetricResult {
        let (connectivity, connection_name) = self
            .connection
            .as_ref()
            .and_then(network_manager_state)
            .unwrap_or_else(fallback_state);
        let ip = primary_ip();
        let (state_label, level) = match connectivity {
            4 => ("已连接", StatusLevel::Good),
            2 => ("需登录", StatusLevel::Warning),
            3 => ("受限", StatusLevel::Warning),
            1 => ("未连接", StatusLevel::Critical),
            _ => ("未知", StatusLevel::Unknown),
        };
        let (value, subtitle) = network_presentation(state_label, level, &connection_name, &ip);
        MetricResult {
            value,
            subtitle,
            tooltip: Some(format!(
                "NetworkManager 连通性: {} · 连接: {} · IP: {}",
                connectivity_label(connectivity),
                connection_name,
                ip
            )),
            state: MetricState::Normal,
            cached: false,
            metadata: None,
        }
    }
}

fn network_presentation(
    state_label: &str,
    level: StatusLevel,
    connection_name: &str,
    ip: &str,
) -> (CardValue, Option<String>) {
    let value = CardValue::Status {
        label: if ip.is_empty() { state_label } else { ip }.into(),
        level: if ip.is_empty() {
            level
        } else {
            StatusLevel::Normal
        },
    };
    let subtitle = match (state_label.is_empty(), connection_name.is_empty()) {
        (false, false) => Some(format!("{state_label} · {connection_name}")),
        (false, true) => Some(state_label.into()),
        (true, false) => Some(connection_name.into()),
        (true, true) => None,
    };
    (value, subtitle)
}

fn network_manager_state(connection: &zbus::blocking::Connection) -> Option<(u32, String)> {
    let manager = zbus::blocking::Proxy::new(
        connection,
        "org.freedesktop.NetworkManager",
        "/org/freedesktop/NetworkManager",
        "org.freedesktop.NetworkManager",
    )
    .ok()?;
    let connectivity = manager.get_property::<u32>("Connectivity").ok()?;
    let primary = manager
        .get_property::<zbus::zvariant::OwnedObjectPath>("PrimaryConnection")
        .ok()?;
    let name = if primary.as_str() == "/" {
        String::new()
    } else {
        zbus::blocking::Proxy::new(
            connection,
            "org.freedesktop.NetworkManager",
            primary.as_str(),
            "org.freedesktop.NetworkManager.Connection.Active",
        )
        .ok()
        .and_then(|proxy| proxy.get_property::<String>("Id").ok())
        .unwrap_or_default()
    };
    Some((connectivity, name))
}

fn fallback_state() -> (u32, String) {
    let Ok(entries) = std::fs::read_dir("/sys/class/net") else {
        return (0, String::new());
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "lo" {
            continue;
        }
        if std::fs::read_to_string(entry.path().join("operstate"))
            .ok()
            .is_some_and(|state| state.trim() == "up")
        {
            return (4, name);
        }
    }
    (1, String::new())
}

fn primary_ip() -> String {
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return String::new();
    };
    if socket.connect("1.1.1.1:80").is_err() {
        return String::new();
    }
    match socket.local_addr() {
        Ok(SocketAddr::V4(address)) => address.ip().to_string(),
        _ => String::new(),
    }
}

fn connectivity_label(value: u32) -> &'static str {
    match value {
        4 => "full",
        3 => "limited",
        2 => "portal",
        1 => "none",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use crate::model::card_model::{CardValue, StatusLevel};

    use super::{connectivity_label, network_presentation};

    #[test]
    fn network_manager_connectivity_values_are_stable() {
        assert_eq!(connectivity_label(4), "full");
        assert_eq!(connectivity_label(2), "portal");
        assert_eq!(connectivity_label(1), "none");
    }

    #[test]
    fn ip_is_the_primary_network_value() {
        let (value, subtitle) =
            network_presentation("已连接", StatusLevel::Good, "Home Wi-Fi", "192.168.1.8");

        assert!(matches!(
            value,
            CardValue::Status { label, level }
                if label == "192.168.1.8" && level == StatusLevel::Normal
        ));
        assert_eq!(subtitle.as_deref(), Some("已连接 · Home Wi-Fi"));
    }

    #[test]
    fn network_state_remains_visible_without_an_ip() {
        let (value, subtitle) = network_presentation("未连接", StatusLevel::Critical, "", "");

        assert!(matches!(
            value,
            CardValue::Status { label, level }
                if label == "未连接" && level == StatusLevel::Critical
        ));
        assert_eq!(subtitle.as_deref(), Some("未连接"));
    }
}
