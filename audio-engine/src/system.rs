use std::{env, fs, path::Path};

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
    pub alsa_inputs: Vec<AlsaDevice>,
    pub alsa_outputs: Vec<AlsaDevice>,
}

pub fn discover() -> SystemDevices {
    let (alsa_inputs, alsa_outputs) = alsa_devices();
    SystemDevices {
        network_interfaces: network_interfaces(),
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

fn network_interfaces() -> Vec<NetworkInterface> {
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };
    let addresses = ipv4_addresses();
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

// /proc/net/fib_trie lists local addresses but not their owners, so pair them using /proc/net/route.
fn ipv4_addresses() -> Vec<(String, String)> {
    let Ok(output) = fs::read_to_string("/proc/net/fib_trie") else {
        return Vec::new();
    };
    let local: Vec<String> = {
        let mut found = Vec::new();
        let mut previous = String::new();
        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("/32 host LOCAL") {
                if let Some(address) = previous.split_whitespace().last() {
                    if !found.contains(&address.to_owned()) {
                        found.push(address.to_owned());
                    }
                }
            }
            if let Some(rest) = trimmed.strip_prefix("|-- ") {
                previous = rest.to_owned();
            }
        }
        found
    };
    let Ok(routes) = fs::read_to_string("/proc/net/route") else {
        return Vec::new();
    };
    let mut pairs = Vec::new();
    for address in local {
        if address.starts_with("127.") {
            pairs.push(("lo".to_owned(), address));
            continue;
        }
        let octets: Vec<u32> = address
            .split('.')
            .filter_map(|part| part.parse().ok())
            .collect();
        if octets.len() != 4 {
            continue;
        }
        let value = octets[0] | (octets[1] << 8) | (octets[2] << 16) | (octets[3] << 24);
        for line in routes.lines().skip(1) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 8 {
                continue;
            }
            let (Ok(destination), Ok(mask)) = (
                u32::from_str_radix(fields[1], 16),
                u32::from_str_radix(fields[7], 16),
            ) else {
                continue;
            };
            if destination != 0 && value & mask == destination {
                pairs.push((fields[0].to_owned(), address.clone()));
                break;
            }
        }
    }
    pairs
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
    use super::parse_alsa_devices;

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
