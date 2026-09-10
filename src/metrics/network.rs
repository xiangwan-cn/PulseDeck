use crate::model::card_model::{CardValue, StatusLevel};
use crate::model::metric_result::{MetricResult, MetricState};

use super::traits::MetricContext;

/// NetworkManager state is shared by all network cards through MetricContext.
/// The source keeps one connection, signal invalidation, and a bounded
/// fallback deadline; projections only format the shared snapshot.
pub struct NetworkMetric;

impl NetworkMetric {
    pub fn new() -> Self {
        Self
    }

    pub fn collect(&mut self, ctx: &MetricContext) -> MetricResult {
        let snapshot = ctx.network.lock().unwrap().snapshot();
        let connectivity = snapshot.connectivity;
        let connection_name = snapshot.connection_name;
        let ip = snapshot.primary_ip;
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
    use crate::sources::network::{default_route_iface_from, is_unwanted_ip};

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

    #[test]
    fn malformed_route_rows_do_not_hide_valid_default_route() {
        let route_table = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMTU\tWindow\tIRTT\n\
wlan0\t00000000\t0100A8C0\t0003\t0\t0\t600\t0\t0\t0\n\
malformed\tnot-hex\n\
usb0\t00000000\t00000000\t0003\t0\t0\t100\t0\t0\t0\n";

        assert_eq!(
            default_route_iface_from(route_table).as_deref(),
            Some("usb0")
        );
    }
}
