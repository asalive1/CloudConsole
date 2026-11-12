use std::{env, ffi::CStr, fs, net::Ipv4Addr, path::Path};

use serde::Serialize;

#[derive(Serialize)]
pub struct NetworkInterface {
    pub name: String,
    pub state: String,
    pub addresses: Vec<String>,
}

#[derive(Serialize)]
pub struct AlsaDevice {
    pub device: String,
    pub name: String,
    pub description: String,
}

#[derive(Serialize)]
pub struct SystemDevices {
    pub network_interfaces: Vec<NetworkInterface>,
    pub default_audio_address: Option<String>,
    pub default_metadata_address: Option<String>,
    pub audio_listener_ports: Option<String>,
    pub metadata_listener_ports: Option<String>,
    pub alsa_inputs: Vec<AlsaDevice>,
    pub alsa_outputs: Vec<AlsaDevice>,
}

pub fn discover() -> SystemDevices {
    let (alsa_inputs, alsa_outputs) = alsa_devices();
    SystemDevices {
        network_interfaces: network_interfaces(),
        default_audio_address: crate::network::default_address("AUDIO_BIND"),
        default_metadata_address: crate::network::default_address("METADATA_BIND"),
        audio_listener_ports: crate::network::audio_listener_ports(),
        metadata_listener_ports: crate::network::default_address("METADATA_TCP_PORTS"),
        alsa_inputs,
        alsa_outputs,
    }
}

pub fn interface_ipv4(name: &str) -> Option<String> {
    network_interfaces()
        .into_iter()
        .find(|interface| interface.name == name)
        .and_then(|interface| interface.addresses.into_iter().next())
}

pub fn interface_for_ipv4(address: &str) -> Option<String> {
    network_interfaces()
        .into_iter()
        .find(|interface| interface.addresses.iter().any(|ip| ip == address))
        .map(|interface| interface.name)
}

fn network_interfaces() -> Vec<NetworkInterface> {
    let entries = match fs::read_dir("/sys/class/net") {
        Ok(entries) => entries,
        Err(error) => {
            tracing::error!(%error, "could not enumerate network interfaces");
            return Vec::new();
        }
    };
    let addresses = match ipv4_addresses() {
        Ok(addresses) => addresses,
        Err(error) => {
            tracing::error!(%error, "could not enumerate network interface addresses");
            Vec::new()
        }
    };
    let mut interfaces: Vec<NetworkInterface> = entries
        .filter_map(Result::ok)
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let state = fs::read_to_string(entry.path().join("operstate"))
                .map(|value| value.trim().to_owned())
                .unwrap_or_else(|_| "unknown".to_owned());
            let addresses = addresses
                .iter()
                .filter(|(owner, _)| *owner == name)
                .map(|(_, address)| address.clone())
                .collect();
            NetworkInterface {
                name,
                state,
                addresses,
            }
        })
        .collect();
    interfaces.sort_by(|left, right| left.name.cmp(&right.name));
    interfaces
}

fn ipv4_addresses() -> std::io::Result<Vec<(String, String)>> {
    let mut list = std::ptr::null_mut();
    // getifaddrs owns the linked list until freeifaddrs is called.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut pairs = Vec::new();
    let mut current = list;
    while !current.is_null() {
        // The pointers are supplied by getifaddrs and remain valid until freeifaddrs.
        unsafe {
            let entry = &*current;
            if !entry.ifa_addr.is_null()
                && !entry.ifa_name.is_null()
                && (*entry.ifa_addr).sa_family as i32 == libc::AF_INET
            {
                let address = &*(entry.ifa_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes());
                let name = CStr::from_ptr(entry.ifa_name)
                    .to_string_lossy()
                    .into_owned();
                pairs.push((name, ip.to_string()));
            }
            current = entry.ifa_next;
        }
    }
    unsafe { libc::freeifaddrs(list) };
    pairs.sort();
    pairs.dedup();
    Ok(pairs)
}

fn alsa_devices() -> (Vec<AlsaDevice>, Vec<AlsaDevice>) {
    let path = env::var_os("ALSA_PCM_PATH");
    let pcm_path = path
        .as_deref()
        .map(Path::new)
        .unwrap_or(Path::new("/proc/asound/pcm"));
    let pcm = match fs::read_to_string(pcm_path) {
        Ok(pcm) => pcm,
        Err(error) => {
            if path.is_some() {
                tracing::warn!(path = %pcm_path.display(), %error, "could not read ALSA PCM inventory");
            }
            return (Vec::new(), Vec::new());
        }
    };
    parse_alsa_devices(&pcm)
}

fn parse_alsa_devices(pcm: &str) -> (Vec<AlsaDevice>, Vec<AlsaDevice>) {
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    // Lines look like "00-00: ALC3220 Analog : ALC3220 Analog : playback 1 : capture 1".
    for line in pcm.lines() {
        let parts: Vec<&str> = line.split(" : ").collect();
        let Some((ids, name)) = parts.first().and_then(|head| head.split_once(':')) else {
            continue;
        };
        let Some((card, device)) = ids.trim().split_once('-') else {
            continue;
        };
        let (Ok(card), Ok(device)) = (card.parse::<u32>(), device.parse::<u32>()) else {
            continue;
        };
        let description = parts.get(1).copied().unwrap_or("").trim().to_owned();
        let make = || AlsaDevice {
            device: format!("hw:{card},{device}"),
            name: name.trim().to_owned(),
            description: description.clone(),
        };
        if parts.iter().any(|part| part.trim().starts_with("capture")) {
            inputs.push(make());
        }
        if parts.iter().any(|part| part.trim().starts_with("playback")) {
            outputs.push(make());
        }
    }
    (inputs, outputs)
}

#[cfg(test)]
mod tests {
    use super::{ipv4_addresses, parse_alsa_devices};

    #[test]
    fn discovers_loopback_address_on_its_interface() {
        let addresses = ipv4_addresses().expect("getifaddrs failed");
        assert!(addresses
            .iter()
            .any(|(name, address)| name == "lo" && address == "127.0.0.1"));
    }

    #[test]
    fn discovers_loopback_capture_and_playback_devices() {
        let pcm = "00-00: Loopback PCM : Loopback PCM : playback 8 : capture 8\n\
                   00-01: Loopback PCM : Loopback PCM : playback 8 : capture 8\n\
                   04-00: Generic Analog : Generic Analog : playback 1 : capture 1\n";
        let (inputs, outputs) = parse_alsa_devices(pcm);
        assert_eq!(
            inputs.iter().map(|d| d.device.as_str()).collect::<Vec<_>>(),
            ["hw:0,0", "hw:0,1", "hw:4,0"]
        );
        assert_eq!(
            outputs
                .iter()
                .map(|d| d.device.as_str())
                .collect::<Vec<_>>(),
            ["hw:0,0", "hw:0,1", "hw:4,0"]
        );
    }

    #[test]
    fn separates_capture_and_playback_and_ignores_invalid_entries() {
        let (inputs, outputs) = parse_alsa_devices(
            "01-00: Input : Input : capture 1\n02-03: Output : Output : playback 1\ninvalid\n",
        );
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].device, "hw:1,0");
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].device, "hw:2,3");
    }
}
