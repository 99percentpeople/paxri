# P2P Tunnel

P2P Tunnel 面向处在不同网络的两台或多台电脑，提供类似 SSH 端口转发的体验。以访问家里的开发服务为例，笔记本在 `127.0.0.1:8080` 接受连接，家里的节点连接 `127.0.0.1:3000`。两个本地 TCP 连接之间使用加密的 P2P 隧道。

```text
浏览器 ──TCP── 笔记本节点 ══ QUIC 双向流 ══ 家里节点 ──TCP── 开发服务
                            直连或中继
```

## 当前可以使用

工作区提供三个 Rust 程序。`tunnel-gateway` 常驻并持有设备密钥、iroh 会话和 TCP 监听器。`tunnel` 是命令行客户端，也保留独立的身份、配置检查和网络诊断命令。`tunnel-gui` 是 GPUI 桌面客户端，通过与 CLI 相同的本机接口操作网关。

### 构建与运行

默认构建网关和 CLI。桌面端单独构建，无桌面的主机可以跳过 GPUI 依赖。Linux 构建桌面端需要 GPUI 的开发库，例如 Debian 的 `libfontconfig1-dev`、`libxkbcommon-dev`、`libxkbcommon-x11-dev`、`libwayland-dev` 和 `libvulkan-dev`，运行时需要图形会话和 Vulkan 驱动。

```sh
cargo build --locked
cargo build --locked -p tunnel-gui
target/debug/tunnel config check examples/home.toml
target/debug/tunnel config check examples/laptop.toml
cargo test --workspace --locked
python3 scripts/check-gateway.py
```

在 Windows 桌面安装 pywin32 后，可运行 `py scripts/check-tab-drag-windows.py`。脚本在独立窗口里检查按下未选中标签时的页面切换、排序和拆窗阈值、单标签拖窗，以及连续拖出和并入时的键盘焦点。

### 同步源码到 Windows

在开发机的 Git 工作区运行以下命令，两端需要 Python 3.9 或更高版本，开发机需要可以免交互登录目标 Windows 的 OpenSSH 客户端。默认 SSH 主机为 `yzzpc`，目标目录为 Windows 用户主目录下的 `p2p-tunnel`。

```sh
python3 scripts/sync-windows.py          # 立即同步一次
python3 scripts/sync-windows.py --watch  # 持续同步本地修改，Ctrl+C 退出
```

可用 `--host` 指定其他 Windows SSH 主机。脚本同步根目录的 Cargo 文件、README、.gitignore 和项目源码目录，包括尚未提交的新源码。每次写入都会核对 SHA-256；Windows 上独立修改过的文件会报告冲突，手工合并后可继续。监听模式遇到断线会自动重试。由脚本同步的文件在本地删除后，只有 Windows 副本仍匹配上次同步内容时才会删除。构建产物和运行配置、私钥保存在同步范围之外。只使用源码同步模式时，在 Windows 项目目录执行 `cargo build --workspace --locked` 并重新启动应用。

### 自动更新两端的开发程序

在 Linux 开发机运行 `python3 scripts/sync-windows.py --watch --dev`，会持续把源码同步到 `yzzpc`。Rust 源码或 Cargo 配置变化时，两台机器各自增量构建三个程序。两边都编译成功且构建期间源码稳定后，更新各自 `target/debug` 的产物。文档变更只同步源码。编译失败会保留当前程序，修正后自动重试。监听地址、配对、SSH 配置和设备密钥保存在原位置。

更新器只重启二进制发生变化的开发进程。Linux 复用当前用户的 `p2p-tunnel-gateway.service` 和 `p2p-tunnel-gui.service`。Windows 通过当前登录用户的桌面会话重新启动正在使用此开发目录的网关或 GUI，保留原启动参数。重启网关会中断该网关的活动连接，重启 GUI 会关闭其终端标签。仅更新 CLI 时，已运行的界面和网关可继续使用。两端需要可用的 Rust 工具链，Windows 更新正在运行的桌面程序时需要用户已登录。

要让监听器随 Linux 用户会话启动，可以在 `~/p2p-tunnel` 执行以下命令。已有只做源码同步的监听器时，先停止它。

```sh
systemctl --user link "$PWD/packaging/linux/p2p-tunnel-dev-watch.service"
systemctl --user enable --now p2p-tunnel-dev-watch.service
systemctl --user status p2p-tunnel-dev-watch.service
journalctl --user-unit=p2p-tunnel-dev-watch.service -f
```

### 启动两个网关转发 TCP

以下命令名对应 `target/debug` 里的可执行文件，Windows 文件名带 `.exe`。

1. 在家里电脑执行 `tunnel identity home.key`，在笔记本执行 `tunnel identity laptop.key`。互相核对输出的公钥，分别填入[家里配置](examples/home.toml)和[笔记本配置](examples/laptop.toml)的 `peers` 条目。在家里先启动目标服务，例如监听 `127.0.0.1:3000` 的开发服务器。
2. 在家里运行 `tunnel-gateway --config home.toml --key home.key`，笔记本运行 `tunnel-gateway --config laptop.toml --key laptop.key`。使用默认中继时，网关通过 iroh 的公共地址发现按公钥寻址。两端也可以显式设置同一个 `native.relay_url`，使用自己的中继。
3. 在笔记本运行 `tunnel status` 查看实际监听地址，再访问 `http://127.0.0.1:8080`。每条连接通过授权服务名 `dev-web` 到达家里电脑的目标。打开 `tunnel-gui`，在 **Home → Port forwarding → Add rule → Named service** 填写规则名、监听地址（如 `127.0.0.1:8080`），选择已配对设备，填写它发布的服务名。保存后立即开始监听。主页也支持修改、暂停、恢复和删除规则。

`tunnel forward pause home-web` 会释放监听端口，现有连接继续传输。`tunnel forward resume home-web` 会重新绑定端口。暂停只影响本次运行。重启后重新加载 TOML。查看机器可读状态可用 `tunnel status --json`。

桌面端启动时只打开 **Home**。默认布局把标签放在窗口最上沿，右侧是菜单和自绘的最小化、最大化／还原、关闭按钮。拖动这一行的空白处可以移动窗口，双击可在最大化和还原之间切换。点击 **Menu** 会在按钮下方展开带图标的菜单。可用 ↑、↓ 选择，Enter 执行，Esc 或点击菜单外收起。菜单首项 **Settings** 始终可用。设置页已打开时，选择它会切到已有标签，并激活该标签所在的窗口。设置页已关闭时，选择它会在当前窗口打开设置。点击设置标签的 **×** 可以关闭它。菜单中的 **Tabs on left** 和 **Tabs on top** 切换标签栏位置。

按下标签时会立即显示对应页面，并将键盘焦点移到该标签。有多个标签时，可以按住标签沿顶栏横向排序，或沿侧栏纵向排序。顶栏末尾到设备状态之间、侧栏最后一个标签到窗口底部之间的空白区都可以拖入。按住时标签跟随鼠标，在空白处松手会排到末尾。鼠标垂直离开顶栏或横向离开侧栏超过 **24 个逻辑像素**，原窗口会切到剩余页面，拖出的页面沿用原窗口尺寸。鼠标按住期间，标签保留在新窗口标签栏中原来的位置，松手后移到第一格。沿栏拖到窗口边界以外也有相同的缓冲距离。只有一个标签时，拖动标签会直接移动整个窗口。

在 Windows 和 Linux 的 X11 桌面会话中，把独立窗口的标签拖进另一个工作区的顶栏或侧栏，标签会在鼠标仍然按住时并入。继续沿栏移动可以调整插入位置，再次越过阈值可以拆出，松手确定位置。经过页面正文时窗口保持分离。在 Windows 上，拆窗拖动途中按 **Esc** 可恢复原来的布局。

在 Wayland 桌面会话中可以拖出标签建立独立窗口。

**Settings** 显示本机名称、设备 ID 和已发布的服务，并提供复制连接票据的入口。名称默认取自网关所在电脑。选择 **Change name** 可覆盖显示名，清空后保存即可恢复使用电脑名。显示名和配对使用的公钥相互独立，已配置的对端别名保持有效。名称覆盖和转发规则写入网关启动时使用的 TOML，网关账户需要对配置文件及所在目录有写权限。

在局域网做离线测试时，可在两端加 `--no-relay`。先启动服务提供端，用 `tunnel ticket` 复制其票据，填入发起端的 `peers.home.ticket`，再启动发起端。票据中的公钥必须匹配已配对公钥。默认公网发现只在启用中继且未指定自托管中继时使用。

CLI、GUI 和 gateway 都接受 `--ipc` 或环境变量 `TUNNEL_IPC`。Unix 使用专用目录内的 socket，Windows 使用本机命名管道。CLI 与 GUI 退出后，前台或系统服务模式运行的 gateway 继续处理连接。部署与权限见[系统服务](docs/architecture.md#系统服务部署)。

### 在本地或对端监听 TCP 端口

在 **Home → Port forwarding → Add rule** 选择监听位置。假设你正在操作笔记本，对端别名是 `home`。

| 模式 | 客户端连接的入口 | 隧道另一端连接的目标 |
| --- | --- | --- |
| **Listen locally** | 笔记本的 `127.0.0.1:8080` | 家里电脑的 `127.0.0.1:3000` |
| **Listen on peer** | 家里电脑的 `127.0.0.1:9000` | 笔记本的 `127.0.0.1:5000` |

在家里电脑的 **Settings → Port access → laptop → Edit access**，填写允许笔记本连接的目标或监听的地址。已向笔记本发布的 TCP 服务也可以直接按相同的目标地址连接。CLI 等价操作如下，`port-access set` 会替换该设备的两张端口授权列表。

```sh
# 家里电脑
tunnel port-access set laptop --connect 3000 --listen 9000

# 笔记本
tunnel forward add home-web-port --peer home --listen 8080 --target 3000
tunnel forward add laptop-web --remote --peer home --listen 9000 --target 5000
tunnel forward list
```

单独填写端口会使用 `127.0.0.1`。监听地址也可以是指定 IP、`[::1]:8080` 或显式的 `0.0.0.0:8080`。远端授权按请求的地址和端口匹配。监听端口填 `0` 会由操作系统分配，实际地址可在主页或 `tunnel status` 查看。远端监听规则在对端离线或绑定失败时显示状态并自动重试。用 `tunnel forward edit` 更新规则，或用 `pause`、`resume`、`remove` 管理它。

### 通过隧道 SSH 登录

在两台已配对的电脑上使用以下规则。假设家里电脑的 SSH 服务正在监听 `127.0.0.1:22`，家里配置中的笔记本别名为 `laptop`，笔记本配置中的家里电脑别名为 `home`。

在家里电脑的 TOML 中发布 SSH 服务。

```toml
[services.ssh]
target = "127.0.0.1:22"
allow_peers = ["laptop"]
```

在笔记本的 TOML 中添加本地监听。

```toml
[forwards.home-ssh]
listen = "127.0.0.1:2222"
peer = "home"
service = "ssh"
```

重启两端网关以加载配置，在笔记本执行以下命令，将 `alice` 换成家里电脑的 SSH 用户名。

```sh
ssh -p 2222 alice@127.0.0.1
```

首次连接时核对家里电脑的 SSH 主机密钥指纹，随后使用该账户原有的 SSH 密钥或允许的认证方式登录。路由器无需向公网开放 TCP 22。

### 按服务名登录 SSH

启动两端网关后，可以在本机终端直接登录配对设备。`home` 是本机配置中的设备别名。省略用户名时，OpenSSH 使用其客户端配置或当前用户名。

```sh
tunnel ssh-config set home --user alice --service ssh --identity ~/.ssh/id_ed25519
tunnel ssh home
tunnel ssh-config show home
tunnel ssh home -l alice --service admin-ssh -i ~/.ssh/id_ed25519
```

在 **Home → Paired devices** 选择设备旁的 **SSH settings**，可以保存登录名、服务名和本机身份文件路径。点击 **Open SSH** 会直接在设备标签页登录。未填写时使用 OpenSSH 的账户和密钥配置，服务名默认为 `ssh`。再次打开同一设备会切回已有会话。切换标签、拖出独立窗口或合并窗口时会保留连接和终端内容。关闭终端标签会断开该会话。连接结束后，点击 **Reconnect** 或在终端按回车可以重新登录。拖动鼠标选择文本，双击选词，三击选行，按住 Shift 点击可扩展选区。用 Ctrl+Shift+C 复制、Ctrl+Shift+V 粘贴（macOS 使用 Cmd+C 和 Cmd+V）。有选区时复制选区，没有选区时复制当前屏幕。Windows 和 Linux 的 Ctrl+C 在有选区时复制，其他时候向远端发送中断。

命令行选项可以临时覆盖已保存的值。`tunnel ssh-config reset home` 恢复默认设置。`tunnel ssh --prompt home` 可以在本次登录手动选择账户和服务。配置文件只记录身份文件路径，私钥和密码由 OpenSSH 管理。

两种入口都需要本机安装 OpenSSH 客户端。桌面端从自身所在目录启动 `tunnel`，部署时将两个程序放在一起。主机密钥确认、密码提示、SSH agent 和用户私钥由 OpenSSH 处理。在 CLI 中可用 `-F` 指定客户端配置。

笔记本只需配对家里设备。省略 `[forwards.home-ssh]`，把 `tunnel` 放入 PATH，然后用 OpenSSH 直接打开家里设备的 `ssh` 服务。

```sh
ssh -o "ProxyCommand=tunnel connect home ssh" alice@home
```

也可以把下面的条目放进客户端的 `~/.ssh/config`，以后运行 `ssh home-p2p`。OpenSSH 用这个别名记录和校验主机密钥。

```sshconfig
Host home-p2p
    User alice
    ProxyCommand tunnel connect home ssh
```

目标端可以继续使用上一节的 TCP 服务。在 Linux 上，还可把家里网关的服务改为以下配置，让每次已授权的连接启动一个 `/usr/sbin/sshd -i` 进程。这样两端都无需监听 SSH 的 TCP 端口。

```toml
[services.ssh]
target = { sshd_config = "/etc/p2p-tunnel/sshd_config" }
allow_peers = ["laptop"]
```

`sshd_config` 是本机管理员准备的 OpenSSH 服务端配置。网关启动时会运行 `sshd -t -f` 检查它。使用普通账户运行时，可为该账户提供可读的专用 `HostKey`、自己的 `AuthorizedKeysFile` 并设 `UsePAM no`，登录同一账户。要使用系统现有的多账户登录策略，可以直接选已有的 TCP 目标。Windows 和 macOS 网关目前使用 TCP 目标来提供 SSH 服务。Linux 上可运行 `python3 scripts/check-gateway.py --ssh-stdio` 验证一次使用临时密钥的完整登录。

### 用两台电脑测试打洞

下面用 `tunnel` 指代编译出的可执行文件。先在两台电脑各自的用户私有目录里创建身份。命令输出公钥，重复执行会读取原有身份。

1. 在家里电脑运行 `tunnel identity home.key`，在笔记本运行 `tunnel identity laptop.key`。私钥文件各自保存在本机。
2. 通过可信渠道互换并核对公钥。在家里电脑复制 `examples/home.toml` 为 `home.toml`，把 `peers.laptop.endpoint_id` 换成笔记本公钥。在笔记本复制 `examples/laptop.toml` 为 `laptop.toml`，把 `peers.home.endpoint_id` 换成家里电脑公钥。
3. 在家里电脑执行以下监听命令。监听命令持续运行直到 Ctrl-C，并打印包含公钥及当前连接地址的票据。

```sh
tunnel probe listen --key home.key --config home.toml --peer laptop
```

4. 如果两端在 TOML 中显式设置了相同的 `native.relay_url`，笔记本可以直接按设备别名连接。它通过该中继找到已配对的设备，两端自动交换候选地址并探测 IPv6 和 IPv4 路径。

```sh
tunnel probe connect --key laptop.key --config laptop.toml --peer home
```

使用默认中继、不同中继或直接地址时，把家里电脑输出中 `ticket=` 后面的完整值交给笔记本，替换下方的 `TICKET`。两种方式默认发送 20 次回显，也可以用 `--count` 指定 1 到 300 次。

```sh
tunnel probe connect --key laptop.key --config laptop.toml --peer home --ticket TICKET
```

每行结果包含回显耗时、采样时选中的路径及路径收发字节数。`path=direct` 表示直连，`path=relay` 表示中继。要观察 NAT 打洞，请使用不同外网。要单独检查局域网直连，可在监听和连接命令上都加 `--no-relay`。[路径与测试条件](docs/connectivity.md)

诊断协议用于编号回显。网关的 TCP 转发使用独立的 `tcp-forward/1` 协议。备用中继接管和 WebRTC 适配仍在设计阶段。

## 设计文档

| 文档 | 内容 |
| --- | --- |
| [架构与客户端](docs/architecture.md) | 三平台常驻服务、CLI 与 GPUI 的本机管理接口、iroh 与 WebRTC 如何共用转发核心、子项目与实施顺序 |
| [原生协议与配置](docs/native-protocol.md) | TOML 字段、设备身份、线上消息格式、半关闭与错误处理 |
| [NAT 穿透与性能验收](docs/connectivity.md) | 中继部署、打洞过程、弱网恢复和跨网验收方法 |

代码入口为[主网关](apps/tunnel-gateway/src/main.rs)、[CLI](apps/tunnel-cli/src/main.rs)、[GPUI 桌面端](apps/tunnel-gui/src/main.rs)和[共享管理接口](crates/tunnel-control/src/lib.rs)。
