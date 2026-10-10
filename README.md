# termail

A terminal Maildir reader that lays conversations out as navigable thread maps.

## Run

```sh
cargo run -- --maildir ~/Mail
```

Run without arguments to open the built-in demo mailbox. Termail reads messages from `cur/` and
`new/`, watches for changes, and groups replies using `In-Reply-To` and `References` headers.

Termail supports Linux, macOS and Windows. Maildir flags preserve the platform's existing filename
separator and new portable fixtures use `!2,` instead of the Windows-incompatible `:2,` form.

## Try the sandbox

The repository includes a fake Maildir, account and local test sender. It never contacts a mail
server:

```sh
cargo run -- --config sandbox/config.toml
```

Compose with `c`, close the editor, then select Send. The built-in test transport appends the full
message to `sandbox/sent.log`, while Termail stores its Maildir copy in
`sandbox/maildir/.Sent/cur`. Archive, Trash, Drafts and Outbox stay inside the sandbox too.

The same command works in PowerShell, Command Prompt and Unix shells. Delete `sandbox/sent.log` and
the generated messages under the dot-folders whenever you want to reset your local test.

## Configuration

Pass a config explicitly with `--config PATH`, set `TERMAIL_CONFIG`, or let Termail use the native
per-user config directory selected by the operating system.

```toml
name = "Tuna Kılıç"
email = "tuna@example.com"
maildir = "/path/to/Mail"
editor = ["code", "--wait"]
send_command = ["msmtp", "-t"]
```

Commands are argument arrays rather than shell strings, so paths and quoting behave consistently
on Windows, Linux and macOS. On Windows, an executable such as `msmtp.exe` can be used in
`send_command`. Without an editor setting, Termail uses `$EDITOR`, then Notepad on Windows,
TextEdit on macOS, or `vi` on Linux.

## Keys

| Key | Action |
| --- | --- |
| `j` / `k` | Move selection or scroll a message |
| `tab` | Switch between inbox and thread map |
| `enter` | Open the selected message |
| `/` | Search messages; use `from:ada` to search senders |
| `1` / `2` / `3` | Show all, unread, or starred threads |
| `m` | Toggle read status |
| `s` | Toggle star |
| `a` | Move the message to `.Archive` |
| `d` | Move the message to `.Trash` |
| `u` | Undo the last message action |
| `c` / `r` | Compose a message or reply in `$EDITOR` |
| `g` | Open the folder navigator |
| `x` | Retry the selected Outbox message |
| `:` | Open the command palette |
| `?` | Show all shortcuts |

After editing, choose Send or Save draft. Failed sends are retained in `.Outbox/new`; successful
sends are copied to `.Sent/cur`. Maildir++ folders are created when first used.

## Commands

- `:reload`
- `:filter all`
- `:filter unread`
- `:filter starred`
- `:open maildir PATH`
- `:compose`, `:reply`, `:help`, `:quit`

## Built with

- [Ratatui](https://ratatui.rs)
- [Rataflow](https://github.com/tuna-kilic/rataflow)
- [ratatui-spinner](https://github.com/ratatui/ratatui-spinner)

## License

Copyright (c) Tuna Kılıç <tuna@tunakilic.com>

This project is licensed under the MIT license ([LICENSE] or <http://opensource.org/licenses/MIT>)

[LICENSE]: ./LICENSE
