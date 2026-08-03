use std::{collections::HashSet, path::Path};

use anyhow::{Result, anyhow};

use crate::app::App;
use crate::util::successful_output;

const SERVE_PORT_BASE: u16 = 8443;
const SERVE_PORT_MAX: u16 = 8480;

pub(crate) fn tailnet_host(ts: &Path) -> Option<String> {
    let out = successful_output(ts, &["status", "--json"]).ok()?;
    serde_json::from_str::<serde_json::Value>(&out).ok()?["Self"]["DNSName"]
        .as_str()
        .map(|s| s.trim_end_matches('.').into())
}

pub(crate) fn serve_ports(ts: &Path) -> HashSet<u16> {
    let mut ports = HashSet::new();
    let Ok(out) = successful_output(ts, &["serve", "status", "--json"]) else {
        return ports;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&out) else {
        return ports;
    };
    for key in ["TCP", "Web"] {
        if let Some(map) = value[key].as_object() {
            for name in map.keys() {
                if let Ok(port) = name.rsplit(':').next().unwrap_or(name).parse() {
                    ports.insert(port);
                }
            }
        }
    }
    ports
}

pub(crate) fn allocate_serve_port(app: &App, public: bool) -> Result<u16> {
    let ts = app
        .tailscale
        .as_deref()
        .ok_or_else(|| anyhow!("tailscale not found - cannot publish"))?;
    let mut used = serve_ports(ts);
    for row in app.rows()? {
        if let Some(port) = row.serve_port {
            used.insert(port);
        }
    }
    let choices: Vec<u16> = if public {
        vec![8443, 10000, 443]
    } else {
        (SERVE_PORT_BASE..=SERVE_PORT_MAX).collect()
    };
    choices
        .into_iter()
        .find(|p| !used.contains(p))
        .ok_or_else(|| anyhow!("no free Tailscale HTTPS port"))
}
