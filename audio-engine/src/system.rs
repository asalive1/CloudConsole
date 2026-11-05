use std::fs;

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
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    let Ok(pcm) = fs::read_to_string("/proc/asound/pcm") else {
        return (inputs, outputs);
    };
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
