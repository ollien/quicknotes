# `quicknotes`

[![Crates.io Version](https://img.shields.io/crates/v/quicknotes)](https://crates.io/crates/quicknotes)
[![GitHub Actions Workflow Status](https://img.shields.io/github/actions/workflow/status/ollien/quicknotes/push.yml)](https://github.com/ollien/quicknotes/actions/workflows/push.yml)

`quicknotes` is a notes application that makes taking notes... quick.
You can edit your notes using your preferred text editor, and all notes are
saved locally in plain text.

## Installation

`quicknotes` can be installed from Cargo via

```
cargo install quicknotes
```

## Usage

To write a new note, all you have to do is run `quicknotes new <title>...`.
For example, to create a new note about the time machine I am building, I would
run `quicknotes new Flux Capacitor Design`.

By default, this will launch the editor stored in `$EDITOR`, but this is
configurable. All notes have a preamble, which must be preserved so that
`quicknotes` can index your note, but after that, write what you want! There
are no rules on formatting.

```
---
title = "Flux Capacitor Design"
created_at = 2025-01-11T10:58:00.587807852-05:00
---

If my calculations are correct, when this baby hits 88 miles per hour...
```

If you want to go back and revise your note, you can use `quicknotes open`,
and search for your note. In general, the index will be automatically built
when editing a note, but if for any reason you need to rebuild the index,
you can run `quicknotes index`.

`quicknotes` also supports "daily" notes, to aid your journaling. To open
today's daily note, run `quicknotes daily`. This will create a new note with
today's date, or open one if one already exists. You can also open a daily note
from a previous day by doing `quicknotes daily <offset>`, where `offset` is a
"fuzzy" date. You can either enter an absolute date (e.g. `2015-10-21`), or a
relative date (e.g. `yesterday`, `2 days ago`).

## Configuration

When you run `quicknotes` for the first time, a configuration file will be
generated for you in your operating system's configuration directory.


| Platform   | Location                                                            |
|------------|---------------------------------------------------------------------|
| Linux      | `$XDG_CONFIG_HOME/quicknotes/config.toml`                           |
| macOS      | `~/Library/Application Support/com.ollien.quicknotes/config.toml`   |
| Windows    | `C:\Users\<Username>\AppData\Roaming\ollien\quicknotes\config.toml` |

```toml
# required, directory where notes are stored
notes_root = "/home/ferris/Documents/quicknotes/"

# required, file extension for notes
note_file_extension = ".md"

# optional, uses $EDITOR if not specified, or `nano` if $EDITOR is unset
editor_command = "/usr/bin/nvim"
```

## Philosophy

I wrote `quicknotes` for my personal workflow, where I am constantly in a
shell and often just want to write something down quickly. I've found that if
I make my notes system too complicated, I'll end up doing silly things like
pasting things in random `vim` buffers. To that end, I've designed `quicknotes`
to be as frictionless as possible.

Contributions are absolutely welcome, but I will note that I have designed
`quicknotes` for my needs, first and foremost, and may not accept new features
unless they seem like something I could use. It is not my goal to design a
"swiss-army-knife" notes app, as so many others have; I wanted a note-taking
tool that would give me an easy framework I could use day-to-day, and worked
out of the box.

## Development

`quicknotes` is a bog-standard Rust project, so development should be as simple
as `cargo build` to build the project, and `cargo test` to run the tests.

## License
BSD-3
