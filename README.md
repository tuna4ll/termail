# termail

A terminal Maildir reader that lays conversations out as navigable thread maps.

## Run

```sh
cargo run -- --maildir ~/Mail
```

Run without arguments to open the built-in demo mailbox. Termail reads messages from `cur/` and
`new/`, watches for changes, and groups replies using `In-Reply-To` and `References` headers.

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
| `:` | Open the command palette |
| `?` | Show all shortcuts |

Composed messages are saved to `.Drafts/new`; Termail does not send mail yet. Archive and Trash
folders are created as Maildir++ folders when first used.

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
