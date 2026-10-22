//! Restrictive packaged policy and explicitly bounded development connections.
use crate::config::RevenantConfig;
use std::collections::BTreeMap;

pub(super) fn policy(config: &RevenantConfig, development: bool) -> BTreeMap<String, Vec<String>> {
    let preview = if cfg!(windows) {
        "http://revenant-preview.localhost"
    } else {
        "revenant-preview:"
    };
    let mut policy: BTreeMap<String, Vec<String>> = [
        ("default-src", vec!["'none'"]),
        ("script-src", vec!["'self'"]),
        ("style-src", vec!["'self'", "'unsafe-inline'"]),
        (
            "connect-src",
            vec!["'self'", "ipc:", "http://ipc.localhost", preview],
        ),
        ("img-src", vec!["'self'", "data:", "blob:", preview]),
        ("media-src", vec!["'self'", "data:", "blob:", preview]),
        ("font-src", vec!["'self'", "data:"]),
        ("worker-src", vec!["'self'", "blob:"]),
        ("object-src", vec!["'none'"]),
        ("base-uri", vec!["'self'"]),
        ("form-action", vec!["'none'"]),
        ("frame-ancestors", vec!["'none'"]),
    ]
    .into_iter()
    .map(|(key, values)| {
        (
            key.to_owned(),
            values.into_iter().map(str::to_owned).collect(),
        )
    })
    .collect();
    if development {
        let connections = policy.get_mut("connect-src").unwrap();
        for host in ["127.0.0.1", "localhost"] {
            connections.push(format!("http://{host}:{}", config.desktop.dev_port));
            connections.push(format!("ws://{host}:{}", config.desktop.dev_port));
        }
    }
    for (directive, sources) in &config.desktop.csp_allowances {
        let values = policy.entry(directive.clone()).or_default();
        for source in sources {
            if !values.contains(source) {
                values.push(source.clone());
            }
        }
    }
    policy
}
