use std::{env, net::Ipv4Addr};

fn configured(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

pub fn host_mode() -> bool {
    configured("HOST_AUDIO_NETWORK").as_deref() == Some("true")
}

pub fn validate_configuration() -> Result<(), String> {
    if !host_mode() {
        return Ok(());
    }
    for name in ["AUDIO_BIND", "METADATA_BIND"] {
        let value = configured(name).ok_or_else(|| format!("{name} is required"))?;
        let ip: Ipv4Addr = value
            .parse()
            .map_err(|_| format!("{name} must be an IPv4 address"))?;
        if ip.is_unspecified() || crate::system::interface_for_ipv4(&value).is_none() {
            return Err(format!(
                "{name} must be an IPv4 address assigned to a host interface"
            ));
        }
    }
    port_range("ENGINE_UDP_PORTS")?;
    port_range("METADATA_TCP_PORTS")?;
    Ok(())
}

fn port_range(name: &str) -> Result<(u16, u16), String> {
    let value = configured(name).ok_or_else(|| format!("{name} is required"))?;
    parse_port_range(name, &value)
}

fn parse_port_range(name: &str, value: &str) -> Result<(u16, u16), String> {
    let (first, last) = value
        .split_once('-')
        .ok_or_else(|| format!("{name} must be a port range"))?;
    let start: u16 = first.parse().map_err(|_| format!("invalid {name}"))?;
    let end: u16 = last.parse().map_err(|_| format!("invalid {name}"))?;
    if start == 0 || start > end {
        return Err(format!("invalid {name}"));
    }
    Ok((start, end))
}

pub fn check_listener_port(name: &str, port: u16) -> Result<(), String> {
    if host_mode() {
        let (start, end) = port_range(name)?;
        if !(start..=end).contains(&port) {
            return Err(format!(
                "listener port {port} must be in {name} ({start}-{end})"
            ));
        }
    }
    Ok(())
}

pub fn default_address(name: &str) -> Option<String> {
    if host_mode() {
        configured(name)
    } else {
        None
    }
}

pub fn default_audio_interface() -> Result<Option<String>, String> {
    match default_address("AUDIO_BIND") {
        Some(address) => crate::system::interface_for_ipv4(&address)
            .map(Some)
            .ok_or_else(|| format!("AUDIO_BIND {address} is not assigned to a host interface")),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_port_range;

    #[test]
    fn validates_port_ranges() {
        assert_eq!(parse_port_range("TEST", "7010-7019").unwrap(), (7010, 7019));
        for value in ["0-1", "7019-7010", "70000-70001", "7010", "a-b"] {
            assert!(parse_port_range("TEST", value).is_err(), "{value}");
        }
    }
}
