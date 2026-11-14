# MihomoManifold

<div align="center">
<img src="data/icons/io.github.cublae.MihomoManifold.svg" alt="MihomoManifold" width="128" height="128">
</div>

[English](README.md)

---

一个基于 GTK4/libadwaita 构建的 [mihomo](https://github.com/MetaCubeX/mihomo) 代理核心前端，围绕三个核心理念设计：携带设备标识符的订阅、用户可控的分流路由，以及由 GUI 完全掌控而非隐藏的核心。

## 功能特性
- **订阅 (Clash/Mihomo YAML)。** 仅从提供商处提取 `proxies:` 部分。其余所有内容（如 `tun`、`dns`、`proxy-groups`、`rules`）均根据您的本地设置生成，因此提供商更新配置时绝不会覆盖您的路由规则。配置文件会缓存在磁盘上，确保核心在无网络环境下仍能启动。
- **HWID 请求头。** 每次订阅请求均携带 `x-hwid`、`x-device-os`、`x-ver-os` 和 `x-device-model` 请求头，以适配 Remnawave 等面板的设备统计方式。该标识符是基于 `/etc/machine-id` 在特定应用命名空间下生成的 UUIDv5，因此原始 machine-id 永远不会离开本机，且在重启或切换 NixOS 世代后依然有效。当面板拒绝该设备时，应用会明确提示并显示其发送的确切 HWID，而非静默失败。
- **分流路由。** 支持按应用（`PROCESS-NAME` / `PROCESS-PATH`）、按目标（域名、后缀、关键字、正则、IP-CIDR、GEOIP、GEOSITE、端口）、按远程规则提供商进行分流，并提供原生代码块以处理 UI 未建模的任何规则。
- **实时控制。** 节点选择器支持按组进行延迟测试，并提供流量图表和核心日志，所有功能均通过外部控制器实现。当核心停止时，节点列表将从缓存的配置文件中展开，保持可浏览状态，此时选择的节点将在核心下次启动时生效。在此状态下进行延迟测试会启动一个一次性的临时核心进行测量——无 TUN、无回环端口，测试完成后立即销毁，确保真实核心保持关闭状态。

### 规则顺序
核心采用首次匹配原则，因此配置文件中各部分的编写顺序即为其完整行为逻辑：
1. 原生“全局前置”代码块
2. 应用规则
3. 私有网络绕过（`GEOIP,private` 以及 `.local` / `.lan`），若已启用
4. 目标规则
5. 规则提供商
6. 原生“默认动作前置”代码块
7. `MATCH,<默认目标>`

应用规则故意置于地理位置列表之上——若置于其下，这些规则将永远不会触发。启用任何应用规则也会自动将核心切换为 `find-process-mode: always`，否则 `PROCESS-*` 规则将静默失效。

## 在 NixOS 上安装
无需安装即可试用：
```sh
nix run github:cublae/mihomo-manifold
```
这将为您提供 GUI 以及一个纯 HTTP/SOCKS 代理端口。TUN 模式需要 NixOS 模块，因为授予 `CAP_NET_ADMIN` 属于系统级更改。

**flake.nix**
```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    mihomo-manifold.url = "github:cublae/mihomo-manifold";
  };
  outputs = { nixpkgs, mihomo-manifold, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        ./configuration.nix
        mihomo-manifold.nixosModules.default
      ];
    };
  };
}
```

**configuration.nix** — 特权部分：
```nix
programs.mihomo-manifold = {
  enable = true;
  users = [ "alice" ];   # 允许使用 capability 包装器的用户
};
```

**home.nix** — 应用程序及可选的声明式设置：
```nix
imports = [ inputs.mihomo-manifold.homeManagerModules.default ];
programs.mihomo-manifold = {
  enable = true;
  autostart = false;
};
```

`defaults` 是可选的。仅当需要覆盖内置默认值时才值得编写——作为每次重建时恢复的每台机器基准：
```nix
programs.mihomo-manifold.defaults = {
  # gvisor 是内置默认值；在支持的系统栈上速度更快
  core = { tun_stack = "system"; log_level = "warning"; };
  # 主机在面板设备列表中的名称
  hwid.device_model = "thinkpad-x1";
};
```
请勿将订阅 URL 写入其中：`defaults` 中的任何内容都会进入全局可读的 Nix store，而这些 URL 包含您的访问令牌。请在 UI 中添加它们——它们会以 0600 权限写入 `config.json`。

NixOS 模块会安装 `/run/wrappers/bin/mihomo` 并赋予其 `cap_net_admin,cap_net_raw,cap_net_bind_service+ep` 权限，同时通过 `MIHOMO_MANIFOLD_CORE` 环境变量将 GUI 指向该路径，因此 **GUI 本身永远不会以 root 身份运行**。

它还会添加一条 polkit 规则。在 TUN 模式下，核心通过运行 `resolvectl dns|domain|default-route mihomo-tun …` 将系统解析器指向自身的 DNS，而 systemd 将这些操作标记为 `auth_admin`——如果没有该规则，每次启动都会弹出身份验证对话框，关闭对话框会导致系统解析器绕过隧道。该规则授予 `org.freedesktop.resolve1.*` 权限，且仅授予允许运行核心的组成员；如果您宁愿手动输入密码，可通过 `programs.mihomo-manifold.tun.allowResolved = false;` 将其关闭。

首次重建后请注销并重新登录：组成员资格和 `MIHOMO_MANIFOLD_CORE` 环境变量仅在下次登录时才会传递到已运行的会话。

`defaults` 会合并到可变配置*之下*，且应用仅回写与默认值不同的设置——因此您在 UI 中从未修改过的值将继续遵循您的 Nix 配置，而您所做的任何更改都将优先生效。

## 防火墙
默认设置无需开放任何端口。在 `allow-lan` 关闭的情况下，核心仅绑定回环地址——代理端口 7890、控制器端口 9097 和 DNS 端口 1053——因此网络无法访问这些服务。
- **与其他机器共享代理。** 开启“允许局域网”仅会将*代理端口*移动到所有接口；控制器和 DNS 监听器仍保留在 127.0.0.1，因此局域网设备可以使用代理，但无法控制核心。请自行开放端口：`networking.firewall.allowedTCPPorts = [ 7890 ];`
- **TUN。** 核心会创建 `mihomo-tun` 设备及其自身的路由。对您本机发起的连接回复属于 `ESTABLISHED` 状态，默认的 NixOS 防火墙已接受该状态，因此无需额外规则。如果 TUN 已启动但无流量通过，请尝试 `networking.firewall.checkReversePath = "loose";`——反向路径过滤通常是隧道接口问题的常见原因。
- 核心不会自行编写防火墙规则：`auto-redirect` 处于关闭状态，因为它仅在主机转发其他设备流量时才有意义，否则它将成为您 nftables 规则集的第二个写入者。

## 文件路径
| 路径 | 内容 |
| --- | --- |
| `$XDG_CONFIG_HOME/mihomo-manifold/config.json` | 设置、订阅 URL (0600 权限) |
| `$XDG_CONFIG_HOME/mihomo-manifold/defaults.json` | 来自 home-manager 的声明式默认值 |
| `$XDG_STATE_HOME/mihomo-manifold/profiles/*.yaml` | 下载的订阅 (0600 权限) |
| `$XDG_STATE_HOME/mihomo-manifold/core/config.yaml` | 生成的配置 |
| `$XDG_STATE_HOME/mihomo-manifold/core/scratch.yaml` | 延迟测试提供给临时核心的废弃配置 |
| `$XDG_STATE_HOME/mihomo-manifold/core.log` | 核心标准输出/标准错误 |
| `$XDG_STATE_HOME/mihomo-manifold/scratch.log` | 临时测试核心标准输出/标准错误 |

订阅 URL 包含访问令牌，并以纯文本 0600 权限文件存储，而非密钥环——因为核心无论如何都需要明文读取它们。

## 在 Debian 和 Ubuntu 上安装
发布版本包含一个独立的 `.deb` 包：包含 GUI、其驱动的 mihomo 核心、桌面入口和图标。
```sh
sudo apt install ./mihomo-manifold_0.1.0_amd64.deb
```
核心安装在 `/usr/lib/mihomo-manifold/mihomo`，`postinst` 脚本会为其授予 `cap_net_admin,cap_net_raw,cap_net_bind_service` 权限，因此 TUN 模式在首次启动时即可工作，且 GUI 本身永远不会以 root 身份运行。`/usr/bin` 中的包装器将 GUI 指向该核心；可设置 `MIHOMO_MANIFOLD_CORE` 环境变量以覆盖此行为。

构建该软件包需要与应用程序编译时相同的 GTK 版本——4.18 或更新版本（这排除了 Ubuntu 24.04）——以及 `dpkg-dev` 和 `librsvg2-bin`：
```sh
cargo build --release
packaging/build-deb.sh          # -> result/mihomo-manifold_<version>_<arch>.deb
```
`MIHOMO_VERSION` 用于固定核心版本，默认使用下表中的版本。它被刻意设计为独立于应用程序自身的版本：新核心是一个安全决策，不应由 GUI 重建静默引入。设置 `MIHOMO_BIN` 可打包已下载的核心，这也是离线路径和测试所使用的。

`.github/workflows/build-deb.yml` 会在出现 `v*` 标签时运行相同的脚本，并发布附带该包的 GitHub release。标签必须与 `Cargo.toml` 中的版本匹配；若不匹配，工作流将明确报错。
`test.yml` 在每次推送和拉取请求时运行测试。运行器固定为 `ubuntu-26.04` 而非 `ubuntu-latest`，因为 `ubuntu-latest` 在两个月的迁移窗口内才会升级到 26.04，而标签构建不能在其标签下发生更改。

打包和发布流程记录在 [docs/packaging/debian.md](docs/packaging/debian.md) 和 [docs/RELEASE.md](docs/RELEASE.md) 中。

## 开发
```sh
nix develop
cargo test
cargo run
```
`mihomo-manifold --print-config` 会渲染将传递给核心的配置并退出，这与 `mihomo -t -f` 配合使用效果很好：
```sh
mihomo-manifold --print-config > /tmp/c.yaml && mihomo -t -f /tmp/c.yaml
```
