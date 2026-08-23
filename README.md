<div align="center">

<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/light-banner.png">
    <source media="(prefers-color-scheme: light)" srcset="assets/dark-banner.png">
    <img alt="ask" src="assets/dark-banner.png" width="320">
  </picture>
</h1>

A small CLI for asking AI questions from your terminal.

[Motivation](#motivation) •
[Install](#install) •
[Usage](#usage) •
[How it works](#how-it-works)

</div>

Ask a question and get back to your shell:

```console
$ ask how do i find TODO or FIXME comments in src
Use ripgrep:
  rg -n 'TODO|FIXME' src

$ rg -n 'TODO|FIXME' src
src/generated/client.ts:18:// TODO: generated placeholder
src/generated/schema.ts:204:// FIXME: compatibility shim
src/api.ts:42:// TODO: retry failed requests

$ ask -c how do i exclude generated files
Add an exclude:
  rg -n --glob '!generated/**' 'TODO|FIXME' src

$ rg ...
```

Or just run `ask` to start a session:

```console
$ ask
> why is this container restarting?
Check its exit code:
  docker inspect -f '{{.State.ExitCode}}' my-container

> it says 137. what does that mean?
Exit code 137 usually means SIGKILL, often from running out of memory.

> thank you
You're welcome.
> ^D

$ ...
```

Every answer is saved. Use `ask -c` to continue the latest one in this folder.

## Motivation

I made ask because I wanted a normal terminal command for asking AI questions, whichever agent or model I used. I didn't want a TUI. I wanted to ask something, read the answer, and get my shell back.

Coding agents do a lot in the background, and I don't always want that. With ask, the agent stays read-only. I run the commands and make the changes myself. That way I actually learn.

## Install

```sh
curl -fsSL https://benja.dev/ask/install.sh | sh
```

The installer supports Intel/ARM Macs and x86_64/ARM64 Linux. It verifies the release checksum and installs to `~/.local/bin/ask`. Set `ASK_INSTALL_DIR` to change the destination.

## Usage

```text
ask [QUESTION...]          ask once
ask                        start a session
ask -c [QUESTION...]       continue the latest session here
ask --sessions             reopen a saved session
ask --settings             set defaults for new sessions
ask --upgrade              update ask
ask -V                     print the version
```

Answers go to stdout. Prompts and errors go to stderr, so one-shot answers are safe to pipe. Agents start in the current directory and run read-only.

ask only checks for updates when you run `ask --upgrade`.

## How it works

Under the hood, ask runs Codex, Claude Code, Pi, or OpenCode using the CLI already installed and logged in on your machine.

For a new session, ask uses your default agent and settings. When you continue a session, it restores the saved agent, settings, and underlying agent session ID. It starts the selected CLI read-only in your current folder, prints the answer to stdout, and saves the turn automatically.

## Agents and models

Run `ask --settings` to choose the default agent and any model it supports. Use `/settings` inside a session to change its model, reasoning, or answer instructions. Start a new session to switch agents.

## Contributing

Requires Rust 1.88 or newer. Before opening a pull request:

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Installer changes also need `shellcheck install.sh tests/install.sh` and `sh tests/install.sh`.

## License

[MIT](LICENSE). Have fun :)
