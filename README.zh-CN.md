# docker2ssh

[English](./README.md)

`docker2ssh` 是一个运行在 Linux 上的工具，用来通过 SSH 兼容端口暴露 Docker 容器。

它适用于 Cursor、Code-OSS、Trae 以及其他类似 VS Code、但由于合规或平台限制无法使用 `Remote Docker` 插件的场景。

典型用途：

- 用普通 SSH 工具连接到某个指定容器
- 在本地通过 `DOCKER_HOST=ssh://...` 使用 Docker CLI 工作流
- 支持需要 SSH 或 Docker-over-SSH 访问远程容器环境的编辑器工作流

## 安装

### 通过 PyPI 安装

```bash
python3 -m pip install -U pip
python3 -m pip install -U docker2ssh
```

说明：

- 当前发布的包支持 Linux `x86_64` 和 `aarch64`
- 在较老的系统上，建议先升级 `pip`，避免无法识别已发布的 wheel tag
- 安装或升级 `docker2ssh` 时，会一并安装或更新其内置的 `d2s` 运行时

### 从源码构建

#### 普通构建

```bash
cargo build --release
```

二进制产物：

```bash
./target/release/d2s help
```

#### 静态 Linux 构建

如果你需要一个不依赖外部 `.so` 的 Linux 二进制，可以使用 `musl` 构建：

```bash
./scripts/build-static-linux.sh
```

默认产物：

```bash
./dist/d2s-x86_64-unknown-linux-musl
```

说明：

- 默认目标：`x86_64-unknown-linux-musl`
- 可以用 `TARGET=aarch64-unknown-linux-musl` 覆盖目标架构
- 可以用 `USE_DOCKER=1` 在 `clux/muslrust:stable` 容器里构建
- 脚本会在可用时用 `ldd` 验证产物

## 功能

- 为每个映射端口暴露一个 SSH 监听器
- 接受普通 SSH shell / exec 请求，并映射到 `docker exec`
- 接受 Docker CLI 的 `docker system dial-stdio` 请求
- 将 Docker HTTP API 流量代理到宿主机 Docker Socket
- 将 `/containers/<id 或 name>/...` 请求限制到当前映射容器
- 支持容器名、完整 ID、短 ID
- 配置中允许写入已经停止的容器
- `show` 只显示当前正在运行的容器
- `doctor` 用来验证当前活跃映射是否可用

## 命令

```bash
docker2ssh
docker2ssh help
docker2ssh config set <port> <container>
docker2ssh config rm <port>
docker2ssh config list
docker2ssh show
docker2ssh doctor --host 127.0.0.1 --user docker [--identity /path/to/id_ed25519]
docker2ssh serve
docker2ssh stop
```

说明：

- `docker2ssh` 不带子命令时会打印帮助
- `config set` 会校验容器是否存在，即使容器已经停止
- `show` 只输出当前容器正在运行的映射
- `serve` 会启动后台 daemon，并在配置文件旁边写入 pid 文件
- `stop` 会停止同一配置路径下启动的 daemon

## 配置文件

默认路径：

- Linux：`~/.config/d2s/config.toml`

示例：

```toml
listen_host = "0.0.0.0"
docker_socket = "/var/run/docker.sock"
authorized_keys = "/etc/d2s/authorized_keys"
host_key = "/etc/d2s/host_key_ed25519"

[[mappings]]
port = 2222
container = "my-app"
shell = "/bin/bash"

[[mappings]]
port = 2223
container = "3d2c1b0a9f87"
```

字段说明：

- `listen_host`：SSH 监听地址
- `docker_socket`：Docker daemon 的 Unix Socket 路径
- `authorized_keys`：可选的 OpenSSH `authorized_keys` 文件；如果不配置，测试连接会放宽认证，生产环境不要这么用
- `host_key`：可选的 SSH host private key 路径；如果没有，内置运行时会自动生成
- `mappings[].shell`：容器内 shell 覆盖项；如果不配置，会依次尝试 `/bin/bash`、`/bin/sh`、`sh`

## 典型用法

把端口映射到容器：

```bash
docker2ssh config set 2222 my-app
docker2ssh config set 2223 my-app --shell /bin/bash
```

启动 daemon：

```bash
docker2ssh serve
docker2ssh show
```

从另一台机器访问：

```bash
ssh root@your-server -p 2222
ssh root@your-server -p 2222 "ls /"
DOCKER_HOST=ssh://docker@your-server:2222 docker ps
DOCKER_HOST=ssh://docker@your-server:2222 docker cp ./file.txt my-app:/tmp/file.txt
```

稍后停止：

```bash
docker2ssh stop
```

## Daemon 用法

`docker2ssh serve` 会自动后台化，普通场景下不需要再额外接一个服务管理器。

典型生命周期：

```bash
docker2ssh --config /etc/d2s/config.toml serve
docker2ssh --config /etc/d2s/config.toml stop
```

说明：

- 除非你的运行用户已经在 Docker group 中，否则需要访问 `/var/run/docker.sock`
- 绑定 `2222` 这种高位端口通常不需要 root
- daemon 的 pid 文件会写在配置文件同目录，文件名是 `d2s.pid`

## Python 包

这个仓库也包含了用来发布到 PyPI 的 Python 打包目录，用于分发预编译的 Linux 版本。

包目录：

- `python/`

它提供：

- `docker2ssh` 命令行脚本
- 一个会调用内置 `d2s` 二进制的小型 Python API
- `D2S` 方法：`serve`、`show`、`doctor`、`config set/rm/list`
- 每个已发布 wheel 中都内置一个 `d2s` 二进制

在先构建静态二进制之后，可以这样构建预编译 wheel：

```bash
TARGET=x86_64-unknown-linux-musl ./scripts/build-static-linux.sh
TARGET=x86_64-unknown-linux-musl ./scripts/build-python-wheel.sh
```

说明：

- wheel metadata 会生成为 `py3-none-<platform>`
- CI 会在 `.github/workflows/python-wheels.yml` 中构建 Linux `x86_64` 和 `aarch64`
- tag `vX.Y.Z` 默认发布包版本 `X.Y.Z`；如果该版本在 PyPI 已存在，会自动改成 `X.Y.Z.postN`
- 安装已发布 wheel 时不需要 source distribution
- 如果你想替换运行时二进制，仍然可以用 `D2S_BIN` 覆盖

如果你想把 wrapper 指向一个自定义二进制路径：

```bash
export D2S_BIN=/usr/local/bin/d2s
```

示例：

```python
from docker2ssh import D2S

client = D2S(config="/etc/d2s/config.toml")
print(client.show())
client.config_set(2222, "my-container")
```

## 安全模型

- 当配置了 `authorized_keys` 时，认证方式是 SSH 公钥认证
- 普通 SSH 访问会被翻译成 `docker exec`
- 仍然接受 Docker CLI 的 `docker system dial-stdio` 以支持远程 Docker API 访问
- 针对 `/containers/<ref>/...` 的请求会被重写并限制到映射容器
- `/_ping`、`/version`、`/info`、`/events` 这类全局 Docker endpoint 仍然允许访问

它是一个偏实用的“单容器范围 Docker 代理”，不是完整的 Docker RBAC 实现。

## 测试

运行：

```bash
cargo test
```

当前覆盖包括：

- 配置读写、upsert/remove 行为
- Docker 路径授权与重写
- 基于 Unix Socket 的 HTTP 代理
- 基于 mock Docker backend 的 SSH 端到端 doctor 流程
- 映射到容器 exec 的 SSH shell 模式
- 映射到容器 exec 的 SSH exec 模式

## 仓库结构

- Rust 服务端：`src/`
- 静态构建脚本：`scripts/build-static-linux.sh`
- Python 包：`python/`
