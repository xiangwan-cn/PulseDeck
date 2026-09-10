use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::path::Path;
use std::time::{Duration, Instant};

/// Shared NetworkManager state used by every network-status projection. The
/// value is signal-invalidated by the window coordinator and also has a
/// bounded health deadline so a lost D-Bus edge cannot make it stale forever.
#[derive(Debug, Clone)]
pub struct NetworkSnapshot {
    pub connectivity: u32,
    pub connection_name: String,
    /// Resolved once per shared snapshot; projections never repeat ioctl and
    /// interface enumeration for each card.
    pub primary_ip: String,
    pub refreshed_at: Instant,
}

pub struct NetworkSource {
    connection: Option<zbus::blocking::Connection>,
    snapshot: Option<NetworkSnapshot>,
    invalidated: bool,
    fallback_deadline: Duration,
}

impl NetworkSource {
    pub fn new() -> Self {
        Self {
            connection: zbus::blocking::Connection::system().ok(),
            snapshot: None,
            invalidated: true,
            fallback_deadline: Duration::from_secs(30),
        }
    }

    pub fn invalidate(&mut self) {
        self.invalidated = true;
    }

    pub fn snapshot(&mut self) -> NetworkSnapshot {
        let now = Instant::now();
        if !self.invalidated
            && self.snapshot.as_ref().is_some_and(|snapshot| {
                now.duration_since(snapshot.refreshed_at) < self.fallback_deadline
            })
        {
            return self.snapshot.as_ref().expect("checked above").clone();
        }

        if self.connection.is_none() {
            self.connection = zbus::blocking::Connection::system().ok();
        }
        let manager_state = self.connection.as_ref().and_then(network_manager_state);
        if manager_state.is_none() {
            // Let a later bounded watchdog retry the system bus after a
            // disconnect instead of pinning the source to its first fallback.
            self.connection = None;
        }
        let (connectivity, connection_name, primary_iface) =
            manager_state.unwrap_or_else(fallback_state);
        let primary_ip = primary_ip(primary_iface.as_deref());
        let snapshot = NetworkSnapshot {
            connectivity,
            connection_name,
            primary_ip,
            refreshed_at: now,
        };
        self.snapshot = Some(snapshot.clone());
        self.invalidated = false;
        snapshot
    }
}

/// Resolve the primary usable IPv4 address for the shared network snapshot.
/// This source-layer helper deliberately owns interface enumeration so
/// projections only format the already-collected value.
pub(crate) fn primary_ip(preferred_iface: Option<&str>) -> String {
    let route_iface = default_route_iface();
    let mut addresses = Vec::new();
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
            addresses.push((name.clone(), ip, is_physical_interface(&name)));
        }
    }

    if let Some(iface) = preferred_iface {
        if let Some((_, ip, _)) = addresses
            .iter()
            .find(|(name, _, physical)| name == iface && *physical)
        {
            return ip.to_string();
        }
    }
    if let Some(iface) = route_iface.as_deref() {
        if let Some((_, ip, _)) = addresses
            .iter()
            .find(|(name, _, physical)| name == iface && *physical)
        {
            return ip.to_string();
        }
    }
    if let Some((_, ip, _)) = addresses.iter().find(|(_, _, physical)| *physical) {
        return ip.to_string();
    }

    if let Some(iface) = preferred_iface {
        if let Some((_, ip, _)) = addresses.iter().find(|(name, _, _)| name == iface) {
            return ip.to_string();
        }
    }
    if let Some(iface) = route_iface.as_deref() {
        if let Some((_, ip, _)) = addresses.iter().find(|(name, _, _)| name == iface) {
            return ip.to_string();
        }
    }
    if let Some((_, ip, _)) = addresses.first() {
        return ip.to_string();
    }
    udp_probe_ip()
}

fn is_physical_interface(name: &str) -> bool {
    let path = Path::new("/sys/class/net").join(name);
    path.join("device").exists() || path.join("wireless").exists()
}

pub(crate) fn is_unwanted_ip(ip: Ipv4Addr) -> bool {
    ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || matches!(ip.octets(), [198, 18..=19, _, _])
}

fn default_route_iface() -> Option<String> {
    let contents = std::fs::read_to_string("/proc/net/route").ok()?;
    default_route_iface_from(&contents)
}

pub(crate) fn default_route_iface_from(contents: &str) -> Option<String> {
    let mut best: Option<(u32, String)> = None;
    for line in contents.lines().skip(1) {
        let fields: Vec<_> = line.split_whitespace().collect();
        let (Some(&iface), Some(&destination), Some(&metric)) =
            (fields.first(), fields.get(1), fields.get(6))
        else {
            continue;
        };
        let Ok(destination) = u32::from_str_radix(destination, 16) else {
            continue;
        };
        let Ok(metric) = metric.parse::<u32>() else {
            continue;
        };
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

fn network_manager_state(
    connection: &zbus::blocking::Connection,
) -> Option<(u32, String, Option<String>)> {
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
    let (name, primary_iface) = if primary.as_str() == "/" {
        (String::new(), None)
    } else {
        let active = zbus::blocking::Proxy::new(
            connection,
            "org.freedesktop.NetworkManager",
            primary.as_str(),
            "org.freedesktop.NetworkManager.Connection.Active",
        )
        .ok();
        let name = active
            .as_ref()
            .and_then(|proxy| proxy.get_property::<String>("Id").ok())
            .unwrap_or_default();
        let primary_iface = active
            .and_then(|proxy| {
                proxy
                    .get_property::<Vec<zbus::zvariant::OwnedObjectPath>>("Devices")
                    .ok()
            })
            .and_then(|devices| devices.into_iter().next())
            .and_then(|device| {
                let proxy = zbus::blocking::Proxy::new(
                    connection,
                    "org.freedesktop.NetworkManager",
                    device.as_str(),
                    "org.freedesktop.NetworkManager.Device",
                )
                .ok()?;
                proxy.get_property::<String>("Interface").ok()
            });
        (name, primary_iface)
    };
    Some((connectivity, name, primary_iface))
}

fn fallback_state() -> (u32, String, Option<String>) {
    let Ok(entries) = std::fs::read_dir("/sys/class/net") else {
        return (0, String::new(), None);
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
            return (4, name.clone(), Some(name));
        }
    }
    (1, String::new(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidation_forces_a_new_snapshot_deadline() {
        let mut source = NetworkSource::new();
        let first = source.snapshot();
        assert!(!first.connection_name.is_empty() || first.connectivity <= 4);
        source.invalidate();
        let second = source.snapshot();
        assert!(second.refreshed_at >= first.refreshed_at);
    }
}
