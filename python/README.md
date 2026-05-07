# docker2ssh Python Package

`docker2ssh` is a thin Python wrapper around a bundled precompiled `d2s` binary.

It does not reimplement the SSH server in Python. Instead, it:

- discovers the bundled `d2s` executable
- exposes a small Python API
- provides a `docker2ssh` console script

## Install

```bash
pip install docker2ssh
```

Published Linux wheels bundle `d2s` directly. Set `D2S_BIN` only if you want to override
the bundled executable:

```bash
export D2S_BIN=/usr/local/bin/d2s
```

## Python API

```python
from docker2ssh import D2S

client = D2S(config="/etc/d2s/config.toml")
print(client.show())
client.config_set(2222, "my-container")
```

## CLI Wrapper

```bash
docker2ssh show
docker2ssh config set 2222 my-container
docker2ssh serve
```
