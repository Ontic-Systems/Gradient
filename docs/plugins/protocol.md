# Gradient Plugin Protocol — v1

> Status: stable, version 1. Bumping the version number requires an ADR.
>
> Issue: [#377](https://github.com/Ontic-Systems/Gradient/issues/377)
> (Epic [#304](https://github.com/Ontic-Systems/Gradient/issues/304) — Tooling suite).

The Gradient CLI (`gradient`) supports out-of-tree plugins so the
community can ship `gradient-fuzz`, `gradient-miri`, `gradient-profile`,
`gradient-debugger`, etc. as separate binaries discovered on `PATH`.

The model is intentionally cargo-style: any binary on `PATH` named
`gradient-<name>` is invocable as `gradient <name> [args]`. The host
exec's the plugin, forwards remaining arguments, sets a small set of
well-known environment variables, and propagates the plugin's exit
code.

This protocol is pinned at version `1`. Plugins MUST inspect
`GRADIENT_PLUGIN_PROTOCOL_VERSION` and refuse to run if they do not
support the host's protocol version.

---

## Discovery

When the host CLI is invoked as:

```
gradient <name> [args...]
```

it consults a static list of built-in subcommands. If `<name>` matches
a built-in (see [Reserved names](#reserved-names) below), the built-in
handler runs and the plugin lookup is skipped entirely. Otherwise the
host:

1. Validates `<name>` is a syntactically valid plugin name (ASCII
   alphanumeric, `-`, `_`; first character must be alphanumeric).
2. Walks `PATH` (split by the platform separator — `:` on Unix, `;`
   on Windows) and probes each directory for an executable file named
   `gradient-<name>` (Unix) or `gradient-<name>.exe` then
   `gradient-<name>` (Windows).
3. Returns the first match. PATH order wins; ambiguity is resolved by
   the user's PATH configuration.

If no match is found the host falls through to its own argument parser,
which produces an "unrecognized subcommand" error. Plugin authors who
want a friendlier surface can ship a `gradient-help` plugin that lists
known plugins (the host does not enumerate plugins itself).

### Reserved names

The following names are always built-in and MUST NOT be used as plugin
names. A plugin shipping under one of these names will be silently
shadowed by the built-in.

| Name | Built-in subcommand |
|---|---|
| `add` | dependency add |
| `bench` | run `@bench` perf harness |
| `build` | compile the current project |
| `check` | type-check without code generation |
| `doc` | generate API documentation |
| `fetch` | download registry dependencies |
| `fmt` | format Gradient source |
| `init` | initialize a project in the cwd |
| `new` | create a new project |
| `repl` | start the REPL |
| `run` | compile + execute |
| `test` | run tests |
| `update` | re-resolve `gradient.lock` |

This list is the canonical source of truth. The runtime equivalent
lives in `codebase/build-system/src/commands/plugin.rs`
(`BUILTIN_SUBCOMMANDS`) and is locked alphabetically by a unit test —
keep this table in sync when a new built-in lands.

---

## Invocation

The host invokes the plugin via `std::process::Command::status` with:

- **argv[0]**: the absolute path to the resolved `gradient-<name>` binary.
- **argv[1..]**: the host's argv from index 2 onward, verbatim. The
  host does NOT pass `<name>` itself or the host's own argv[0]; the
  plugin sees only the args the user wrote after the subcommand.
- **stdin / stdout / stderr**: inherited from the host. Plugins may
  read stdin and write to either output stream.
- **environment**: the host's environment, augmented with the variables
  in [Environment](#environment) below.
- **working directory**: inherited from the host. Plugins MUST use
  `GRADIENT_PROJECT_ROOT` (when set) instead of assuming the cwd is a
  project root — the user may invoke the plugin from a subdirectory.

The host waits for the plugin to exit and propagates the plugin's exit
code as its own. If the plugin cannot be exec'd (e.g. EACCESS, ENOENT
mid-flight), the host prints an error to stderr and exits 1.

---

## Environment

The host sets the following variables before exec'ing the plugin.
Existing values inherited from the parent are overwritten.

| Variable | Value | Always set? |
|---|---|---|
| `GRADIENT_PLUGIN_PROTOCOL_VERSION` | The protocol version string (currently `1`). | yes |
| `GRADIENT_VERSION` | The host CLI version (e.g. `0.1.0`). Same as `gradient --version`. | yes |
| `GRADIENT_BIN` | Absolute path to the `gradient` binary that exec'd the plugin. Best-effort — may be missing on platforms where `current_exe` fails. | best-effort |
| `GRADIENT_PROJECT_ROOT` | Absolute path to the nearest enclosing directory containing `gradient.toml`. | only when in a project |

Plugins SHOULD treat absence of `GRADIENT_PROJECT_ROOT` as "not in a
Gradient project" rather than searching upward themselves; the host
already searched and found nothing.

Plugins MAY inspect the host's full environment (e.g. `RUST_LOG`,
`HOME`) but MUST NOT rely on host-internal env vars not listed here.
Future protocol versions may add variables; plugins should ignore
unrecognized ones.

### Protocol version handshake

A plugin SHOULD start by checking `GRADIENT_PLUGIN_PROTOCOL_VERSION`:

```sh
case "${GRADIENT_PLUGIN_PROTOCOL_VERSION:-0}" in
  1) ;;
  *) echo "gradient-myplugin: unsupported protocol version $GRADIENT_PLUGIN_PROTOCOL_VERSION" >&2; exit 64 ;;
esac
```

The host pins `GRADIENT_PLUGIN_PROTOCOL_VERSION = 1` for this protocol
revision. Any future bump is a breaking change and triggers a new
ADR.

---

## Conventions

### `--help` / `--version`

Plugins SHOULD support `--help` and `--version`. The host does NOT
intercept these flags when dispatching to a plugin — they pass through
as ordinary arguments.

### Exit codes

Plugins SHOULD follow standard Unix conventions:

| Code | Meaning |
|---|---|
| `0` | success |
| `1` | generic error (compilation / verification failed, etc.) |
| `2` | argument / usage error |
| `64` | protocol version mismatch |
| `127` | plugin reported a missing subcommand of its own |

The host propagates the plugin's exit code unchanged.

### Output formats

Plugins SHOULD support a `--json` flag for machine-readable output
when a structured form makes sense. The host's built-in subcommands
(`gradient bench --json`, `gradient doc --json`) follow this
convention.

### Logging

Plugins SHOULD log diagnostics to stderr and reserve stdout for the
plugin's primary output (e.g. the JSON document, the formatted report,
the generated artifact). This keeps `gradient myplugin --json | jq` working.

---

## Security

Plugins are arbitrary binaries on the user's `PATH`. The host does NOT
sandbox them, vet them, or check signatures. Users are responsible for
the contents of their PATH.

Future work (Epic [#303](https://github.com/Ontic-Systems/Gradient/issues/303)
— Package registry) will introduce sigstore-style verified plugin
distribution. Until then, install plugins only from sources you trust.

A plugin MUST NOT impersonate a built-in subcommand by, for example,
producing identical-looking error messages. The reserved-name shadowing
in the host already prevents accidental shadowing, but a plugin named
`gradient-buld` (typo) could mislead users. Plugin authors should
choose unambiguous names.

---

## Reference plugin

The repository ships a reference plugin under
`docs/plugins/gradient-hello-plugin/` (a small shell script). It
demonstrates:

- protocol-version handshake
- `--help` / `--version` parsing
- reading `GRADIENT_PROJECT_ROOT`
- argument forwarding

Adding it to PATH lets you run:

```
gradient hello
gradient hello --json
gradient hello --version
```

See the reference plugin's [README.md](./gradient-hello-plugin/README.md)
for installation steps.

---

## FAQ

**Q: Can a plugin be a Gradient program (a `.gr` binary)?**
A: Yes — once the package registry lands. For now, plugins are any
executable on PATH. The protocol is language-agnostic.

**Q: Does the host pass `--` correctly?**
A: Yes. Anything after the subcommand name is forwarded as-is, including
`--`, the GNU "end of options" marker. Plugins can safely accept their
own flags.

**Q: How do I list available plugins?**
A: The host doesn't enumerate plugins itself. Use shell globbing:
`compgen -c gradient- | sort -u` (bash) or `which -a gradient-* | sort -u`.
A future built-in `gradient plugins list` may be added — track issue
[#377](https://github.com/Ontic-Systems/Gradient/issues/377)
follow-ups.

**Q: Can plugins call back into `gradient`?**
A: Yes — `GRADIENT_BIN` is set so plugins can exec the host CLI for
sub-tasks. Avoid recursion loops (plugin invokes the host invokes the
same plugin); the host does not detect them.

---

## Implementation references

- Host dispatch logic: `codebase/build-system/src/commands/plugin.rs`
- Built-in subcommand list: `BUILTIN_SUBCOMMANDS` in the same file
- Pre-clap dispatch hook: top of `fn main()` in
  `codebase/build-system/src/main.rs`
- Unit tests: `codebase/build-system/src/commands/plugin.rs` (`mod tests`)
- Integration tests (Unix only): `codebase/build-system/tests/plugin_dispatch.rs`
- Reference plugin: `docs/plugins/gradient-hello-plugin/`
