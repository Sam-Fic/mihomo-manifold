//! Config generation. The subscription only contributes `proxies`; every other
//! section — tun, dns, proxy-groups, rules — is ours, so a provider updating
//! their profile can never rewrite the user's routing.

use anyhow::{Context, Result};
use regex::Regex;
use serde_yaml::{Mapping, Value};

use crate::config::{AppConfig, GroupKind};

fn v<T: Into<Value>>(x: T) -> Value {
    x.into()
}

fn seq<I: IntoIterator<Item = Value>>(items: I) -> Value {
    Value::Sequence(items.into_iter().collect())
}

fn strings<I: IntoIterator<Item = S>, S: Into<String>>(items: I) -> Value {
    seq(items.into_iter().map(|s| Value::String(s.into())))
}

fn put(map: &mut Mapping, key: &str, value: Value) {
    map.insert(Value::String(key.to_string()), value);
}

/// uTLS fingerprints the core understands, in menu order. The empty first entry
/// means "leave whatever the subscription put on each node".
pub const FINGERPRINTS: [&str; 9] = [
    "", "chrome", "firefox", "safari", "ios", "android", "edge", "360", "random",
];

/// Force one fingerprint onto every node. The core dropped the global option in
/// 1.19, so it has to be written per proxy.
fn with_fingerprint(proxies: &[Value], fingerprint: &str) -> Vec<Value> {
    if fingerprint.trim().is_empty() {
        return proxies.to_vec();
    }
    proxies
        .iter()
        .map(|proxy| {
            let mut proxy = proxy.clone();
            if let Value::Mapping(map) = &mut proxy {
                put(map, "client-fingerprint", v(fingerprint.trim()));
            }
            proxy
        })
        .collect()
}

/// Node names in subscription order, used to fill the generated groups.
pub fn proxy_names(proxies: &[Value]) -> Vec<String> {
    proxies
        .iter()
        .filter_map(|p| p.get("name").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// A generated group with its members resolved. The Nodes page uses this when
/// the core is not answering, so the list stays browsable instead of empty.
pub struct GroupView {
    pub name: String,
    pub kind: GroupKind,
    pub members: Vec<String>,
}

/// Expand every configured group against the cached profile, mirroring
/// [`proxy_groups`]: same members in the same order, so the offline list and
/// what the core reports once it starts agree.
///
/// A group with a `filter` is handed to the core as `include-all-proxies` plus
/// that regex, so the same expression has to be applied here — a filter is only
/// settable declaratively, but the offline list still has to honour it.
pub fn group_views(cfg: &AppConfig, proxies: &[Value]) -> Vec<GroupView> {
    let names = proxy_names(proxies);
    let all_group_names = cfg.routing.group_names();
    let mut views = Vec::new();

    for spec in &cfg.routing.groups {
        let mut members: Vec<String> = Vec::new();
        if spec.include_specials {
            members.extend(
                all_group_names
                    .iter()
                    .filter(|n| n.as_str() != spec.name)
                    .cloned(),
            );
            members.push("DIRECT".to_string());
        }

        let filter = spec.filter.trim();
        if filter.is_empty() {
            members.extend(names.iter().cloned());
        } else if let Ok(re) = Regex::new(filter) {
            members.extend(names.iter().filter(|name| re.is_match(name)).cloned());
        }

        // The core refuses a group with no members, so the generator falls back
        // to DIRECT; the offline view has to show the same thing.
        if members.is_empty() && filter.is_empty() {
            members.push("DIRECT".to_string());
        }

        views.push(GroupView {
            name: spec.name.clone(),
            kind: spec.kind,
            members,
        });
    }

    views
}

fn tun_section(cfg: &AppConfig) -> Value {
    let mut tun = Mapping::new();
    put(&mut tun, "enable", v(true));
    put(&mut tun, "stack", v(cfg.core.tun_stack.clone()));
    put(&mut tun, "device", v("mihomo-tun"));
    put(&mut tun, "auto-route", v(true));
    // auto-redirect makes the core write its own nftables rules, and only pays
    // off when this host forwards other devices' traffic. On a desktop it buys
    // nothing and adds a second writer to the firewall, so it stays off.
    put(&mut tun, "auto-redirect", v(false));
    put(&mut tun, "auto-detect-interface", v(true));
    put(&mut tun, "strict-route", v(false));
    put(&mut tun, "mtu", v(9000u64));
    put(&mut tun, "dns-hijack", strings(["any:53", "tcp://any:53"]));
    Value::Mapping(tun)
}

fn dns_section(cfg: &AppConfig) -> Value {
    let mut dns = Mapping::new();
    put(&mut dns, "enable", v(true));
    put(&mut dns, "ipv6", v(cfg.core.ipv6));
    put(&mut dns, "listen", v("127.0.0.1:1053"));
    put(&mut dns, "prefer-h3", v(false));
    put(&mut dns, "respect-rules", v(true));
    if cfg.core.fake_ip {
        put(&mut dns, "enhanced-mode", v("fake-ip"));
        put(&mut dns, "fake-ip-range", v("198.18.0.1/16"));
        put(
            &mut dns,
            "fake-ip-filter",
            strings([
                "*.lan",
                "*.local",
                "*.localdomain",
                "localhost",
                "time.*.com",
                "+.pool.ntp.org",
                "+.in-addr.arpa",
                "+.ip6.arpa",
            ]),
        );
    } else {
        put(&mut dns, "enhanced-mode", v("redir-host"));
    }
    put(
        &mut dns,
        "default-nameserver",
        strings(["223.5.5.5", "119.29.29.29"]),
    );
    put(
        &mut dns,
        "nameserver",
        strings(["https://223.5.5.5/dns-query", "https://doh.pub/dns-query"]),
    );
    // Resolving the node hostnames themselves must not go through the tunnel.
    put(
        &mut dns,
        "proxy-server-nameserver",
        strings(["https://223.5.5.5/dns-query", "https://doh.pub/dns-query"]),
    );
    Value::Mapping(dns)
}

fn proxy_groups(cfg: &AppConfig, names: &[String]) -> Value {
    let all_group_names = cfg.routing.group_names();
    let mut groups = Vec::new();

    for spec in &cfg.routing.groups {
        let mut group = Mapping::new();
        put(&mut group, "name", v(spec.name.clone()));
        put(&mut group, "type", v(spec.kind.as_yaml()));

        let mut members: Vec<String> = Vec::new();
        if spec.include_specials {
            members.extend(
                all_group_names
                    .iter()
                    .filter(|n| n.as_str() != spec.name)
                    .cloned(),
            );
            members.push("DIRECT".to_string());
        }

        if spec.filter.trim().is_empty() {
            members.extend(names.iter().cloned());
        } else {
            // Let the core apply the regex over every node it knows about.
            put(&mut group, "include-all-proxies", v(true));
            put(&mut group, "filter", v(spec.filter.clone()));
        }

        // A group with an empty member list makes the core refuse to start.
        if members.is_empty() && spec.filter.trim().is_empty() {
            members.push("DIRECT".to_string());
        }
        put(&mut group, "proxies", strings(members));

        if !matches!(spec.kind, GroupKind::Select) {
            put(&mut group, "url", v(spec.test_url.clone()));
            put(&mut group, "interval", v(spec.interval));
            put(&mut group, "tolerance", v(50u64));
        }
        groups.push(Value::Mapping(group));
    }

    seq(groups)
}

fn rule_providers(cfg: &AppConfig) -> Option<(Value, Vec<String>)> {
    let enabled: Vec<_> = cfg
        .routing
        .rule_providers
        .iter()
        .filter(|p| p.enabled && !p.name.trim().is_empty() && !p.url.trim().is_empty())
        .collect();
    if enabled.is_empty() {
        return None;
    }

    let mut map = Mapping::new();
    let mut rules = Vec::new();
    for provider in enabled {
        let mut entry = Mapping::new();
        put(&mut entry, "type", v("http"));
        put(&mut entry, "url", v(provider.url.clone()));
        put(&mut entry, "behavior", v(provider.behavior.clone()));
        put(&mut entry, "format", v(provider.format.clone()));
        put(&mut entry, "interval", v(provider.interval));
        put(
            &mut entry,
            "path",
            v(format!(
                "./providers/rules/{}.{}",
                provider.name, provider.format
            )),
        );
        map.insert(Value::String(provider.name.clone()), Value::Mapping(entry));

        let mut rule = format!(
            "RULE-SET,{},{}",
            provider.name,
            provider.target.as_rule_target()
        );
        if provider.behavior == "ipcidr" {
            rule.push_str(",no-resolve");
        }
        rules.push(rule);
    }
    Some((Value::Mapping(map), rules))
}

/// Rule order is load-bearing: the core takes the first match, so process rules
/// have to sit above the geo/provider lists or they never fire.
fn rules(cfg: &AppConfig, provider_rules: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();

    out.extend(cfg.routing.raw_prepend.iter().cloned());

    out.extend(
        cfg.routing
            .app_rules
            .iter()
            .filter(|r| r.enabled && !r.value.trim().is_empty())
            .map(|r| r.to_rule()),
    );

    if cfg.core.bypass_private {
        out.push("GEOIP,private,DIRECT,no-resolve".to_string());
        out.push("DOMAIN-SUFFIX,local,DIRECT".to_string());
        out.push("DOMAIN-SUFFIX,lan,DIRECT".to_string());
    }

    out.extend(
        cfg.routing
            .domain_rules
            .iter()
            .filter(|r| r.enabled && !r.value.trim().is_empty())
            .map(|r| r.to_rule(cfg.core.fake_ip)),
    );

    out.extend(provider_rules);
    out.extend(cfg.routing.raw_append.iter().cloned());
    out.push(format!(
        "MATCH,{}",
        cfg.routing.default_target.as_rule_target()
    ));
    out
}

/// Build the full config the core is started with.
pub fn generate(cfg: &AppConfig, proxies: &[Value]) -> Result<String> {
    let mut root = Mapping::new();

    put(&mut root, "mixed-port", v(cfg.core.mixed_port as u64));
    put(&mut root, "allow-lan", v(cfg.core.allow_lan));
    put(&mut root, "bind-address", v("*"));
    put(&mut root, "mode", v("rule"));
    put(&mut root, "log-level", v(cfg.core.log_level.clone()));
    put(&mut root, "ipv6", v(cfg.core.ipv6));
    put(&mut root, "unified-delay", v(true));
    put(&mut root, "tcp-concurrent", v(true));
    put(
        &mut root,
        "external-controller",
        v(cfg.core.controller_addr()),
    );
    put(&mut root, "secret", v(cfg.core.secret.clone()));

    // Matching by process needs the core to look up the owner of every
    // connection; without this the PROCESS-* rules silently never match.
    put(
        &mut root,
        "find-process-mode",
        v(if cfg.routing.uses_process_rules() {
            "always"
        } else {
            "strict"
        }),
    );

    let mut profile = Mapping::new();
    put(&mut profile, "store-selected", v(true));
    put(&mut profile, "store-fake-ip", v(true));
    put(&mut root, "profile", Value::Mapping(profile));

    if cfg.core.tun_enabled {
        put(&mut root, "tun", tun_section(cfg));
    }
    put(&mut root, "dns", dns_section(cfg));

    let proxies = with_fingerprint(proxies, &cfg.core.client_fingerprint);
    put(&mut root, "proxies", seq(proxies.iter().cloned()));

    let names = proxy_names(&proxies);
    put(&mut root, "proxy-groups", proxy_groups(cfg, &names));

    let provider_rules = match rule_providers(cfg) {
        Some((providers, provider_rules)) => {
            put(&mut root, "rule-providers", providers);
            provider_rules
        }
        None => Vec::new(),
    };

    put(&mut root, "rules", strings(rules(cfg, provider_rules)));

    serde_yaml::to_string(&Value::Mapping(root)).context("serializing generated config")
}

/// A throwaway config for the temporary core that measures latency while the
/// real one is stopped. It carries only what a delay test needs: the nodes, one
/// `select` group per configured group (so `/group/<name>/delay` resolves the
/// same members the page lists), no TUN, no DNS and no routing rules. It must
/// never grow into a second copy of [`generate`].
///
/// `profile` stores nothing: this core shares `-d` with the real one, and it has
/// no business writing the selections held in `cache.db`.
pub fn scratch_config(
    cfg: &AppConfig,
    proxies: &[Value],
    controller_port: u16,
    mixed_port: u16,
    secret: &str,
) -> Result<String> {
    let mut root = Mapping::new();

    put(&mut root, "mixed-port", v(mixed_port as u64));
    put(&mut root, "allow-lan", v(false));
    put(&mut root, "mode", v("rule"));
    put(&mut root, "log-level", v("warning"));
    put(
        &mut root,
        "external-controller",
        v(format!("127.0.0.1:{controller_port}")),
    );
    put(&mut root, "secret", v(secret));

    let mut profile = Mapping::new();
    put(&mut profile, "store-selected", v(false));
    put(&mut profile, "store-fake-ip", v(false));
    put(&mut root, "profile", Value::Mapping(profile));

    let proxies = with_fingerprint(proxies, &cfg.core.client_fingerprint);
    put(&mut root, "proxies", seq(proxies.iter().cloned()));
    put(&mut root, "proxy-groups", scratch_groups(cfg, &proxies));
    put(&mut root, "rules", strings(["MATCH,DIRECT"]));

    serde_yaml::to_string(&Value::Mapping(root)).context("serializing the scratch config")
}

/// The configured groups as plain `select` groups with explicit members, taken
/// from [`group_views`]. That keeps a delay test resolving exactly what the Nodes
/// page shows — filters included — and stops the core from running health checks
/// of its own in the background.
fn scratch_groups(cfg: &AppConfig, proxies: &[Value]) -> Value {
    let groups = group_views(cfg, proxies)
        .into_iter()
        .map(|view| {
            let mut group = Mapping::new();
            put(&mut group, "name", v(view.name));
            put(&mut group, "type", v("select"));
            // An empty member list makes the core refuse to start, and a group
            // that matches nothing has only DIRECT to fall back on anyway.
            let members = if view.members.is_empty() {
                vec!["DIRECT".to_string()]
            } else {
                view.members
            };
            put(&mut group, "proxies", strings(members));
            Value::Mapping(group)
        })
        .collect();
    Value::Sequence(groups)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppMatch, AppRule, DomainRule, MatchKind, Target};

    fn node(name: &str) -> Value {
        let mut m = Mapping::new();
        put(&mut m, "name", v(name));
        put(&mut m, "type", v("vless"));
        Value::Mapping(m)
    }

    #[test]
    fn process_rules_come_before_geo_rules() {
        let mut cfg = AppConfig::default();
        cfg.routing.app_rules.push(AppRule {
            enabled: true,
            match_by: AppMatch::Name,
            value: "steam".into(),
            target: Target::Direct,
            label: "Steam".into(),
        });
        cfg.routing.domain_rules.push(DomainRule {
            enabled: true,
            kind: MatchKind::Geosite,
            value: "youtube".into(),
            target: Target::Group("PROXY".into()),
        });

        let rules = rules(&cfg, Vec::new());
        let steam = rules.iter().position(|r| r.contains("steam")).unwrap();
        let youtube = rules.iter().position(|r| r.contains("youtube")).unwrap();
        assert!(steam < youtube, "process rules must win: {rules:?}");
        assert_eq!(rules.last().unwrap(), "MATCH,PROXY");
    }

    #[test]
    fn process_rules_switch_on_process_lookup() {
        let mut cfg = AppConfig::default();
        let plain = generate(&cfg, &[node("a")]).unwrap();
        assert!(plain.contains("find-process-mode: strict"));

        cfg.routing.app_rules.push(AppRule {
            value: "telegram-desktop".into(),
            ..Default::default()
        });
        let with_apps = generate(&cfg, &[node("a")]).unwrap();
        assert!(with_apps.contains("find-process-mode: always"));
    }

    #[test]
    fn the_forced_fingerprint_reaches_every_node() {
        let mut with_own = Mapping::new();
        put(&mut with_own, "name", v("A"));
        put(&mut with_own, "client-fingerprint", v("edge"));
        let nodes = [Value::Mapping(with_own), node("B")];

        // Empty setting: whatever the subscription said stays.
        let untouched = with_fingerprint(&nodes, "");
        assert_eq!(untouched[0]["client-fingerprint"], v("edge"));
        assert!(untouched[1].get("client-fingerprint").is_none());

        // Set: every node carries it, including one that had its own.
        let forced = with_fingerprint(&nodes, "firefox");
        assert_eq!(forced[0]["client-fingerprint"], v("firefox"));
        assert_eq!(forced[1]["client-fingerprint"], v("firefox"));
    }

    #[test]
    fn generated_config_keeps_subscription_nodes_only() {
        let cfg = AppConfig::default();
        let yaml = generate(&cfg, &[node("Amsterdam"), node("Frankfurt")]).unwrap();
        let parsed: Value = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed["proxies"].as_sequence().unwrap().len(), 2);
        assert!(yaml.contains("Amsterdam"));
        // Groups and rules are ours, not the provider's.
        assert!(parsed["proxy-groups"].as_sequence().unwrap().len() == 2);
    }

    #[test]
    fn offline_groups_mirror_the_generated_ones() {
        let mut cfg = AppConfig::default();
        // Give AUTO a filter; PROXY keeps the default "every node".
        cfg.routing.groups[1].filter = "(?i)hk".to_string();
        let nodes = [node("HK-1"), node("HK-2"), node("US-1")];
        let views = group_views(&cfg, &nodes);

        assert_eq!(views[0].name, "PROXY");
        assert_eq!(views[0].kind.as_core_type(), "Selector");
        // Specials come first, exactly as `proxy_groups` writes them.
        assert_eq!(views[0].members[0], "AUTO");
        assert_eq!(views[0].members[1], "DIRECT");
        assert!(views[0].members.contains(&"US-1".to_string()));

        assert_eq!(views[1].name, "AUTO");
        assert_eq!(views[1].members, vec!["HK-1", "HK-2"]);
    }

    #[test]
    fn scratch_config_is_minimal_and_testable() {
        let cfg = AppConfig::default();
        let yaml = scratch_config(&cfg, &[node("A"), node("B")], 9099, 7891, "s3cret").unwrap();
        let parsed: Value = serde_yaml::from_str(&yaml).unwrap();

        // Nothing that would touch the system: no tunnel, no DNS, no routing.
        assert!(parsed.get("tun").is_none());
        assert!(parsed.get("dns").is_none());
        assert_eq!(
            parsed["rules"].as_sequence().unwrap()[0].as_str(),
            Some("MATCH,DIRECT")
        );
        assert_eq!(parsed["external-controller"], v("127.0.0.1:9099"));

        // Members are explicit, so a filtered group resolves here rather than
        // through include-all-proxies once the core starts.
        let groups = parsed["proxy-groups"].as_sequence().unwrap();
        assert_eq!(groups[0]["name"], v("PROXY"));
        assert_eq!(groups[0]["type"], v("select"));
        assert!(groups[0]["proxies"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|member| member.as_str() == Some("A")));

        // It shares the working directory with the real core, so it must not
        // write the selections stored there.
        assert_eq!(parsed["profile"]["store-selected"], v(false));
    }

    #[test]
    fn offline_groups_keep_an_empty_filtered_group_usable() {
        let mut cfg = AppConfig::default();
        cfg.routing.groups[1].filter = "(?i)nowhere".to_string();
        let views = group_views(&cfg, &[node("HK-1")]);
        // No node matches, and the generator does not add DIRECT for a filter.
        assert!(views[1].members.is_empty());
    }
}
