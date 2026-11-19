# 架构与客户端

## 每台设备一个常驻节点

每台参与转发的电脑运行一个 Rust 后台程序 `tunnel-gateway`。它独占本机设备身份、监听端口和对端会话，开机加载规则，无人登录也能接受连接。CLI 和 GPUI 都通过本机 IPC 访问同一个后台。关闭管理界面后，网关持有的 TCP 转发继续运行。`tunnel connect` 的标准流属于该次调用，退出时会断开对应的连接。无桌面环境的主机只需安装后台和 CLI。

```text
管理     CLI / GPUI ── 本机 IPC ── tunnel-gateway
原生     本机应用 ── TCP ── tunnel-gateway A ══ iroh ══ tunnel-gateway B ── TCP ── 目标服务
终端     CLI / GPUI ── IPC ── 网关 A ══ p2p-terminal/1 ══ 网关 B ── PTY ── 本机 shell
终端     CLI / GPUI ── IPC ── 网关 A ══ iroh ══ 网关 B ── PTY / ConPTY
未来 Web 浏览器内的应用 ══ WebRTC DataChannel ══ tunnel-gateway B ── TCP ── 目标服务
```

例如，笔记本与家里电脑间可以复用一个会话承载多个 TCP 连接，同时与工作站保持另一条独立会话。对端离线时只影响发往该设备的连接。原生节点可使用公共或自托管的 `iroh-relay` 辅助建连和中继。设备配对、服务授权和 TCP 目标配置均由端点管理。首版可以只部署两个节点，按需要选用中继，无需额外的业务服务器。[iroh](https://github.com/n0-computer/iroh)

具备公网入站能力的节点还可以承担备用中继。设计上由管理员显式启用，在同一台机器上分别运行 `tunnel-gateway` 和独立的 `iroh-relay` 服务。其他设备可以直连此节点发布的服务，也可以借它转发两台设备之间的端到端加密流量。中继按设备公钥限制接入，业务服务仍由最终目标节点授权。这个角色属于 iroh 适配器，浏览器阶段的 WebRTC 中继另用 TURN。[备用中继与端口映射设计](connectivity.md#节点作为备用中继)

## 跨平台服务生命周期

| 首批目标 | 托管方式 | 本机管理通道 |
| --- | --- | --- |
| Linux（systemd） | 启用系统级 service unit，配置异常退出后重启 | Unix domain socket |
| Windows | 注册为自动启动的 Windows Service，通过 Service Control Manager 接收停止请求并配置故障恢复 | 仅接受本机客户端的命名管道 |
| macOS | 使用系统域的 LaunchDaemon，由 `launchd` 启动和监督，GPUI 在用户登录会话运行 | Unix domain socket |

三个平台共用同一个转发核心。平台相关代码负责服务启停、目录、权限与 IPC。开发时可前台运行同一后台入口。服务使用专用低权限身份，配置和密钥放在系统服务的数据目录。本机管理通道通过 Unix 文件权限或 Windows 访问控制列表限制给获准管理该节点的用户。[systemd 服务](https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html) · [Windows 服务](https://learn.microsoft.com/en-us/windows/win32/services/services) · [Apple Launch Daemons](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html)

启动时先加载持久身份和规则，提供本机管理接口。外网尚未就绪时仍可查看状态，网络恢复或设备唤醒后重新探测路径，按需建连并对连续失败退避。系统请求停止时停止接入，给活跃流有限的收尾时间，再关闭连接并释放端口。进程重启后恢复原身份和监听规则，原有 TCP 连接随进程终止，由业务客户端重新连接。

### 系统服务部署

先生成设备密钥、配置配对设备，并确认前台运行正常。使用系统服务时提供配置和密钥的绝对路径。

- **Linux**　示例 unit 使用 `p2p-tunnel` 账户和组。将二进制放在 `/usr/local/bin`，将 `node.toml` 和 `device.key` 放入 `/etc/p2p-tunnel`，私钥仅供服务账户读取。将 unit 安装到 `/etc/systemd/system` 后执行 `systemctl daemon-reload` 和 `systemctl enable --now p2p-tunnel-gateway`。本机操作员加入服务组后可通过 `/run/p2p-tunnel/gateway.sock` 管理网关。[systemd unit](../packaging/linux/p2p-tunnel-gateway.service)
- **macOS**　LaunchDaemon 模板使用 `_p2ptunnel` 账户和组。按模板准备程序与数据目录，预建 `run` 为服务账户和操作员组所有、权限 `0750`。将 root 所有的 plist 放入 `/Library/LaunchDaemons`，使用 `launchctl bootstrap system /Library/LaunchDaemons/org.p2ptunnel.gateway.plist` 加载。桌面应用保持在用户会话。[LaunchDaemon 模板](../packaging/macos/org.p2ptunnel.gateway.plist)
- **Windows**　在管理员 PowerShell 中按下例注册。先把文件放在示例路径并授权 LocalService 读取配置和密钥，限制其他账户读取私钥。`--windows-service` 启用 SCM 入口。使用系统服务时管理员通过本机管道管理，前台运行时管道所有者也可管理。

```powershell
New-Service -Name P2PTunnelGateway -BinaryPathName '"C:\P2PTunnel\tunnel-gateway.exe" --windows-service --config "C:\P2PTunnel\node.toml" --key "C:\P2PTunnel\device.key"' -StartupType Automatic
sc.exe config P2PTunnelGateway obj= "NT AUTHORITY\LocalService"
sc.exe failure P2PTunnelGateway reset= 86400 actions= restart/5000/restart/10000/restart/30000
Start-Service P2PTunnelGateway
```

在 Linux 上交叉构建 Windows x64 网关与 CLI 时，需要 Rust 的 `x86_64-pc-windows-gnu` 目标和 MinGW-w64 工具链。iroh 1.4.1 同时输出 Rust 库和 DLL。GNU ld 默认自动导出符号可能超出 PE 导出表上限，交叉链接 DLL 时使用 `-Wl,--exclude-all-symbols`。Windows 原生 MSVC 构建使用自身的链接器。桌面端使用各平台原生图形工具链构建。

## 可替换的 P2P 传输接口

当前 Rust 接口为 `Transport::open(peer)`，返回实现 Tokio `AsyncRead + AsyncWrite` 的双向流。入站适配器将经过认证的设备 ID 与流交给 `serve`。原生适配器封装会话复用，下面的表描述传输、会话和流的职责。`tcp-forward/1` 定义流上的应用协议，iroh 和 WebRTC 各自负责把它送到经过配对验证的对端。

| 接口 | 核心依赖的行为 |
| --- | --- |
| `Transport` | 主动连接指定设备，或接收入站会话。适配器向核心交付会话前，验证对端身份与应用协议版本。候选地址、打洞和中继都由适配器处理。 |
| `Session` | 提供已认证的 `PeerId`，支持主动打开和接收多条并发双向流。诊断探测可以观测路径，将网关会话的实际路径呈现到管理界面是后续工作。 |
| `BiStream` | 可靠、有序地读写字节，写入支持背压。`finish_send` 在已写数据之后发出本方向 EOF，对向仍能发送。`reset` 异常终止当前流。 |

**iroh 适配器（首版）** 持有 iroh Endpoint，用对端公钥验证身份，通过 QUIC ALPN 选择应用协议。每个本地 TCP 连接使用一条 QUIC 双向流。原生 IPv4/IPv6 探测、打洞和中继均在适配器内部完成。[iroh](https://github.com/n0-computer/iroh)

**WebRTC 适配器（浏览器阶段）** 由浏览器和支持 WebRTC 的原生后台共同实现。通过信令交换 SDP 和 ICE 候选，使用 ICE 选路，按需使用 TURN 中继。每条逻辑流映射到一条可靠、有序的 DataChannel。适配器把字节切分为有界的 `DATA` 消息，按协商的消息大小限制发送，并在发送缓冲积压时暂停写入。`FIN` 消息传递单向 EOF，使目标服务在收到 EOF 后仍可回包。双方完成收发后再关闭通道，提前关闭按流中断处理。浏览器消费通道数据，本机其他应用需要 TCP 监听入口时由原生后台提供。[WebRTC Data Channels](https://www.rfc-editor.org/rfc/rfc8831.html) · [RTCDataChannel](https://www.w3.org/TR/webrtc/#rtcdatachannel)

**传输选择与配对** 由发起方根据本机策略和已验证配对信息中双方共同支持的传输建连。现有 v1 配置中的 `endpoint_id` 对应 iroh 公钥，继续按 iroh 连接。未来的配对信息可以携带应用设备身份、支持的传输与各自的寻址信息，浏览器据此选用 WebRTC。WebRTC 建连时应验证配对设备对本次握手的身份绑定，例如双方用设备密钥签署包含会话随机数和各自 DTLS 指纹的握手记录，并核对实际连接的证书指纹。服务授权始终查询认证得到的 `PeerId`。无共同传输时返回明确错误，身份校验失败时结束建连。两种适配器可以同时运行，已打开的 TCP 流始终属于建立它的会话。

## CLI 与 GUI 的共同接口

CLI 与 GPUI 通过同一套带版本的本机请求和事件操作后台。已实现状态查询、导出票据和逐条暂停或恢复监听。暂停释放监听端口并保留已有流，重启会按 TOML 恢复规则。主页负责配对和设备列表，每台设备的设置标签通过本机 IPC 创建、修改和删除该设备的转发规则，设置页可以保存或清除 TOML 顶层的 `device_name`。网关校验配置并预绑定本地端口，再原子替换配置文件。远端端口由对方网关授权并绑定，设备页显示实际地址或最近的绑定错误。同一监听地址修改目标只影响新连接。每个对端的 终端授权和 TCP 端口授权可通过 CLI 或 GUI 在线保存。新配对的设备由两端网关在线写入并加载，手工编辑的命名服务在重启时加载。下表记录后续管理协议的目标。当前已经实现的命令见[项目首页](../README.md)。

| 操作 | 后台行为 |
| --- | --- |
| 导出本机配对信息、添加设备 | 导出公钥及寻址信息，将用户确认的对端身份保存到本机。设备别名只在本机使用。 |
| 发布、修改或撤销服务 | 保存目标地址和允许访问的设备。撤销授权时拒绝新请求，并结束该授权下仍活跃的转发流。 |
| 创建、修改或删除监听规则 | 尝试绑定本机地址。绑定失败时向调用方返回具体错误。删除规则时释放监听器，已经建立的流可继续完成。 |
| 获取快照、订阅变化 | 返回设备身份、规则、实际监听地址、会话路径、活跃流数和最近错误。事件订阅断开后重新取完整快照。 |

网关串行处理设置变更，本地预绑定或写盘失败时保留原有规则。保存的远端监听在对端可用时生效。各工作区窗口共用一个网关控制器，每个窗口维护自己的标签列表，每次显示一个选中的页面。启动时打开 Home，通过菜单打开 Settings，点击标签上的 × 可关闭。后续计划通过拖动标签在同一窗口内分屏。设备终端由其标签持有，跨窗口拖动会保留会话、终端内容和选区，关闭标签后释放对应的 PTY。

管理员也可以停止后台、手工修改 TOML，再启动加载。设备私钥与配置分开保存，本机管理接口可以读取或手动刷新设备配对码，私钥始终由网关持有。已有探测用密钥由管理员显式选用或导入。已配置的身份读取失败时应报错并保留原文件。

### GPUI 组件与主题

`apps/tunnel-gui/src/theme/palette.rs` 定义浅色和深色的语义颜色，包括成对的背景和前景、交互状态及终端 ANSI 调色板。共用间距和圆角在 `theme/mod.rs`。页面渲染时从当前窗口取得 `Ui`，用 `ui.section`、`ui.button`、`ui.icon_button` 和 `ui.labeled` 等组件组合页面。输入框、菜单、真实标签与拖动预览都使用同一套组件。新增控件或交互状态时，先复用已有颜色角色，需要新角色时同时填写两套调色板。

表单用 `Input::new` 编辑单行文本，用 `Textarea::new` 编辑多行文本，两者通过 `text()` 读取内容。`Select::new(id, value, on_change)` 接收当前值，通过 `.option(value, label)` 添加选项。Select 复用 Dropdown 的触发器、键盘导航和弹层。主题选择器使用 `.icon_trigger(icon, tooltip)`，菜单在当前模式旁显示勾选标记。

每个原生窗口创建一个 `DockState::new(DockSettings::default(), window, cx)`。渲染时用 `DockHeader`、`DockTabs` 和 `DockPanel` 组合布局，每个标签使用稳定的键。Home、Settings 和每台设备的设置页使用各自的滚动容器，原生终端用 `DockPanel::fill` 填满剩余空间并处理自己的滚动。

```rust
let header = DockHeader::new("P2P Tunnel", status, menu);
let tabs = DockTabs::new("workspace-tab-list", &self.dock)
    .item(Page::Home, home_tab)
    .item(Page::Settings, settings_tab);
DockLayout::new(header, tabs, DockPanel::scroll("home-scroll", home))
```

切换到 `DockPlacement::Left` 时，同一个标签栏会移至左侧。折叠状态和展开宽度由每个窗口的 `DockSettings` 保存，拆出的窗口继承这两项设置。Dock 组件测量标签及末尾空白区域，处理排序和拖动预览。工作区持有页面和终端会话，负责跨窗口转移及原生窗口事件。页面中的设备信息和权限列表用 `ListItem` 组合图标、说明和操作。编辑表单用 `ui.form_actions` 提供保存与取消，提交和校验由页面处理。

下拉菜单把普通按钮作为触发器，把选项和回调放入 `DropdownContent`。视图创建时用 `DropdownState::new(window, cx)` 保存一份状态，下面的 `open_settings` 是接收窗口和应用上下文的回调。

```rust
let trigger = DropdownTrigger::new(
    ui.button(Secondary, "workspace-menu", "Menu")
        .disclosure(menu.read(cx).is_open()),
);
let content = DropdownContent::new()
    .item("settings", Icon::Settings.label("Settings"), open_settings);
Dropdown::new(&menu, trigger, content)
```

外观偏好由 GUI 按桌面用户保存。`System` 从各窗口读取系统外观并订阅变化事件，手动切换会重绘所有窗口。终端按主题解析默认色和前 16 个 ANSI 色，保留应用指定的 256 色扩展和真彩色。

## 监听方向

本地监听端决定「在哪里接受连接」，服务端决定「允许连什么」。两台设备可以交换这两个角色。若要支持从 A 下令让 B 开监听的 `ssh -R` 式体验，需要另外定义远程创建、授权和回收监听器的控制操作。首版原生协议按监听器所在机器上的配置建立规则。

## 按服务名直接开流

TCP 监听器和本机 IPC 都可用「对端设备 + 服务名」打开已发布的 TCP 目标。目标网关核验设备公钥和访问授权，再连接服务地址。[原生开流握手](native-protocol.md#线上消息)

| 使用方式 | 发起端如何取得流 | 目标设备如何处理流 |
| --- | --- | --- |
| TCP 转发 | 客户端连接本机监听地址，例如 `127.0.0.1:8080` | 网关连接已发布的 TCP 服务 |
| 原生终端 | `tunnel shell <peer>` 或设备页的 **New session** 经本机 IPC 打开独立的远端 PTY。GUI 按设备列出已打开的会话。设备身份和逐对端授权用于鉴权，窗口尺寸直接传给主机 | Unix PTY 和 Windows ConPTY |
| 规划的 Web 应用 | 浏览器应用通过 WebRTC DataChannel 打开指定设备的服务 | 网关将流交给应用内的处理器，例如 HTTP/1.1 处理器，也可转给已有 TCP 服务 |

`tunnel connect <peer> <service>` 通过本机 IPC 请求常驻网关开流。CLI 将错误写到标准错误，标准输出专用于业务字节。远端接受服务后，网关在 IPC 回复成功并切换到数据模式。每帧由 4 字节大端长度和至多 32 KiB 的数据组成，零长度表示当前发送方向的 EOF。另一方向可继续发送。

命名服务可以承载 HTTP 等 TCP 业务，网关仍需网络套接字来建立直连或使用中继。浏览器内的应用可以显式读写 DataChannel。如果希望普通网页用地址栏或标准 `fetch()` 访问隧道内服务，则需提供浏览器可访问的 HTTP(S) 入口。[WebRTC DataChannel](https://www.w3.org/TR/webrtc/#rtcdatachannel)

## 工作区与落地顺序

| 现状 | 子项目 | 职责 |
| --- | --- | --- |
| 已有 | `apps/tunnel-gateway` | 管理本机 IPC、身份、TCP 监听器和系统服务生命周期。 |
| 已有 | `apps/tunnel-cli`、`apps/tunnel-gui` | CLI 与 GPUI 使用相同管理协议获取运行状态、导出票据、暂停或恢复转发。CLI 还提供独立探测。 |
| 已有 | `crates/tunnel-control` | 版本化的请求和快照，基于 Unix socket 或 Windows 本机命名管道。 |
| 已有 | `crates/tunnel-config`、`crates/tunnel-core` | TOML 校验以及独立于 iroh 的传输接口、服务授权、协议帧和 TCP 字节转发。 |
| 已有 | `crates/tunnel-terminal` | 交互终端的二进制帧、窗口尺寸、退出状态和跨平台 PTY 生命周期。 |
| 已有 | `crates/tunnel-native` | iroh 适配器，按设备复用 QUIC 会话，处理入站流和独立诊断探测。 |
| 规划 | WebRTC 适配器与 Web 客户端 | 接入相同的流协议，增加信令入口并按需部署 TURN。 |
