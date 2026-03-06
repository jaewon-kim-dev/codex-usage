# codex-usage

Fast Codex usage analyzer written in Rust.

<img width="1268" height="535" alt="image" src="https://github.com/user-attachments/assets/18072c86-b15d-4e3f-a2a5-8dc0eb440644" />


## Setup

### Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

If you want the Rust toolchain to be available in every new shell, add this to your shell profile:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

This is the preferred global setup. If `$HOME/.cargo/bin` is already in your `PATH`, you usually do not need an alias at all.

### Install `codex-usage`

```bash
git clone <your-repo-url>
cd codex-usage
cargo install --path .
```

By default, Cargo installs the binary to:

```bash
$HOME/.cargo/bin/codex-usage
```

### Check what will run

Before using the command, it is worth checking whether your shell resolves `codex-usage` to the Cargo binary, a shell alias, or something else:

```bash
type -a codex-usage
type codex-usage
command -v codex-usage
```

### If an existing alias overrides the binary

If `codex-usage` is already aliased to another command, remove that alias in the current shell:

```bash
unalias codex-usage
hash -r
```

To make the change persistent for future terminals, remove or replace the alias in your shell profile such as `~/.zshrc`, `~/.bashrc`, or `~/.config/fish/config.fish`.

If you still want to keep an alias, note that this command only affects the current terminal session:

```bash
alias codex-usage="$HOME/.cargo/bin/codex-usage"
```

To make that alias persistent globally for your user, add it to your shell profile and reload the shell:

```bash
echo 'alias codex-usage="$HOME/.cargo/bin/codex-usage"' >> ~/.zshrc
source ~/.zshrc
```

For most setups, using `PATH` is cleaner than using an alias because every new shell will resolve `codex-usage` directly to the installed binary.

## Usage

### Basic commands

```bash
codex-usage
codex-usage daily
codex-usage daily --split-by-model
codex-usage monthly
codex-usage monthly --split-by-model
codex-usage sessions
```

Use `--split-by-model` to emit separate daily or monthly rows when multiple models were used in the same period.

## Performance

Based on previous local measurements on the same machine, `codex-usage` was substantially faster than `ccusage-codex` for the JSON daily report path.

- `codex-usage daily --json --refresh-cache`: about `3.27s`
- `codex-usage daily --json`: about `0.42s`
- `ccusage-codex daily --json`: about `109.93s`

In those runs, `codex-usage` was roughly `33x` faster on a cold run and about `260x` faster on a warm run.

### Why it is faster

`codex-usage` is faster mainly because it does less work per session file and keeps the hot path simple.

- It scans JSONL files with a streaming reader instead of loading whole files into memory first.
- It uses a cheap byte-pattern prefilter to skip irrelevant lines before JSON deserialization.
- It only parses the event types needed for usage accounting.
- It avoids expensive global event reshuffling and aggregates usage directly during scanning.
- It processes session files in parallel with Rayon.
- It keeps binary cache files for parsed session summaries, so repeated runs can reuse unchanged files.
- It narrows the candidate file set early when date filters are provided.

### Date filters

```bash
codex-usage daily --since 20260301 --until 20260306
```

### JSON output

```bash
codex-usage monthly --json
codex-usage sessions --json
```

### Refresh cache

```bash
codex-usage --refresh-cache
codex-usage daily --refresh-cache
```

### Custom Codex home

```bash
codex-usage daily --codex-home /path/to/.codex
```

You can also set `CODEX_HOME` in your shell environment.

### Custom timezone

```bash
codex-usage daily --timezone UTC
```
