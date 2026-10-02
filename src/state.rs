//! Shared application state and the actions the pages trigger. GTK is single
//! threaded, so this is plain `Rc`/`RefCell`; anything blocking is handed to the
//! tokio runtime in `runtime.rs`.

use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::api::ClashApi;
use crate::config::AppConfig;
use crate::corectl::{self as core, CoreStatus};
use crate::i18n::{t, tf};
use crate::{paths, runtime, subscription, sysproxy, template};

type Listener = Rc<dyn Fn(&Rc<AppState>)>;

pub struct AppState {
    pub config: RefCell<AppConfig>,
    pub status: RefCell<CoreStatus>,
    pub core_version: RefCell<Option<String>>,
    listeners: RefCell<Vec<Listener>>,
    toaster: RefCell<Option<adw::ToastOverlay>>,
    /// Set while a page rebuilds itself, so widget signals do not write back.
    refreshing: Cell<bool>,
    /// Offline latency results by node name, from the temporary test core. The
    /// live page reads the controller instead; this is only used with the core
    /// stopped.
    pub delays: RefCell<HashMap<String, u32>>,
    /// The group a temporary test core is measuring right now, if any.
    pub testing: RefCell<Option<String>>,
}

impl AppState {
    pub fn new() -> Rc<Self> {
        let config = AppConfig::load();
        crate::i18n::apply_setting(&config.language);
        Rc::new(Self {
            config: RefCell::new(config),
            status: RefCell::new(CoreStatus::Stopped),
            core_version: RefCell::new(None),
            listeners: RefCell::new(Vec::new()),
            toaster: RefCell::new(None),
            refreshing: Cell::new(false),
            delays: RefCell::new(HashMap::new()),
            testing: RefCell::new(None),
        })
    }

    pub fn attach_toaster(&self, overlay: &adw::ToastOverlay) {
        *self.toaster.borrow_mut() = Some(overlay.clone());
    }

    pub fn toast(&self, message: &str) {
        if let Some(overlay) = self.toaster.borrow().as_ref() {
            overlay.add_toast(adw::Toast::builder().title(message).timeout(4).build());
        } else {
            eprintln!("mihomo-manifold: {message}");
        }
    }

    /// Pages call this to rebuild themselves whenever the config or core changes.
    pub fn subscribe(self: &Rc<Self>, listener: impl Fn(&Rc<AppState>) + 'static) {
        self.listeners.borrow_mut().push(Rc::new(listener));
    }

    pub fn notify(self: &Rc<Self>) {
        let listeners = self.listeners.borrow().clone();
        let was_refreshing = self.refreshing.replace(true);
        for listener in listeners {
            listener(self);
        }
        self.refreshing.set(was_refreshing);
    }

    /// True while widgets are being repopulated programmatically.
    pub fn is_refreshing(&self) -> bool {
        self.refreshing.get()
    }

    pub fn save(self: &Rc<Self>) {
        if let Err(err) = self.config.borrow().save() {
            self.toast(&tf("Could not save settings: {}", &[&err]));
        }
    }

    /// Persist and tell every page to redraw.
    pub fn commit(self: &Rc<Self>) {
        self.save();
        self.notify();
    }

    pub fn api(&self) -> Option<ClashApi> {
        let cfg = self.config.borrow();
        ClashApi::new(&cfg.core.controller_url(), &cfg.core.secret).ok()
    }

    pub fn is_running(&self) -> bool {
        matches!(
            *self.status.borrow(),
            CoreStatus::Running | CoreStatus::Adopted
        )
    }
}

// ---------------------------------------------------------------- actions

/// Nodes for the active subscription, read from the cached profile.
pub fn active_proxies(state: &Rc<AppState>) -> Result<Vec<serde_yaml::Value>, String> {
    let cfg = state.config.borrow();
    let Some(sub) = cfg.active() else {
        return Err(t("Add a subscription first.").to_string());
    };
    match subscription::load_cached(sub) {
        Some(proxies) if !proxies.is_empty() => Ok(proxies),
        _ => Err(tf(
            "No downloaded profile for \"{}\" yet — update it first.",
            &[&sub.name],
        )),
    }
}

pub fn render_config(state: &Rc<AppState>) -> Result<String, String> {
    let proxies = active_proxies(state)?;
    let cfg = state.config.borrow();
    template::generate(&cfg, &proxies).map_err(|e| e.to_string())
}

/// Start the core, or hot-reload it if it is already up.
pub fn apply(state: &Rc<AppState>) {
    let yaml = match render_config(state) {
        Ok(yaml) => yaml,
        Err(err) => {
            state.toast(&err);
            return;
        }
    };

    if state.is_running() {
        if let Err(err) = paths::write_private(&paths::generated_config(), &yaml) {
            state.toast(&tf("Could not write the config: {}", &[&err]));
            return;
        }
        let Some(api) = state.api() else { return };
        let path = paths::generated_config().to_string_lossy().into_owned();
        let state = state.clone();
        runtime::spawn(
            async move { api.reload(&path).await.map_err(|e| e.to_string()) },
            move |result| {
                match result {
                    Ok(()) => {
                        state.toast(t("Configuration reloaded"));
                        // The pages read proxies from the core, so they have to
                        // query it again. refresh_status alone would not do it:
                        // it only redraws when the status itself changed, and a
                        // reload leaves the core exactly as running as it was.
                        state.notify();
                    }
                    Err(err) => state.toast(&tf("Reload failed: {}", &[&err])),
                }
                // The mode may have just changed from tunnel to proxy-only.
                sync_system_proxy(&state);
                refresh_status(&state);
            },
        );
        return;
    }

    // A test core shares the working directory with the real one; let it go
    // before the real one opens the same cache.db.
    core::stop_scratch();

    let cfg_snapshot = state.config.borrow().clone();
    if cfg_snapshot.core.tun_enabled {
        if let Some(warning) = core::tun_readiness(&cfg_snapshot.core.resolve_binary()).warning() {
            state.toast(&warning);
        }
    }

    if let Err(err) = core::start(&cfg_snapshot, &yaml) {
        *state.status.borrow_mut() = CoreStatus::Failed(err.to_string());
        state.toast(&format!("{err}"));
        state.notify();
        return;
    }

    // Give the core a moment to bind the controller, then confirm it is alive.
    let state = state.clone();
    let api = state.api();
    runtime::spawn(
        async move {
            let Some(api) = api else {
                return Err(t("controller unreachable").to_string());
            };
            for _ in 0..40 {
                if let Ok(version) = api.version().await {
                    return Ok(version);
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            Err(t("the core did not answer on the controller port").to_string())
        },
        move |result| match result {
            Ok(version) => {
                *state.status.borrow_mut() = CoreStatus::Running;
                *state.core_version.borrow_mut() = Some(version);
                sync_system_proxy(&state);
                state.toast(t("Core started"));
                state.notify();
                // A node picked while the core was down has never reached it.
                replay_selections(&state);
            }
            Err(err) => {
                let tail = core::tail_log(12);
                core::stop();
                *state.status.borrow_mut() = CoreStatus::Failed(err.clone());
                state.toast(&tf("{}. Check the Logs page.", &[&err]));
                if !tail.is_empty() {
                    eprintln!("mihomo-manifold: core log tail:\n{tail}");
                }
                state.notify();
            }
        },
    );
}

/// Point the desktop at the core in proxy-only mode, and take it back when the
/// core is not the one serving it any more. Only ever touches settings that
/// point at us, so a proxy the user configured themselves is left alone.
pub fn sync_system_proxy(state: &Rc<AppState>) {
    let (wanted, host, port) = {
        let cfg = state.config.borrow();
        (
            cfg.core.set_system_proxy && !cfg.core.tun_enabled && state.is_running(),
            "127.0.0.1".to_string(),
            cfg.core.mixed_port,
        )
    };

    if wanted {
        if !sysproxy::set(&host, port) {
            state.toast(t(
                "This desktop has no proxy settings to write (gsettings schema missing).",
            ));
        }
    } else if sysproxy::points_at(&host, port) {
        sysproxy::clear();
    }
}

pub fn stop(state: &Rc<AppState>) {
    if core::stop() {
        *state.status.borrow_mut() = CoreStatus::Stopped;
        // Do this before notifying: leaving the desktop pointed at a port that
        // no longer answers takes the whole session offline.
        sync_system_proxy(state);
        *state.core_version.borrow_mut() = None;
        state.notify();
        return;
    }

    // No child of ours, but the user asked for the proxy to be off. A core
    // answering the controller may be one this GUI started in an earlier
    // session and never got to clean up, and leaving it running after the
    // switch is turned off is the worse failure.
    let binary = state.config.borrow().core.binary.clone();
    let binary = if binary.trim().is_empty() {
        state.config.borrow().core.resolve_binary()
    } else {
        binary
    };
    let stopped = core::stop_external(&binary);

    if stopped.is_empty() {
        // Nothing matched. Saying "stopped" anyway would be a lie the poll
        // below undoes, and turning the system proxy off here would take the
        // whole session offline for a stop that never happened.
        state.toast(t(
            "No core was found to stop. Another program may be holding the port; \
             MihomoManifold can only stop the core it knows about.",
        ));
        return;
    }

    *state.status.borrow_mut() = CoreStatus::Stopped;
    sync_system_proxy(state);
    *state.core_version.borrow_mut() = None;
    state.notify();
    state.toast(&tf(
        "Stopped a core this app did not start ({}).",
        &[&stopped
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ")],
    ));
}

/// Probe the controller; also picks up a core started outside the GUI.
pub fn refresh_status(state: &Rc<AppState>) {
    let Some(api) = state.api() else { return };
    let child_alive = core::is_child_alive();
    let state = state.clone();
    runtime::spawn(async move { api.version().await.ok() }, move |version| {
        let previous = state.status.borrow().clone();
        let next = match (version.is_some(), child_alive) {
            (true, true) => CoreStatus::Running,
            (true, false) => CoreStatus::Adopted,
            (false, _) => match previous {
                CoreStatus::Failed(ref err) => CoreStatus::Failed(err.clone()),
                _ => CoreStatus::Stopped,
            },
        };
        *state.core_version.borrow_mut() = version;
        if next != previous {
            *state.status.borrow_mut() = next;
            state.notify();
        } else {
            *state.status.borrow_mut() = next;
        }
    });
}

/// Remember a node choice. When the core is up it is told immediately; when it
/// is stopped the choice is kept for the next start, which is what
/// [`replay_selections`] pushes once the controller answers.
pub fn select_node(state: &Rc<AppState>, group: &str, node: &str) {
    state
        .config
        .borrow_mut()
        .selected
        .insert(group.to_string(), node.to_string());
    state.save();

    if !state.is_running() {
        state.notify();
        state.toast(t("Saved — applied when the core starts."));
        return;
    }

    let Some(api) = state.api() else {
        state.notify();
        return;
    };
    let (group, node) = (group.to_string(), node.to_string());
    let state = state.clone();
    runtime::spawn(
        async move { api.select(&group, &node).await.map_err(|e| e.to_string()) },
        move |result| match result {
            Ok(()) => state.notify(),
            Err(err) => state.toast(&tf("Could not switch node: {}", &[&err])),
        },
    );
}

/// Keep the app's idea of the current node in step with the core, so the Nodes
/// page still shows it after the core is stopped. Written only when it actually
/// changed: this runs on every rebuild of that page.
pub fn note_selection(state: &Rc<AppState>, group: &str, node: &str) {
    let changed = {
        let mut cfg = state.config.borrow_mut();
        if cfg.selected.get(group).map(String::as_str) == Some(node) {
            false
        } else {
            cfg.selected.insert(group.to_string(), node.to_string());
            true
        }
    };
    if changed {
        state.save();
    }
}

/// Push node choices made while the core was stopped. `store-selected` only
/// remembers what the core itself was told, so a pick made offline has to be
/// re-sent before the pages read the controller's view.
fn replay_selections(state: &Rc<AppState>) {
    let wanted: Vec<(String, String)> = {
        let cfg = state.config.borrow();
        let groups = cfg.routing.group_names();
        cfg.selected
            .iter()
            .filter(|(group, _)| groups.iter().any(|g| g == *group))
            .map(|(group, node)| (group.clone(), node.clone()))
            .collect()
    };
    if wanted.is_empty() {
        return;
    }
    let Some(api) = state.api() else { return };

    let state = state.clone();
    runtime::spawn(
        async move {
            for (group, node) in wanted {
                // A group that does not take a manual pick simply refuses; the
                // offline choice just does not stick, and there is nothing to
                // report about it.
                let _ = api.select(&group, &node).await;
            }
        },
        move |()| state.notify(),
    );
}

// ---------------------------------------------------------------- latency

/// The URL a delay test fetches through each node. The live page sends the same
/// one to the controller.
pub const TEST_URL: &str = "https://cp.cloudflare.com/generate_204";
pub const TEST_TIMEOUT_MS: u32 = 3000;

/// A port nothing is listening on right now. It is bound and released, so there
/// is a brief race — the alternative is handing the core a socket we hold.
fn free_port() -> Option<u16> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .ok()?
        .local_addr()
        .ok()
        .map(|addr| addr.port())
}

/// Stops the temporary core when the test returns, however it returns.
struct ScratchGuard;

impl Drop for ScratchGuard {
    fn drop(&mut self) {
        core::stop_scratch();
    }
}

/// Measure one group through a throwaway core, so a node can be judged without
/// turning the real one on. Nothing here touches the system: the test core has
/// no TUN, binds loopback ports only and serves no traffic.
pub fn test_group_offline(state: &Rc<AppState>, group: &str) {
    if state.is_running() || state.testing.borrow().is_some() {
        return;
    }

    let proxies = match active_proxies(state) {
        Ok(proxies) => proxies,
        Err(err) => {
            state.toast(&err);
            return;
        }
    };
    let (cfg, secret) = {
        let cfg = state.config.borrow();
        (cfg.clone(), cfg.core.secret.clone())
    };

    // Two distinct ports; a collision would leave the controller unreachable.
    let (Some(controller_port), Some(mixed_port)) = (free_port(), free_port()) else {
        state.toast(t("Could not find a free port for the test core."));
        return;
    };

    *state.testing.borrow_mut() = Some(group.to_string());
    state.notify();
    state.toast(t("Testing through a temporary core…"));

    let state = state.clone();
    let group = group.to_string();
    runtime::spawn(
        run_scratch_test(cfg, proxies, controller_port, mixed_port, secret, group),
        move |result| {
            *state.testing.borrow_mut() = None;
            match result {
                Ok(delays) => {
                    let answered = delays.values().any(|delay| *delay > 0);
                    state.delays.borrow_mut().extend(delays);
                    state.notify();
                    if !answered {
                        state.toast(t("No node answered the latency test."));
                    }
                }
                Err(err) => {
                    state.notify();
                    state.toast(&tf("Latency test failed: {}", &[&err]));
                }
            }
        },
    );
}

/// The whole offline test as one future: bring up the throwaway core, wait for
/// its controller and ask it for the group's delays. Kept separate from the UI
/// plumbing so it can also be driven headlessly.
pub(crate) async fn run_scratch_test(
    cfg: AppConfig,
    proxies: Vec<serde_yaml::Value>,
    controller_port: u16,
    mixed_port: u16,
    secret: String,
    group: String,
) -> Result<HashMap<String, u32>, String> {
    let yaml = template::scratch_config(&cfg, &proxies, controller_port, mixed_port, &secret)
        .map_err(|e| e.to_string())?;

    core::start_scratch(&cfg, &yaml).map_err(|e| e.to_string())?;
    let _guard = ScratchGuard;

    let api = ClashApi::new(&format!("http://127.0.0.1:{controller_port}"), &secret)
        .map_err(|e| e.to_string())?;

    let mut ready = false;
    for _ in 0..40 {
        // `/version` answers as soon as the controller is bound, which can be
        // before the configuration — and so the groups — are registered; the
        // group endpoint fails until then. Readiness means the group is really
        // there, not merely that the port answers.
        match api.proxies().await {
            Ok(response) if response.proxies.contains_key(&group) => {
                ready = true;
                break;
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
        }
    }
    if !ready {
        return Err(t("the test core did not load the proxy groups").to_string());
    }

    let delays = api
        .group_delay(&group, TEST_URL, TEST_TIMEOUT_MS)
        .await
        .map_err(|e| e.to_string())?;

    let groups = cfg.routing.group_names();
    let members = template::group_views(&cfg, &proxies)
        .into_iter()
        .find(|view| view.name == group)
        .map(|view| view.members)
        .unwrap_or_default();

    Ok(mark_failed(delays, &members, &groups))
}

/// The core omits nodes it could not reach, which would leave them looking
/// untested. Record each as 0 — rendered as a timeout — so a run that failed is
/// visibly different from one that never happened. Members that are themselves
/// groups (AUTO) are not testable and are left untouched.
fn mark_failed(
    mut delays: HashMap<String, u32>,
    members: &[String],
    groups: &[String],
) -> HashMap<String, u32> {
    for member in members {
        if !groups.contains(member) {
            delays.entry(member.clone()).or_insert(0);
        }
    }
    delays
}

/// Download one subscription and remember what the panel reported.
pub fn update_subscription(state: &Rc<AppState>, id: &str, then_apply: bool) {
    let (sub, hwid) = {
        let cfg = state.config.borrow();
        let Some(sub) = cfg.subscription(id) else {
            return;
        };
        (sub.clone(), cfg.hwid.clone())
    };

    state.toast(&tf("Updating \"{}\"…", &[&sub.name]));
    let state = state.clone();
    let id = id.to_string();
    runtime::spawn(
        async move { subscription::fetch(sub, hwid).await },
        move |result| {
            {
                let mut cfg = state.config.borrow_mut();
                let Some(entry) = cfg.subscription_mut(&id) else {
                    return;
                };
                match &result {
                    Ok(fetched) => {
                        entry.last_error = None;
                        entry.last_updated = Some(chrono::Utc::now().timestamp());
                        entry.node_count = fetched.proxies.len();
                        entry.user_info = fetched.user_info;
                        if let Some(title) = &fetched.title {
                            if entry.name.trim().is_empty() || entry.name == "New subscription" {
                                entry.name = title.clone();
                            }
                        }
                    }
                    Err(err) => entry.last_error = Some(err.to_string()),
                }
                if cfg.active_subscription.is_none() {
                    cfg.active_subscription = Some(id.clone());
                }
            }
            match result {
                Ok(fetched) => {
                    state.toast(&tf("{} nodes downloaded", &[&fetched.proxies.len()]));
                    state.commit();
                    // The nodes just changed; last run's measurements are moot.
                    state.delays.borrow_mut().clear();
                    // Downloading only refreshes the file on disk. The core is
                    // still serving the nodes it was started with, so without a
                    // reload the new ones never reach the Nodes page.
                    let feeds_the_core = {
                        let cfg = state.config.borrow();
                        cfg.active().is_some_and(|active| active.id == id)
                    };
                    if then_apply || (feeds_the_core && state.is_running()) {
                        apply(&state);
                    }
                }
                Err(subscription::FetchError::DeviceLimit(message)) => {
                    state.commit();
                    show_device_limit(&state, &message);
                }
                Err(err) => {
                    state.toast(&tf("Update failed: {}", &[&err]));
                    state.commit();
                }
            }
        },
    );
}

/// The panel refused the device: show which HWID was sent and how to change it.
fn show_device_limit(state: &Rc<AppState>, message: &str) {
    let hwid = state.config.borrow().hwid.value();
    let dialog = adw::AlertDialog::builder()
        .heading(t("Device slot rejected"))
        .body(tf(
            "{}\n\nThis machine identifies itself as:\n{}\n\nFree a slot in the panel, or set a different HWID in Settings.",
            &[&message, &hwid],
        ))
        .build();
    dialog.add_response("close", t("Close"));
    dialog.add_response("settings", t("Open Settings"));
    dialog.set_response_appearance("settings", adw::ResponseAppearance::Suggested);

    let state_for_response = state.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "settings" {
            state_for_response.toast(t("Settings → Device identity"));
        }
    });

    if let Some(overlay) = state.toaster.borrow().as_ref() {
        if let Some(root) = overlay.root().and_downcast::<gtk::Window>() {
            dialog.present(Some(&root));
            return;
        }
    }
    state.toast(message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_nodes_are_recorded_as_timeouts() {
        let members = vec![
            "AUTO".to_string(),
            "DIRECT".to_string(),
            "n1".to_string(),
            "n2".to_string(),
        ];
        let groups = vec!["PROXY".to_string(), "AUTO".to_string()];
        let delays = HashMap::from([("n1".to_string(), 120u32)]);

        let marked = mark_failed(delays, &members, &groups);
        assert_eq!(marked.get("n1"), Some(&120)); // measured, kept as is
        assert_eq!(marked.get("n2"), Some(&0)); // tested, unreachable
        assert_eq!(marked.get("DIRECT"), Some(&0)); // a testable special
        assert_eq!(marked.get("AUTO"), None); // a group, not a node
    }
}
