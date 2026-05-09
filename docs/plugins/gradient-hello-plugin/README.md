# gradient-hello-plugin

Reference plugin for the Gradient plugin protocol (v1).

See [`../protocol.md`](../protocol.md) for the full contract.

## What it does

A minimal plugin that:

- Performs a protocol-version handshake (refuses to run against an
  unknown host protocol).
- Supports `--help`, `--version`, and `--json`.
- Reads the four host-provided environment variables
  (`GRADIENT_PLUGIN_PROTOCOL_VERSION`, `GRADIENT_VERSION`,
  `GRADIENT_BIN`, `GRADIENT_PROJECT_ROOT`) and reports them.
- Returns exit code 64 on protocol mismatch, 2 on bad arguments, 0
  otherwise — matching the conventions in the protocol doc.

It's intentionally a portable POSIX shell script so it can be read in a
single sitting. Real-world plugins should be Rust / Go / Gradient
binaries.

## Install

```sh
# From the repository root:
chmod +x docs/plugins/gradient-hello-plugin/gradient-hello

# Make it discoverable on PATH (one of the following):
export PATH="$PWD/docs/plugins/gradient-hello-plugin:$PATH"
# or copy into ~/.local/bin (assuming that is on PATH):
cp docs/plugins/gradient-hello-plugin/gradient-hello ~/.local/bin/
```

## Run

```sh
gradient hello
# Hello from gradient-hello-plugin (v0.1.0).
#   protocol: v1
#   host:     0.1.0
#   binary:   /path/to/gradient
#   project:  (not in a Gradient project — no gradient.toml found)

gradient hello --json
# {
#   "plugin": "gradient-hello-plugin",
#   "plugin_version": "0.1.0",
#   "host_protocol_version": 1,
#   ...
# }

gradient hello --help
gradient hello --version
```

## Authoring your own plugin

1. Pick a name; verify it does not collide with a built-in (see
   [protocol.md § Reserved names](../protocol.md#reserved-names)).
2. Create a binary named `gradient-<name>`.
3. At startup, check `GRADIENT_PLUGIN_PROTOCOL_VERSION == "1"` and
   exit 64 on mismatch.
4. Read `GRADIENT_PROJECT_ROOT` instead of searching for
   `gradient.toml` yourself.
5. Follow the exit-code conventions.
6. Drop the binary anywhere on `PATH` and invoke as `gradient <name>`.
