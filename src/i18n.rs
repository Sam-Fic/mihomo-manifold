//! Hand-rolled i18n. English source text doubles as the message key, so an
//! untranslated string simply renders as itself; adding a language means adding
//! a table, not touching the pages.
//!
//! The active language is a process-global atomic rather than part of
//! `AppState`: the tray menu is built on the tokio thread and still needs it.
//! Pages rebuild themselves through `AppState::subscribe` on every commit, so
//! changing the language plus a `state.commit()` retranslates the whole UI.
//! Toasts, dialogs and corectl messages call `t()` at display time and follow
//! the change on the next string that gets produced.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Lang {
    En = 0,
    Zh = 1,
}

static LANG: AtomicU8 = AtomicU8::new(Lang::En as u8);

pub fn set(lang: Lang) {
    LANG.store(lang as u8, Ordering::Relaxed);
}

pub fn get() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        x if x == Lang::Zh as u8 => Lang::Zh,
        _ => Lang::En,
    }
}

/// Resolve a configured value — `"auto"`, `"en"`, `"zh"` — and activate it.
pub fn apply_setting(value: &str) {
    set(match value {
        "en" => Lang::En,
        "zh" => Lang::Zh,
        _ => detect_system(),
    });
}

/// LANG / LC_MESSAGES / LC_ALL, whichever the locale set first that is non-empty.
fn detect_system() -> Lang {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        let v = std::env::var(var).unwrap_or_default();
        if v.is_empty() {
            continue;
        }
        return if v.starts_with("zh") {
            Lang::Zh
        } else {
            Lang::En
        };
    }
    Lang::En
}

/// Translate and fill `{}` placeholders left to right.
pub fn tf(key: &'static str, args: &[&dyn std::fmt::Display]) -> String {
    fill(t(key), args)
}

/// Translate a string; falls back to the English key when a table has no entry.
pub fn t(key: &'static str) -> &'static str {
    translate(get(), key)
}

/// [`t`] against an explicit language, without the global — for tests.
pub fn translate(lang: Lang, key: &'static str) -> &'static str {
    if lang == Lang::Zh {
        if let Some(value) = ZH.iter().find(|(k, _)| *k == key).map(|(_, v)| *v) {
            return value;
        }
    }
    key
}

/// [`tf`] against an explicit template, for tests.
pub fn fill(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    let mut args = args.iter();
    while let Some(pos) = rest.find("{}") {
        out.push_str(&rest[..pos]);
        if let Some(arg) = args.next() {
            use std::fmt::Write;
            let _ = write!(out, "{arg}");
        }
        rest = &rest[pos + 2..];
    }
    out.push_str(rest);
    out
}

// ------------------------------------------------------------------ 简体中文

const ZH: &[(&str, &str)] = &[
    // ---- 页面标题 / shell ----
    ("Dashboard", "仪表盘"),
    ("Nodes", "节点"),
    ("Routing", "分流"),
    ("Subscriptions", "订阅"),
    ("Logs", "日志"),
    ("Settings", "设置"),
    // ---- widgets ----
    ("Cancel", "取消"),
    ("no expiry", "长期有效"),
    ("unknown", "未知"),
    ("never", "从未"),
    // ---- 仪表盘 ----
    ("Core", "内核"),
    ("Stopped", "已停止"),
    ("The proxy core is not running", "代理内核未在运行"),
    ("Active subscription", "当前订阅"),
    ("none", "无"),
    ("Mode", "模式"),
    ("Tunnel (TUN) — captures everything", "全局隧道（TUN）— 接管全部流量"),
    ("Proxy only — one port, no privileges", "仅代理 — 单端口，无需特权"),
    ("Proxy address", "代理地址"),
    ("Copy", "复制"),
    ("—", "—"),
    ("Traffic", "流量"),
    ("Download", "下载"),
    ("Upload", "上传"),
    ("This session", "本次会话"),
    ("Quick actions", "快捷操作"),
    ("Apply configuration", "应用配置"),
    (
        "Regenerate config.yaml from your rules and reload the core",
        "根据你的规则重新生成 config.yaml 并重载内核",
    ),
    ("Update active subscription", "更新当前订阅"),
    ("Download nodes again and reload", "重新下载节点并重载"),
    ("Proxy address copied", "已复制代理地址"),
    ("Running", "运行中"),
    ("mihomo {}, started by MihomoManifold", "mihomo {}，由 MihomoManifold 启动"),
    ("started by MihomoManifold", "由 MihomoManifold 启动"),
    ("Running (external)", "运行中（外部）"),
    (
        "A core this app did not start is running. Turning the switch off will stop it.",
        "有一个非本程序启动的内核正在运行。关闭开关会将其停止。",
    ),
    (
        "No core was found to stop. Another program may be holding the port; MihomoManifold can only stop the core it knows about.",
        "未找到可停止的内核。可能是其他程序占用了端口；MihomoManifold 只能停止自己管理的内核。",
    ),
    (
        "Stopped a core this app did not start ({}).",
        "已停止一个非本程序启动的内核（{}）。",
    ),
    ("Failed to start", "启动失败"),
    ("{} — {} nodes", "{} — {} 个节点"),
    ("127.0.0.1:{} (HTTP and SOCKS){}", "127.0.0.1:{}（HTTP 与 SOCKS）{}"),
    (", published to the desktop", "，已发布为系统代理"),
    ("stack {}", "协议栈 {}"),
    ("port {}", "端口 {}"),
    ("{} · {} · {}", "{} · {} · {}"),
    ("{} rule", "{} 条规则"),
    ("{} rules", "{} 条规则"),
    ("{} app rule", "{} 条应用规则"),
    ("{} app rules", "{} 条应用规则"),
    ("0 B/s", "0 B/s"),
    ("0 B", "0 B"),
    ("peak {}/s", "峰值 {}/s"),
    ("Add a subscription first.", "请先添加订阅。"),
    // ---- 节点页 ----
    ("No proxy groups", "没有代理组"),
    (
        "The core is running but reports no groups. Apply the configuration first.",
        "内核正在运行，但没有报告任何代理组。请先应用配置。",
    ),
    ("{} · {} nodes", "{} · {} 个节点"),
    ("Test", "测速"),
    ("Latency test failed: {}", "延迟测试失败：{}"),
    ("Core is not running", "内核未运行"),
    (
        "Start the core on the Dashboard to browse and switch nodes.",
        "请在仪表盘页启动内核，然后浏览并切换节点。",
    ),
    ("Controller unreachable", "无法连接控制端口"),
    ("Could not switch node: {}", "切换节点失败：{}"),
    // ---- 订阅页 ----
    ("Name", "名称"),
    ("Subscription", "订阅"),
    ("Subscription URL", "订阅地址"),
    ("Device identity", "设备标识"),
    (
        "Sends x-hwid, x-device-os, x-ver-os and x-device-model with the request, the way Remnawave counts devices.",
        "随请求发送 x-hwid、x-device-os、x-ver-os 和 x-device-model，与 Remnawave 面板的设备计数方式一致。",
    ),
    ("Send HWID headers", "发送 HWID 请求头"),
    ("This device", "本机"),
    ("Updates", "更新"),
    ("Auto-update interval", "自动更新间隔"),
    ("Minutes; 0 disables automatic updates", "单位：分钟；0 表示禁用自动更新"),
    ("Extra headers", "附加请求头"),
    (
        "One per line as `Key: value`. `{hwid}` is replaced with this device's identifier.",
        "每行一条，格式为 `Key: value`。`{hwid}` 会替换为本机标识。",
    ),
    ("Add subscription", "添加订阅"),
    ("Edit subscription", "编辑订阅"),
    ("Add", "添加"),
    ("Save", "保存"),
    ("A subscription URL is required.", "必须填写订阅地址。"),
    ("Remove subscription?", "删除订阅？"),
    (
        "\"{}\" and its downloaded nodes will be deleted from this machine.",
        "将从本机删除“{}”及其已下载的节点。",
    ),
    ("Remove", "移除"),
    ("{} nodes", "{} 个节点"),
    ("{} of {} used, {} left", "已用 {} / {}，剩余 {}"),
    ("expires {}", "{} 到期"),
    ("updated {}", "更新于 {}"),
    ("never updated", "从未更新"),
    (
        "Only the nodes are taken from the provider — routing stays yours.",
        "只从机场获取节点 — 分流规则完全由你掌控。",
    ),
    ("No subscriptions yet", "还没有订阅"),
    (
        "Add the URL your panel gave you; the HWID headers are sent automatically.",
        "添加面板提供的订阅地址即可；HWID 请求头会自动发送。",
    ),
    ("Use this subscription", "使用此订阅"),
    ("Update now", "立即更新"),
    ("Edit", "编辑"),
    // ---- 日志页 ----
    ("Level requested from the core", "向内核请求的日志级别"),
    ("Filter", "过滤"),
    ("Pause", "暂停"),
    ("Clear", "清空"),
    (
        "The core writes its own log to $XDG_STATE_HOME/mihomo-manifold/core.log as well.",
        "内核也会把自身的日志写入 $XDG_STATE_HOME/mihomo-manifold/core.log。",
    ),
    // ---- 分流页 ----
    ("Application", "应用程序"),
    (
        "Matching by process requires TUN mode; a plain system proxy cannot see which program opened a connection.",
        "按进程匹配需要开启 TUN 模式；普通的系统代理无法识别连接是由哪个程序发起的。",
    ),
    ("Custom…", "自定义…"),
    ("Installed application", "已安装的应用"),
    ("Process name or path", "进程名或路径"),
    ("Match by", "匹配方式"),
    ("Process name", "进程名"),
    ("Executable path", "可执行文件路径"),
    ("Send through", "出口"),
    ("Add application rule", "添加应用规则"),
    (
        "Enter a process name, for example telegram-desktop.",
        "请输入进程名，例如 telegram-desktop。",
    ),
    ("Destination", "目标"),
    (
        "With fake-ip DNS on, address matchers such as GEOIP resolve the destination — otherwise they would never match a domain.",
        "开启 fake-ip DNS 后，GEOIP 等地址类匹配会先解析真实目标 — 否则它们永远匹配不到域名。",
    ),
    ("Match", "匹配类型"),
    ("Value", "值"),
    ("Add routing rule", "添加分流规则"),
    ("Enter a value to match.", "请输入要匹配的值。"),
    ("Rule provider", "规则提供器"),
    (
        "A remote list the core downloads and refreshes on its own.",
        "由内核自行下载并定时刷新的远程规则列表。",
    ),
    ("URL", "URL"),
    ("Behavior", "数据类型"),
    ("Format", "文件格式"),
    ("Refresh interval (seconds)", "刷新间隔（秒）"),
    ("Add rule provider", "添加规则提供器"),
    ("A provider needs both a name and a URL.", "规则提供器必须同时填写名称和 URL。"),
    ("Rule order", "规则顺序"),
    (
        "Rules are evaluated top to bottom and the first match wins: application rules, then the private-network bypass, then destination rules, then rule providers, and finally the default action.",
        "规则自上而下匹配，命中第一条即生效：先是应用规则，然后是内网绕过，然后是目标规则，然后是规则提供器，最后是默认动作。",
    ),
    ("Everything else", "其余流量"),
    ("The final MATCH rule", "即最终的 MATCH 规则"),
    ("Applications", "按应用分流"),
    (
        "Route individual programs, whatever they connect to.",
        "按程序分流，无论它们连接什么目标。",
    ),
    (
        "⚠ TUN is disabled in Settings — process rules cannot match without it.",
        "⚠ 设置中未开启 TUN — 没有它，进程规则无法生效。",
    ),
    ("No application rules", "暂无应用规则"),
    (
        "For example: Steam direct, browser through the tunnel.",
        "例如：Steam 直连，浏览器走代理。",
    ),
    ("Domains, IP and geo", "域名 / IP / 地理"),
    ("No destination rules", "暂无目标规则"),
    (
        "Local traffic already bypasses the tunnel when the private-network bypass is on.",
        "开启内网绕过之后，本地流量已经不会进入隧道。",
    ),
    ("Rule providers", "规则提供器"),
    (
        "Remote lists such as antifilter or a geosite mirror.",
        "例如 antifilter 或 geosite 镜像之类的远程列表。",
    ),
    ("Raw rules", "原始规则"),
    (
        "Written verbatim. The first block goes above everything generated, the second just before the default action.",
        "原样写入。第一块位于所有生成的规则之前，第二块紧挨在默认动作之前。",
    ),
    ("Before everything", "位于最前"),
    ("Just before the default action", "默认动作之前"),
    ("Save raw rules", "保存原始规则"),
    (
        "Raw rules saved — apply the configuration to use them.",
        "已保存原始规则 — 需要应用配置后才会生效。",
    ),
    // ---- 设置页 ----
    (
        "Changes take effect the next time you apply the configuration.",
        "修改将在下次应用配置时生效。",
    ),
    ("mihomo binary", "mihomo 程序路径"),
    ("Resolved to", "实际解析为"),
    ("Mixed proxy port", "混合代理端口"),
    ("HTTP and SOCKS on one port", "HTTP 与 SOCKS 共用一个端口"),
    ("Publish as system proxy", "发布为系统代理"),
    (
        "In proxy-only mode, point the desktop's proxy settings at the port",
        "仅代理模式下，把桌面环境的代理设置指向该端口",
    ),
    (
        "Unavailable: this session has no org.gnome.system.proxy schema",
        "不可用：当前会话没有 org.gnome.system.proxy 配置项",
    ),
    ("TLS fingerprint", "TLS 指纹"),
    (
        "What every node pretends to be during the handshake",
        "所有节点在握手时统一伪装成的指纹",
    ),
    ("As the subscription says", "以订阅自带配置为准"),
    ("Controller port", "控制端口"),
    ("Where the GUI talks to the core", "GUI 与内核通信的端口"),
    ("Controller secret", "控制密钥"),
    ("Generate a new secret", "生成新密钥"),
    (
        "New secret generated — restart the core to use it.",
        "已生成新密钥 — 重启内核后生效。",
    ),
    ("Log level", "日志级别"),
    ("Allow LAN", "允许局域网连接"),
    ("Let other machines use this proxy port", "允许局域网内其他设备使用该代理端口"),
    ("IPv6", "IPv6"),
    ("Start the core when the app opens", "应用启动时自动启动内核"),
    (
        "Start the core without a password prompt",
        "启动内核时不弹密码框",
    ),
    (
        "Install a polkit rule for the three systemd-resolved actions TUN needs. Without it every start asks for your password three times.",
        "为 TUN 所需的三个 systemd-resolved 操作安装一条 polkit 规则。不安装的话，每次启动都会三次询问密码。",
    ),
    (
        "Rule installed — the core will start without asking.",
        "规则已安装 — 启动内核时不再询问密码。",
    ),
    (
        "Rule removed — starting the core will ask for your password.",
        "规则已移除 — 下次启动内核将再次要求密码。",
    ),
    (
        "Required for routing by application and for UDP traffic.",
        "按应用分流以及 UDP 流量都依赖它。",
    ),
    ("Capture all traffic (TUN)", "接管全部流量（TUN）"),
    ("Privileges", "权限"),
    ("Network stack", "网络协议栈"),
    ("Keep local networks off the tunnel", "本地网络不走隧道"),
    (
        "Adds a private-IP and .local/.lan bypass above your destination rules",
        "在目标规则之前添加私有 IP 与 .local/.lan 绕过",
    ),
    ("fake-ip DNS", "fake-ip DNS"),
    (
        "Faster and needed for reliable domain rules under TUN",
        "速度更快，且 TUN 模式下域名规则必须依赖它",
    ),
    (
        "Sent with every subscription request. Panels that enforce a device limit count these.",
        "随每个订阅请求发送。启用设备数限制的面板会统计它们。",
    ),
    ("HWID source", "HWID 来源"),
    ("Derived from /etc/machine-id", "由 /etc/machine-id 派生"),
    ("Entered manually", "手动输入"),
    ("Current HWID", "当前 HWID"),
    ("HWID", "HWID"),
    ("HWID copied", "已复制 HWID"),
    ("x-device-os", "x-device-os"),
    ("x-ver-os", "x-ver-os"),
    ("x-device-model", "x-device-model"),
    ("User-Agent", "User-Agent"),
    ("Advanced", "高级"),
    ("Preview generated config.yaml", "预览生成的 config.yaml"),
    ("Exactly what the core is fed", "查看内核实际加载的配置"),
    ("Files", "文件位置"),
    ("Generated config.yaml", "生成的 config.yaml"),
    ("Copied to clipboard", "已复制到剪贴板"),
    ("Appearance", "外观"),
    ("Language", "语言"),
    ("Follow system", "跟随系统"),
    (
        "Applies immediately to every page.",
        "立即应用到所有页面。",
    ),
    // ---- 状态与操作 ----
    ("Could not save settings: {}", "无法保存设置：{}"),
    (
        "No downloaded profile for \"{}\" yet — update it first.",
        "还没有已下载的“{}”配置文件 — 请先更新订阅。",
    ),
    ("Configuration reloaded", "配置已重载"),
    ("Could not write the config: {}", "写入配置文件失败：{}"),
    ("Reload failed: {}", "重载失败：{}"),
    ("Core started", "内核已启动"),
    ("{}. Check the Logs page.", "{}。请查看日志页。"),
    ("controller unreachable", "无法连接控制端口"),
    (
        "the core did not answer on the controller port",
        "内核没有在控制端口上应答",
    ),
    (
        "This desktop has no proxy settings to write (gsettings schema missing).",
        "当前桌面环境没有可写入的代理设置（缺少 gsettings schema）。",
    ),
    ("Updating \"{}\"…", "正在更新“{}”…"),
    ("{} nodes downloaded", "已下载 {} 个节点"),
    ("Update failed: {}", "更新失败：{}"),
    ("Device slot rejected", "设备名额被拒绝"),
    (
        "{}\n\nThis machine identifies itself as:\n{}\n\nFree a slot in the panel, or set a different HWID in Settings.",
        "{}\n\n本机使用的标识为：\n{}\n\n请在面板中释放一个设备名额，或在设置中改用其他 HWID。",
    ),
    ("Close", "关闭"),
    ("Open Settings", "打开设置"),
    ("Settings → Device identity", "设置 → 设备标识"),
    // ---- 内核进程 ----
    (
        "TUN is on but no core binary was found at {}. Set its path in Settings.",
        "已开启 TUN，但在 {} 未找到内核程序。请在设置中指定其路径。",
    ),
    (
        "TUN is on and {} is set up correctly, but this session may not run it. Log out and back in so the mihomo group applies.",
        "已开启 TUN，{} 的配置也正确，但当前会话可能无权运行它。请注销后重新登录，使 mihomo 用户组生效。",
    ),
    (
        "TUN is on but {} has no CAP_NET_ADMIN. Enable programs.mihomo-manifold.tun in your NixOS configuration.",
        "已开启 TUN，但 {} 缺少 CAP_NET_ADMIN 能力。请在 NixOS 配置中启用 programs.mihomo-manifold.tun。",
    ),
    ("The core binary can create the TUN device.", "内核程序可以创建 TUN 设备。"),
    (
        "getcap is not installed, so privileges could not be checked.",
        "未安装 getcap，无法检查权限。",
    ),
    ("mihomo binary not found: {}\nSet its path in Settings.", "未找到 mihomo 程序：{}\n请在设置中指定其路径。"),
    // ---- 托盘 ----
    ("Core is running", "内核正在运行"),
    ("Core is stopped", "内核已停止"),
    ("Open MihomoManifold", "打开 MihomoManifold"),
    ("Start the core", "启动内核"),
    ("Stop the core", "停止内核"),
    ("Quit", "退出"),
    // ---- 分流出口标签 ----
    ("Direct", "直连"),
    ("Reject", "拦截"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_passthrough_and_fallback() {
        assert_eq!(translate(Lang::En, "Dashboard"), "Dashboard");
        assert_eq!(translate(Lang::Zh, "not in any table"), "not in any table");
    }

    #[test]
    fn chinese_lookup_and_formatting() {
        assert_eq!(translate(Lang::Zh, "Dashboard"), "仪表盘");
        assert_eq!(
            fill(translate(Lang::Zh, "{} nodes downloaded"), &[&42]),
            "已下载 42 个节点"
        );
        assert_eq!(
            fill(translate(Lang::Zh, "Latency test failed: {}"), &[&"boom"]),
            "延迟测试失败：boom"
        );
        // Extra or missing placeholders must not corrupt the text.
        assert_eq!(fill("a {} b", &[&1, &2]), "a 1 b");
        assert_eq!(fill("a {} b {}", &[&1]), "a 1 b ");
    }

    #[test]
    fn the_external_core_wording_is_translated() {
        // English source text is the lookup key, so editing a string in the UI
        // silently drops it back to English. These are easy to break and
        // invisible until someone runs the app in Chinese.
        assert_eq!(
            translate(
                Lang::Zh,
                "A core this app did not start is running. Turning the switch off will stop it."
            ),
            "有一个非本程序启动的内核正在运行。关闭开关会将其停止。"
        );
        assert_eq!(
            translate(
                Lang::Zh,
                "No core was found to stop. Another program may be holding the port; \
                 MihomoManifold can only stop the core it knows about."
            ),
            "未找到可停止的内核。可能是其他程序占用了端口；MihomoManifold 只能停止自己管理的内核。"
        );
        assert_eq!(
            translate(Lang::Zh, "Stopped a core this app did not start ({})."),
            "已停止一个非本程序启动的内核（{}）。"
        );
    }

    #[test]
    fn tables_are_well_formed() {
        // No duplicate keys, and every template placeholder count matches.
        let mut seen = std::collections::HashSet::new();
        for (key, value) in ZH {
            assert!(seen.insert(*key), "duplicate key: {key}");
            assert_eq!(
                key.matches("{}").count(),
                value.matches("{}").count(),
                "placeholder mismatch: {key}"
            );
        }
    }

    #[test]
    fn setting_resolves_to_a_concrete_language() {
        // Only ever switches *to* English: the corectl tests assert English
        // strings and run in parallel, and English is the default anyway.
        apply_setting("en");
        assert_eq!(get(), Lang::En);
        assert_eq!(t("Dashboard"), "Dashboard");
    }
}
