# Networking and security

```sh
oche-core --bind 0.0.0.0 --port 5180 run
oche-core --host board-host --port 5180 remote
oche-core --host board-host --port 5180 status --json
```

The default port is **5180**, keeping OcheCore separate from Autodarts on 3180.
Default binding is loopback. `--bind` selects the listen address; `--host` selects
the daemon to manage. Listen overrides are per invocation. Saved `server.bind`
and `server.port` changes require restart; until then use the old port explicitly.

Existing configuration files keep their saved port. To switch an existing board,
stop its daemon, set `port = 5180` in the `[server]` section of `board.toml`, then
restart it. For a temporary override, use `oche-core --port 5180 run` and pass
`--port 5180` to management commands as well. Existing Docker volumes also keep
their saved configuration; their `server.port` must match the container's 5180
port mapping.

The native API is plain HTTP without authentication: anyone who can reach it can
control the board. Use loopback, a trusted/private LAN, or an SSH tunnel. No hosted
service is needed. The [native API](native-api.md) is separate from future
Autodarts adapters.
