# 安装网关和 CLI

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
