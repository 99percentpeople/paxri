# 安装网关和 CLI

## Windows x64 普通用户

将 `p2p-tunnel-windows-x64.zip` 完整解压到自己的用户目录。免安装包包含原生网关、CLI 和桌面界面，程序静态链接 Visual C++ 运行库。

1. 双击 `tunnel-gui.exe`。首次启动会以当前用户身份在后台启动同目录的 `tunnel-gateway.exe`，在 `%LOCALAPPDATA%\P2PTunnel` 创建 `node.toml` 和 `device.key`。在 **Home** 页面配对。
2. 打开 **Menu → Settings → Gateway** 可停止、重启网关或选择启动方式。停止会断开活动终端和转发。关闭 GUI 后，网关仍会运行。主页离线时可以点击 **Start gateway**。
3. 在解压目录的 PowerShell 也可以运行以下命令。`tunnel-gui.exe` 支持相同子命令。

```powershell
.\tunnel.exe gateway start
.\tunnel.exe gateway status
.\tunnel.exe gateway restart
.\tunnel.exe gateway stop
.\tunnel.exe gateway startup
.\tunnel.exe gateway startup login
```

| 启动方式 | CLI 参数 | 生效时机 |
| --- | --- | --- |
| Manually | `manual` | 手动点击启动或执行 `gateway start` |
| When the app opens | `gui` | 默认。打开 GUI，已有网关时直接连接 |
| At sign-in | `login` | 当前用户登录时启动，也可从 GUI 启动 |
| At system startup | `system` | 系统启动时以当前用户的受限身份运行，需要有权注册启动任务 |

设置保存在同一份 `node.toml` 的 `[gateway]` 表，键为 `startup`。请通过设置页或 `gateway startup <模式>` 更改，以便同步注册或移除操作系统的启动项。Windows 使用任务计划程序。开机模式由同一账户以管理员权限设置或移除，设置时会将启动程序复制到受保护的 Program Files 目录。通过开机任务或已提升权限的 GUI、CLI 启动网关时，网关都使用普通用户权限，沿用原来的设备身份。移动解压目录后，请重新设置启动方式。开机模式使用 S4U 登录，依赖 Windows 登录凭据的网络共享和加密文件受限。权限不足时会显示系统错误，并保留原来的选择。普通用户可选择打开 GUI 时启动或登录时启动。[Windows 任务登录方式](https://learn.microsoft.com/en-us/windows/win32/api/taskschd/ne-taskschd-task_logon_type)

Windows 11 24H2 及以上版本双击 `tunnel-gui.exe` 会直接打开主页。较早的 Windows 版本双击时可能闪过命令窗口，可在已打开的 PowerShell 中运行 `.\tunnel-gui.exe` 来避免闪现。同一个 EXE 在命令行运行 `exec` 时支持输出重定向，并返回远端程序的退出码。[Windows 控制台分配策略](https://learn.microsoft.com/en-us/windows/console/console-allocation-policy)

同一台电脑有多个节点时，使用独立的配置目录和 IPC 名称。例如 `.\tunnel-gui.exe --ipc my-node --config "D:\My Node\node.toml"`。CLI 对应 `.\tunnel.exe --ipc my-node gateway start --config "D:\My Node\node.toml"`。设备密钥和后台启动日志 `gateway.log` 位于该 TOML 的同一目录。旧版前台网关仍在运行时，先关闭其命令窗口一次，再从新 GUI 启动。

远端 PowerShell 使用登录用户本身的权限。当前包中的 EXE 未签名，受管电脑需要允许执行这些程序。Windows 防火墙或单位网络可能限制入站连接，直连受限时，默认配置可以使用能从该网络访问的 iroh 中继。[Windows 防火墙规则](https://learn.microsoft.com/en-us/windows/security/operating-system-security/network-security/windows-firewall/rules)

要从源码制作同一安装包，在有 Rust MSVC 工具链和 Python 的 Windows 构建机上运行 `python packaging/windows/build-portable.py`。输出在 `target/dist`。

### Linux 和 macOS 桌面

相同的 `gateway` 命令和 GUI 设置也可管理普通用户网关。Linux 的 `login` 使用桌面会话的 XDG 自启动项，`system` 使用该用户的 systemd 单元和 lingering，账户需要启用 lingering 或有权执行 `loginctl enable-linger`。关闭网关的开机启动会移除它的单元。该账户的其他服务可以继续使用 lingering。[systemd 登录管理](https://www.freedesktop.org/software/systemd/man/latest/loginctl.html)

macOS 对应用户 LaunchAgent 和系统 LaunchDaemon，系统模式使用配置文件所有者的普通用户身份，安装需要写入 `/Library/LaunchDaemons` 的权限。系统托管的现有网关保持其原有启动方式。

## Docker Compose

已有镜像时，将对应架构的压缩包与 `compose.yaml` 放进同一目录。`uname -m` 显示 `x86_64` 时选择 `linux-amd64`，显示 `aarch64` 时选择 `linux-arm64`。例如，64 位树莓派可直接加载 ARM64 包并启动网关。

```sh
docker image load -i p2p-tunnel-docker-linux-arm64.tar.gz
docker compose up -d --no-build --wait
docker compose exec gateway tunnel pair code
```

从源码安装时，在装有 Docker Engine 和 Docker Compose 的 Linux 主机上，从项目根目录构建并启动网关。

```sh
docker compose up -d --build --wait
docker compose exec gateway tunnel status
docker compose exec gateway tunnel pair code
```

首次构建会编译当前源码。镜像包含 Release 版 `tunnel-gateway` 和 `tunnel`。Docker 通过 `restart: unless-stopped` 管理容器，宿主机只需运行 Docker。查看日志用 `docker compose logs -f gateway`，停止用 `docker compose stop`，重新启动用 `docker compose up -d --wait`。[Docker 重启策略](https://docs.docker.com/engine/containers/start-containers-automatically/)

把配对码交给另一台设备，在对方的 **Home** 页面输入，或用对方的 `tunnel pair join` 命令提交。然后在发码的容器上查看请求，用输出中的设备 ID 替换 `DEVICE_ID` 并确认。

```sh
docker compose exec gateway tunnel pair status
docker compose exec gateway tunnel pair accept DEVICE_ID
```

Compose 在原生 Linux 上使用宿主机网络和主机名。网关可通过宿主机网卡建立 UDP 直连，转发规则中的 `127.0.0.1` 指向宿主机，可以直接访问本机服务。远端打开这个节点的终端或执行命令时，使用的是容器内的 Bash 和普通账户 `p2p-tunnel`。需要操作宿主机文件、程序或原生 shell 时，选择下方的原生服务安装。Docker Desktop 的 host 网络需在设置中显式启用，容器中的终端仍是 Linux shell。[Docker host 网络](https://docs.docker.com/engine/network/drivers/host/)

### 持久数据与升级

Compose 的 `node-data` 卷挂载在 `/var/lib/p2p-tunnel`。首次启动会生成 `node.toml` 和私有的 `device.key`。配对、转发和设备身份随卷保存。更新源码后再次执行 `docker compose up -d --build --wait` 即可使用新镜像。`docker compose down` 后使用原项目名再次启动会接回同一个卷。加 `-v` 会删除卷和设备身份。

如果已有节点，先停止原网关。将 `node.toml` 与 `device.key` 一起放入同一目录，在 Compose 中将 `node-data:/var/lib/p2p-tunnel` 改为该目录的绑定挂载，例如 `./node-data:/var/lib/p2p-tunnel`。使用普通 Linux Docker Engine 时，目录和文件须允许容器的 UID/GID `10001:10001` 读写，私钥使用 `0600`。网关会原子替换 TOML，因此应挂载整个目录。

### 导出镜像和独立运行

可将镜像和 `compose.yaml` 复制到同一 CPU 架构的 Linux 设备。先在构建机上导出镜像。

```sh
docker compose build gateway
docker image save p2p-tunnel:local | gzip > p2p-tunnel-image.tar.gz
```

在目标机用 `docker image load -i p2p-tunnel-image.tar.gz` 加载。只使用 Docker CLI 时，可以用已加载的镜像创建一个独立节点。

```sh
docker run -d --name p2p-tunnel --restart unless-stopped --init \
  --network host --uts host --security-opt no-new-privileges \
  -v p2p-tunnel-data:/var/lib/p2p-tunnel p2p-tunnel:local
docker exec p2p-tunnel tunnel pair code
```

### 从宿主机的 CLI 或 GUI 连接

默认可用 `docker compose exec gateway tunnel` 管理容器。原生 Linux Docker Engine 上，也可以把管理 socket 挂载给宿主机上的 CLI 和 GUI。下面的目录允许当前用户的主组连接，组内成员都能管理该节点。

```sh
mkdir -p "$HOME/.local/share/p2p-tunnel"
sudo install -d -o 10001 -g "$(id -g)" -m 2750 "$HOME/.local/share/p2p-tunnel/docker-ipc"
```

在 `compose.yaml` 旁创建 `compose.override.yaml`，添加这项挂载。

```yaml
services:
  gateway:
    volumes:
      - ${HOME}/.local/share/p2p-tunnel/docker-ipc:/run/p2p-tunnel
```

```sh
docker compose up -d --wait
export TUNNEL_IPC="$HOME/.local/share/p2p-tunnel/docker-ipc/gateway.sock"
tunnel status
tunnel-gui
```

## 原生 Linux systemd 服务

在有 systemd 的 Linux 主机上，从项目根目录构建两个程序，再运行安装脚本。

```sh
cargo build --locked --release -p tunnel-gateway -p tunnel-cli
sudo ./packaging/linux/install-systemd.sh
sudo -u p2p-tunnel /usr/local/bin/tunnel pair code
```

使用预编译安装包时，解压后进入包含 `bin/` 的目录，执行 `sudo ./install-systemd.sh ./bin`。同一个脚本也接受自备的 Release 程序目录。脚本安装到 `/usr/local/bin`，启用并启动 `p2p-tunnel-gateway.service`。升级时再次运行脚本会重启服务并沿用 `/etc/p2p-tunnel/node.toml` 和 `/etc/p2p-tunnel/device.key`。服务账户的主目录是 `/var/lib/p2p-tunnel`，远端命令以该普通账户的权限在宿主机执行。

通过 `journalctl -u p2p-tunnel-gateway -f` 查看日志。要停止服务并关闭开机启动，运行 `sudo systemctl disable --now p2p-tunnel-gateway`。需要用自己的 CLI 或桌面界面管理该网关时，可加入 `p2p-tunnel` 组，重新登录后通过默认的 `/run/p2p-tunnel/gateway.sock` 连接。

同一份原生包也可以用自己的账户前台运行。进入解压目录后执行以下命令。另开终端时，给 `bin/tunnel` 传入相同的 `--ipc` 路径即可管理该节点。

```sh
state_dir="$HOME/.local/share/p2p-tunnel/node"
install -d -m 0700 "$state_dir"
PATH="$PWD/bin:$PATH" ./start-gateway "$state_dir" --ipc "$state_dir/run/gateway.sock"
```
