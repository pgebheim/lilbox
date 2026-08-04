use std::{collections::HashSet, path::Path};

use anyhow::{Result, anyhow, bail};

use crate::app::App;
use crate::util::successful_output;

const SERVE_PORT_BASE: u16 = 8443;
const SERVE_PORT_MAX: u16 = 8480;

pub(crate) const DEFAULT_TAG: &str = "tag:lilbox-vm";
pub(crate) const DEFAULT_AUTH_KEY_ENV: &str = "TS_AUTHKEY";
pub(crate) const CONTROL_PLANE_HOST: &str = "controlplane.tailscale.com";

pub(crate) fn tailnet_host(ts: &Path) -> Option<String> {
    let out = successful_output(ts, &["status", "--json"]).ok()?;
    node_hostname(&out)
}

/// Parse `Self.DNSName` out of `tailscale status --json`, trailing dot trimmed.
/// Tolerates leading noise (e.g. a stray line printed before the JSON object)
/// by scanning for the first `{` rather than assuming the whole string is JSON.
pub(crate) fn node_hostname(json: &str) -> Option<String> {
    let start = json.find('{')?;
    serde_json::from_str::<serde_json::Value>(&json[start..]).ok()?["Self"]["DNSName"]
        .as_str()
        .map(|s| s.trim_end_matches('.').into())
}

/// flag > config > default.
pub(crate) fn resolve_tag(flag: Option<&str>, config_tag: Option<&str>) -> String {
    flag.or(config_tag).unwrap_or(DEFAULT_TAG).to_string()
}

pub(crate) fn validate_tag(tag: &str) -> Result<()> {
    let Some(name) = tag.strip_prefix("tag:") else {
        bail!("invalid tailnet tag '{tag}' (must start with 'tag:')");
    };
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        bail!("invalid tailnet tag '{tag}' (name must be non-empty and use [A-Za-z0-9-])");
    }
    Ok(())
}

pub(crate) fn require_auth_key(value: Option<String>, key_env: &str) -> Result<String> {
    match value {
        Some(value) if !value.is_empty() => Ok(value),
        _ => bail!("{key_env} is not set"),
    }
}

/// `key_env` is interpolated (not passed positionally) into the guest script,
/// so it must look like a POSIX environment variable name before use.
pub(crate) fn is_valid_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Build the guest exec argv for joining the tailnet. References the key only
/// via the env var name -- the literal key value never appears here.
pub(crate) fn tailscale_up_command(tag: &str, key_env: &str, hostname: &str) -> Vec<String> {
    let script = format!(
        "command -v tailscaled >/dev/null 2>&1 || exit 3; /usr/local/bin/lilbox-boot || exit 4; \
tailscale up --auth-key=\"${key_env}\" --advertise-tags=\"$1\" --ssh --hostname=\"$2\" || exit 5; \
tailscale status --json"
    );
    vec![
        "/bin/sh".into(),
        "-c".into(),
        script,
        "sh".into(),
        tag.into(),
        hostname.into(),
    ]
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

/// The box's own MagicDNS URL, e.g. `https://web.tail1234.ts.net/`.
pub(crate) fn magicdns_url(node: &str) -> String {
    format!("https://{node}/")
}

/// Preferred display URL for a box: MagicDNS if it's on the tailnet, else the
/// expose/serve URL.
pub(crate) fn box_display_url(
    tailscale_node: Option<&str>,
    expose_url: Option<&str>,
) -> Option<String> {
    tailscale_node
        .map(magicdns_url)
        .or_else(|| expose_url.map(String::from))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_overrides_config_and_default() {
        assert_eq!(
            resolve_tag(Some("tag:from-flag"), Some("tag:from-config")),
            "tag:from-flag"
        );
    }

    #[test]
    fn config_overrides_default() {
        assert_eq!(
            resolve_tag(None, Some("tag:from-config")),
            "tag:from-config"
        );
    }

    #[test]
    fn falls_back_to_default_tag() {
        assert_eq!(resolve_tag(None, None), DEFAULT_TAG);
    }

    #[test]
    fn validate_tag_accepts_prefixed_tags() {
        assert!(validate_tag("tag:lilbox-vm").is_ok());
        assert!(validate_tag("tag:my-box-1").is_ok());
    }

    #[test]
    fn validate_tag_rejects_missing_prefix() {
        assert!(validate_tag("lilbox-vm").is_err());
    }

    #[test]
    fn validate_tag_rejects_empty() {
        assert!(validate_tag("").is_err());
    }

    #[test]
    fn validate_tag_rejects_bare_prefix() {
        assert!(validate_tag("tag:").is_err());
    }

    #[test]
    fn require_auth_key_errs_when_unset() {
        let err = require_auth_key(None, "TS_AUTHKEY").unwrap_err();
        assert!(err.to_string().contains("TS_AUTHKEY"));
    }

    #[test]
    fn require_auth_key_errs_when_empty() {
        assert!(require_auth_key(Some(String::new()), "TS_AUTHKEY").is_err());
    }

    #[test]
    fn require_auth_key_ok_when_set() {
        assert_eq!(
            require_auth_key(Some("tskey-auth-test".into()), "TS_AUTHKEY").unwrap(),
            "tskey-auth-test"
        );
    }

    #[test]
    fn builds_tailscale_up_argv() {
        let argv = tailscale_up_command("tag:lilbox-vm", "TS_AUTHKEY", "mybox");
        let joined = argv.join(" ");
        assert!(joined.contains("--advertise-tags="));
        assert!(joined.contains("tag:lilbox-vm"));
        assert!(joined.contains("--ssh"));
        assert!(joined.contains("$TS_AUTHKEY"));
        assert!(!joined.contains("tskey-auth"));
    }

    #[test]
    fn node_hostname_parses_and_trims_trailing_dot() {
        let json = r#"{"Self":{"DNSName":"web.tail1234.ts.net."}}"#;
        assert_eq!(node_hostname(json).as_deref(), Some("web.tail1234.ts.net"));
    }

    #[test]
    fn is_valid_env_var_name_accepts_typical_names() {
        assert!(is_valid_env_var_name("TS_AUTHKEY"));
        assert!(is_valid_env_var_name("_private"));
        assert!(is_valid_env_var_name("a1"));
    }

    #[test]
    fn is_valid_env_var_name_rejects_bad_names() {
        assert!(!is_valid_env_var_name(""));
        assert!(!is_valid_env_var_name("1BAD"));
        assert!(!is_valid_env_var_name("TS AUTHKEY"));
        assert!(!is_valid_env_var_name("TS-AUTHKEY"));
        assert!(!is_valid_env_var_name("TS_AUTHKEY\"; rm -rf /"));
    }

    #[test]
    fn magicdns_url_formats_https() {
        assert_eq!(
            magicdns_url("web.tail1.ts.net"),
            "https://web.tail1.ts.net/"
        );
    }

    #[test]
    fn box_display_url_prefers_magicdns() {
        assert_eq!(
            box_display_url(Some("web.tail1.ts.net"), Some("https://exposed/")),
            Some("https://web.tail1.ts.net/".into())
        );
    }

    #[test]
    fn box_display_url_falls_back_to_expose() {
        assert_eq!(
            box_display_url(None, Some("https://exposed/")),
            Some("https://exposed/".into())
        );
    }

    #[test]
    fn box_display_url_none_when_neither() {
        assert_eq!(box_display_url(None, None), None);
    }

    #[test]
    fn node_hostname_ignores_leading_noise_before_json_object() {
        let output =
            "/usr/local/bin/tailscaled\n{\"Self\":{\"DNSName\":\"web.tail1234.ts.net.\"}}\n";
        assert_eq!(
            node_hostname(output).as_deref(),
            Some("web.tail1234.ts.net")
        );
    }
}
