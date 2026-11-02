# herdr-usage

A [Herdr](https://herdr.dev) popup that shows, at a glance:

- your **Claude** and **Codex** plan limits (the 5-hour and weekly windows, and when they reset);
- **CPU, RAM, load and agents** for this machine and every saved Herdr machine.

```
 Plan limits

  Claude   5h     █████████░░░░░░░░░░░░░░░  38%   resets in 1h 27m · 13:09
           week   █████████░░░░░░░░░░░░░░░  38%   resets in 4d 6h · Fri 17:59
  Codex    5h     ████████████████░░░░░░░░  65%   resets in 3h 42m · 15:25
           week   █████████████░░░░░░░░░░░  54%   resets in 3d 7h · Thu 19:24

 Machines        CPU                RAM                         load     agents

  ● thor        ░░░░░░░░░░   2%   ███░░░░░░░  34.5/125  GB    6.0/24   1 working · 5 idle
  ● mimir       ███░░░░░░░  32%   ████░░░░░░  54.2/123  GB   13.6/32   no agents
  ○ heimdall    offline  ssh: connect to host heimdall port 22: Connection timed out
```

Plan limits refresh every two minutes and machines every five seconds while
the popup is open. Press `r` to refresh now and `q` or `Esc` to close.

## Install

As a Herdr plugin (builds with `cargo`):

```bash
herdr plugin install MagnusBVillumsen/herdr-usage
```

Then bind a key in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+u"
type = "plugin_action"
command = "herdr-usage.open"
description = "usage and machines"
```

Or install the binary and open it as a plain popup:

```bash
cargo install --git https://github.com/MagnusBVillumsen/herdr-usage
# or
nix profile install github:MagnusBVillumsen/herdr-usage
```

```toml
[[keys.command]]
key = "prefix+u"
type = "popup"
command = "herdr-usage"
description = "usage and machines"
width = 100
height = 24
```

`herdr-usage --once` prints the same overview once, for scripts and status bars.

## Where the numbers come from

| | |
| --- | --- |
| Claude | `api.anthropic.com/api/oauth/usage` with Claude Code's login (`~/.claude/.credentials.json`, or the keychain on macOS) |
| Codex | `chatgpt.com/backend-api/wham/usage` with Codex's ChatGPT login (`~/.codex/auth.json`) |
| Machines | `/proc` on Linux, `top`, `vm_stat` and `sysctl` on macOS, read over Herdr's saved SSH targets |
| Agents | `herdr agent list`, with `--machine` for saved machines |

These are the endpoints Claude Code and Codex use for their own usage
displays. They are not public APIs and may change. The logins are only read,
never refreshed: if one has expired, start that agent once. `CLAUDE_CONFIG_DIR`
and `CODEX_HOME` are honoured.

A subscription is shared by all your machines, so the limits are read on the
machine running the popup. RAM counts what is not available to new programs
(`MemTotal - MemAvailable` on Linux; active, wired and compressed pages on
macOS).

## License

MIT ([LICENSE-MIT](LICENSE-MIT)) or Apache-2.0 ([LICENSE-APACHE](LICENSE-APACHE)), at your option.
