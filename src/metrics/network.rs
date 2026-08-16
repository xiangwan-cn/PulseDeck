use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

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

/// The main-table default route is a better source hint than a UDP probe:
/// on systems with a proxy tunnel (e.g. 198.18.0.0/15 virtual stacks) the
/// kernel picks the tunnel address for outbound packets, while the user
/// usually wants the physical interface address (e.g. Wi-Fi 192.168.0.x).
fn primary_ip() -> String {
    let preferred = default_route_iface();
    let mut fallback = None;
    if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "lo" {
                continue;
            }
            let Some(ip) = iface_ipv4(&name) else {
                continue;
            };
            if is_unwanted_ip(ip) {
                continue;
            }
            if preferred.as_deref() == Some(name.as_str()) {
                return ip.to_string();
            }
            fallback.get_or_insert(ip);
        }
    }
    if let Some(ip) = fallback {
        return ip.to_string();
    }
    udp_probe_ip()
}

/// IPv4 addresses that should never be presented as "the" address:
/// loopback, link-local, unspecified, and the 198.18.0.0/15 benchmarking
/// block commonly used by proxy tunnel stacks.
fn is_unwanted_ip(ip: Ipv4Addr) -> bool {
    ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || matches!(ip.octets(), [198, 18..=19, _, _])
}

/// Name of the interface owning the main-table default route (lowest metric).
fn default_route_iface() -> Option<String> {
    let contents = std::fs::read_to_string("/proc/net/route").ok()?;
    default_route_iface_from(&contents)
}

fn default_route_iface_from(contents: &str) -> Option<String> {
    let mut best: Option<(u32, String)> = None;
    for line in contents.lines().skip(1) {
        let mut fields = line.split_whitespace();
        let iface = fields.next()?;
        let destination = u32::from_str_radix(fields.next()?, 16).ok()?;
        let metric: u32 = fields.nth(5).and_then(|m| m.parse().ok())?;
        if destination == 0
            && best
                .as_ref()
                .is_none_or(|(best_metric, _)| metric < *best_metric)
        {
            best = Some((metric, iface.to_string()));
        }
    }
    best.map(|(_, iface)| iface)
}

/// First IPv4 address of an interface via SIOCGIFADDR.
fn iface_ipv4(name: &str) -> Option<Ipv4Addr> {
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return None;
    }
    let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
    let bytes = name.as_bytes();
    if bytes.len() >= libc::IFNAMSIZ {
        unsafe { libc::close(fd) };
        return None;
    }
    for (dst, src) in ifr.ifr_name.iter_mut().zip(bytes) {
        *dst = *src as libc::c_char;
    }
    let ok = unsafe { libc::ioctl(fd, libc::SIOCGIFADDR.try_into().unwrap(), &mut ifr) } == 0;
    unsafe { libc::close(fd) };
    if !ok {
        return None;
    }
    let sin =
        unsafe { &ifr.ifr_ifru.ifru_addr } as *const libc::sockaddr as *const libc::sockaddr_in;
    let s_addr = unsafe { (*sin).sin_addr.s_addr };
    Some(Ipv4Addr::from(u32::from_be(s_addr)))
}

/// Original route-based probe, kept as a last resort when the interface
/// enumeration finds nothing usable.
fn udp_probe_ip() -> String {
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

    use super::{
        connectivity_label, default_route_iface_from, is_unwanted_ip, network_presentation,
    };

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

    #[test]
    fn proxy_tunnel_and_loopback_addresses_are_filtered() {
        use std::net::Ipv4Addr;

        assert!(is_unwanted_ip(Ipv4Addr::new(127, 0, 0, 1)));
        assert!(is_unwanted_ip(Ipv4Addr::new(169, 254, 1, 2)));
        assert!(is_unwanted_ip(Ipv4Addr::new(198, 18, 0, 1)));
        assert!(is_unwanted_ip(Ipv4Addr::new(198, 19, 255, 254)));
        assert!(!is_unwanted_ip(Ipv4Addr::new(192, 168, 0, 104)));
        assert!(!is_unwanted_ip(Ipv4Addr::new(172, 16, 42, 1)));
        assert!(!is_unwanted_ip(Ipv4Addr::new(10, 0, 0, 8)));
    }

    #[test]
    fn default_route_prefers_lowest_metric_interface() {
        let route_table = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMTU\tWindow\tIRTT\n\
usb0\t00000000\t00000000\t0003\t0\t0\t100\t0\t0\t0\n\
wlan0\t00000000\t0100A8C0\t0003\t0\t0\t600\t0\t0\t0\n\
wlan0\t0000A8C0\t00000000\t0001\t0\t0\t600\t0\t0\t0\n";

        assert_eq!(
            default_route_iface_from(route_table).as_deref(),
            Some("usb0")
        );
    }

    #[test]
    fn no_default_route_yields_none() {
        let route_table = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMTU\tWindow\tIRTT\n\
wlan0\t0000A8C0\t00000000\t0001\t0\t0\t600\t0\t0\t0\n";

        assert_eq!(default_route_iface_from(route_table), None);
    }
}
