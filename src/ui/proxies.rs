//! Node picker. With the core up the list comes from the controller; with it
//! down the same groups are expanded from the cached profile instead, so nodes
//! stay browsable and a pick can be queued for the next start.

use adw::prelude::*;
use std::collections::HashMap;
use std::rc::Rc;

use crate::api::ProxiesResponse;
use crate::i18n::{t, tf};
use crate::runtime;
use crate::state::{self, AppState};
use crate::template;
use crate::ui::widgets;

/// One row of a group, in either state. `delay` is only ever set by the core.
struct Node {
    name: String,
    kind: String,
    udp: bool,
    delay: Option<u32>,
    selected: bool,
}

struct Group {
    name: String,
    kind: String,
    nodes: Vec<Node>,
}

fn delay_label(delay: Option<u32>) -> gtk::Label {
    let (text, class) = match delay {
        // A test that ran and failed, as opposed to one that never ran.
        Some(0) => (t("timeout").to_string(), "error"),
        Some(ms) if ms < 200 => (format!("{ms} ms"), "success"),
        Some(ms) if ms < 600 => (format!("{ms} ms"), "warning"),
        Some(ms) => (format!("{ms} ms"), "error"),
        None => ("—".to_string(), "dim-label"),
    };
    let label = gtk::Label::builder()
        .label(text)
        .valign(gtk::Align::Center)
        .build();
    label.add_css_class(class);
    label.add_css_class("caption");
    label
}

fn placeholder(title: &str, description: &str) -> gtk::Widget {
    let status = adw::StatusPage::builder()
        .icon_name("network-offline-symbolic")
        .title(title)
        .description(description)
        .vexpand(true)
        .build();
    status.upcast()
}

/// Says why the list is here without a core, and what picking and testing do.
fn offline_hint() -> gtk::Widget {
    let label = gtk::Label::builder()
        .label(t(
            "Core is not running — showing the nodes saved in the profile. \
             Testing runs a temporary core; a pick is applied when the real one starts.",
        ))
        .wrap(true)
        .xalign(0.0)
        .build();
    label.add_css_class("dim-label");
    label.upcast()
}

/// Name → (type, udp) from the cached profile, so the offline rows can still
/// show what each node is.
fn node_facts(proxies: &[serde_yaml::Value]) -> HashMap<String, (String, bool)> {
    proxies
        .iter()
        .filter_map(|proxy| {
            let name = proxy.get("name")?.as_str()?.to_string();
            let kind = proxy
                .get("type")
                .and_then(serde_yaml::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let udp = proxy
                .get("udp")
                .and_then(serde_yaml::Value::as_bool)
                .unwrap_or(false);
            Some((name, (kind, udp)))
        })
        .collect()
}

/// A member that is not a node — DIRECT, or another generated group — still
/// deserves a type rather than "unknown".
fn offline_kind(
    name: &str,
    group_kinds: &HashMap<String, String>,
    facts: &HashMap<String, (String, bool)>,
) -> String {
    match name {
        "DIRECT" => return "Direct".to_string(),
        "REJECT" => return "Reject".to_string(),
        _ => {}
    }
    if let Some(kind) = group_kinds.get(name) {
        return kind.clone();
    }
    facts
        .get(name)
        .map(|(kind, _)| kind.clone())
        .filter(|kind| !kind.is_empty())
        .unwrap_or_else(|| t("unknown").to_string())
}

/// The controller's view: authoritative while the core runs, and the only
/// source of live latency. Each group's `now` is mirrored into the settings so
/// the offline list can still show the current node after the core stops.
fn from_controller(state: &Rc<AppState>, response: &ProxiesResponse) -> Vec<Group> {
    let configured = state.config.borrow().routing.group_names();
    let mut group_names: Vec<String> = response
        .proxies
        .values()
        .filter(|p| p.is_group() && !p.all.is_empty())
        .map(|p| p.name.clone())
        .collect();

    // Our own groups first, in the order they are generated.
    group_names.sort_by_key(|name| {
        configured
            .iter()
            .position(|c| c == name)
            .unwrap_or(usize::MAX)
    });

    let mut groups = Vec::new();
    for group_name in group_names {
        let Some(group) = response.proxies.get(&group_name) else {
            continue;
        };

        if let Some(now) = &group.now {
            // Keeps the app's copy in step without a round trip of its own.
            state::note_selection(state, &group_name, now);
        }

        let nodes = group
            .all
            .iter()
            .map(|member| {
                let info = response.proxies.get(member);
                Node {
                    name: member.clone(),
                    kind: info
                        .map(|i| i.kind.clone())
                        .unwrap_or_else(|| t("unknown").to_string()),
                    udp: info.is_some_and(|i| i.udp),
                    delay: info.and_then(|i| i.last_delay()),
                    selected: group.now.as_deref() == Some(member.as_str()),
                }
            })
            .collect();

        groups.push(Group {
            name: group_name,
            kind: group.kind.clone(),
            nodes,
        });
    }
    groups
}

/// The core's next start, built from the cached profile. Same members and order
/// as the generator, which is what makes the offline checkmark truthful.
fn from_profile(state: &Rc<AppState>) -> Result<Vec<Group>, String> {
    let proxies = state::active_proxies(state)?;
    let cfg = state.config.borrow();

    let facts = node_facts(&proxies);
    let selected = cfg.selected.clone();
    let delays = state.delays.borrow();
    let group_kinds: HashMap<String, String> = cfg
        .routing
        .groups
        .iter()
        .map(|spec| (spec.name.clone(), spec.kind.as_core_type().to_string()))
        .collect();

    let groups = template::group_views(&cfg, &proxies)
        .into_iter()
        .map(|view| {
            let nodes = view
                .members
                .iter()
                .map(|member| Node {
                    name: member.clone(),
                    kind: offline_kind(member, &group_kinds, &facts),
                    udp: facts.get(member).map(|(_, udp)| *udp).unwrap_or(false),
                    delay: delays.get(member).copied(),
                    selected: selected.get(&view.name).map(String::as_str) == Some(member.as_str()),
                })
                .collect();
            Group {
                name: view.name,
                kind: view.kind.as_core_type().to_string(),
                nodes,
            }
        })
        .collect();

    Ok(groups)
}

fn populate(state: &Rc<AppState>, container: &gtk::Box, groups: Vec<Group>, live: bool) {
    widgets::clear(container);

    if groups.is_empty() {
        container.append(&placeholder(
            t("No proxy groups"),
            t("No groups are configured yet."),
        ));
        return;
    }

    if !live {
        container.append(&offline_hint());
    }

    for group in groups {
        let prefs = adw::PreferencesGroup::builder()
            .title(&group.name)
            .description(tf("{} · {} nodes", &[&group.kind, &group.nodes.len()]))
            .build();

        // While a test is in flight for this group, a spinner stands in for the
        // button — the page is rebuilt when the result lands.
        if state.testing.borrow().as_deref() == Some(group.name.as_str()) {
            let spinner = gtk::Spinner::builder()
                .spinning(true)
                .valign(gtk::Align::Center)
                .build();
            prefs.set_header_suffix(Some(&spinner));
        } else {
            let test = widgets::action_button("view-refresh-symbolic", t("Test"));
            if live {
                let test_state = state.clone();
                let test_group = group.name.clone();
                test.connect_clicked(move |_| {
                    let Some(api) = test_state.api() else { return };
                    let state = test_state.clone();
                    let name = test_group.clone();
                    *state.testing.borrow_mut() = Some(name.clone());
                    state.notify();
                    runtime::spawn(
                        async move {
                            api.group_delay(&name, state::TEST_URL, state::TEST_TIMEOUT_MS)
                                .await
                                .map_err(|e| e.to_string())
                        },
                        move |result| {
                            *state.testing.borrow_mut() = None;
                            match result {
                                Ok(_) => state.notify(),
                                Err(err) => {
                                    state.notify();
                                    state.toast(&tf("Latency test failed: {}", &[&err]));
                                }
                            }
                        },
                    );
                });
            } else {
                // With the real core stopped this starts a throwaway one, which
                // the hint above the list explains.
                let test_state = state.clone();
                let test_group = group.name.clone();
                test.connect_clicked(move |_| {
                    state::test_group_offline(&test_state, &test_group);
                });
            }
            prefs.set_header_suffix(Some(&test));
        }

        for node in &group.nodes {
            let row = adw::ActionRow::builder()
                .title(glib_escape(&node.name))
                .subtitle(node.kind.as_str())
                .activatable(true)
                .build();

            if node.udp {
                let udp = gtk::Label::builder()
                    .label("UDP")
                    .valign(gtk::Align::Center)
                    .build();
                udp.add_css_class("dim-label");
                udp.add_css_class("caption");
                row.add_suffix(&udp);
            }
            row.add_suffix(&delay_label(node.delay));

            if node.selected {
                let check = gtk::Image::from_icon_name("object-select-symbolic");
                check.add_css_class("accent");
                row.add_prefix(&check);
            }

            let click_state = state.clone();
            let click_group = group.name.clone();
            let click_member = node.name.clone();
            row.connect_activated(move |_| {
                state::select_node(&click_state, &click_group, &click_member);
            });

            prefs.add(&row);
        }

        container.append(&prefs);
    }
}

/// Node names can contain markup-looking characters; rows use plain text.
fn glib_escape(text: &str) -> String {
    gtk::glib::markup_escape_text(text).to_string()
}

pub fn page(state: &Rc<AppState>) -> gtk::Widget {
    let (scroller, content) = widgets::page_container();

    state.subscribe(move |state| {
        if state.is_running() {
            let Some(api) = state.api() else { return };
            let state = state.clone();
            let content = content.clone();
            runtime::spawn(
                async move { api.proxies().await.map_err(|e| e.to_string()) },
                move |result| match result {
                    Ok(response) => {
                        let groups = from_controller(&state, &response);
                        populate(&state, &content, groups, true);
                    }
                    Err(err) => {
                        widgets::clear(&content);
                        content.append(&placeholder(t("Controller unreachable"), &err));
                    }
                },
            );
            return;
        }

        // No controller: fall back to the profile on disk so the page is still
        // useful, and so a pick can be remembered for the next start.
        match from_profile(state) {
            Ok(groups) => populate(state, &content, groups, false),
            Err(err) => {
                widgets::clear(&content);
                content.append(&placeholder(t("Core is not running"), &err));
            }
        }
    });

    scroller.upcast()
}
