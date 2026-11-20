# Memoni

Memoni (MEMOry MONItor) is a fast, keyboard-driven, visually minimal clipboard manager for X11.

<p align="center">
  <img src="./preview/demo.png" alt="Screenshot of Memoni window with demo clipboard items" />
</p>

## Features

### Everything captured

Text, rich text, images, files, anything. Data of every MIME type is kept and pasted back intact (except your
passwords[*](#sensitive-content) of course, don't worry).

<p align="center">
  <img src="./preview/content_types.webp" alt="Copying rich text, files and image, then pasting the rich text and the image" />
</p>

### Vim and Emacs inspired keymaps

Vim-style navigation in normal mode, Emacs-style editing in the search prompt.

<p align="center">
  <img src="./preview/keymaps.webp" alt="Vim navigation through the history, deleting an item with dd, then opening search with /, Emacs-style navigation and query editing, and pasting the selected item with Enter" />
</p>

### Search

Filter clipboard history with plain, fuzzy, or regex search.

<p align="center">
  <img src="./preview/search.webp" alt="Typing a query in plain search mode, switching to fuzzy mode without losing the query, then switching to regex mode with a fresh query" />
</p>

### Automatic pasting

Selecting an item pastes it automatically with `Ctrl+V`, or with a custom keymap configured per application.

<p align="center">
  <img src="./preview/paste.webp" alt="Pasting into LibreOffice Writer, then Emacs, then Neovim in a terminal, by selecting an item in Memoni each time" />
</p>

For the PRIMARY selection, the item is pasted by middle-clicking at the pointer position instead (not configurable).

<p align="center">
  <img src="./preview/primary_paste.webp" alt="Pasting at the pointer position into LibreOffice Writer, then Emacs, then Neovim in a terminal, by selecting an item in Memoni's PRIMARY window each time" />
</p>

### Pins and bookmarks

Save items by pinning them to the top of the history, or bookmarking them to a dedicated list.

<p align="center">
  <img src="./preview/pins_bookmarks.webp" alt="Pinning and bookmarking items, then closing the window, copying more items, and reopening to find the pinned ones still at the top and the bookmarks intact" />
</p>

### Flexible window placement

Show the window centered in the focused monitor, next to the pointer, or next to the text caret.

<p align="center">
  <table>
    <tr>
      <td align="center" valign="top" width="50%">
        <img src="./preview/monitor_placement.webp" alt="Opening and closing the window" />
        <br />
        <code>monitor</code>
        <br />
        <small>Centered in the focused monitor</small>
      </td>
      <td align="center" valign="top" width="50%">
        <img src="./preview/pointer_placement.webp" alt="Moving the pointer and reopening the window, three times" />
        <br />
        <code>pointer</code>
        <br />
        <small>Always next to the pointer</small>
      </td>
    </tr>
    <tr>
      <td align="center" valign="top" width="50%">
        <img src="./preview/active_pointer_placement.webp" alt="Reopening the window with the pointer visible, then with it hidden by typing" />
        <br />
        <code>active_pointer</code> → <code>monitor</code>
        <br />
        <small>Next to the pointer only when it's visible on the focused monitor, otherwise centered in the focused monitor</small>
      </td>
      <td align="center" valign="top" width="50%">
        <img src="./preview/text_caret_placement.webp" alt="Opening the window in Neovim, LibreOffice Writer, and Emacs, following the caret after typing, then in Thunar, which has no focused input field" />
        <br />
        <code>text_caret</code> → <code>monitor</code>
        <br />
        <small>Next to the caret of the focused input field, otherwise centered in the focused monitor</small>
      </td>
    </tr>
  </table>
</p>

### Sensitive content

Content marked as secret by password managers (`x-kde-passwordManagerHint`) is ignored automatically.

Content can also be ignored by clipboard owner window (`ignore_windows`), clipboard owner process (`ignore_processes`),
or content itself (`ignore_contents`) (see [sample_config.toml](./sample_config.toml) for details).

Some password managers that don't set `x-kde-passwordManagerHint` (e.g., `pass`, `keepassxc-cli`) shell out to `xclip`
and similar tools for clipboard access, so the owner window has no class and the owner process can't be traced back to
the real app. For those, use the [pausing feature](#pausing) instead.

## Building

Requirements:

- Linux/X11 with OpenGL/EGL
- fontconfig (only if using custom fonts)

```sh
cargo build --release
```

## Usage

### Server

Start a server (default CLIPBOARD):

```sh
memoni server
```

Start a PRIMARY server:

```sh
memoni server --selection PRIMARY
```

### Window

Show the window:

```sh
memoni
```

```sh
memoni --selection PRIMARY
```

Press `?` in the window to view the full keymap list.

### Pausing

Pause capture:

```sh
memoni pause           # follows `pause_auto_resume_after` config (default: paused forever)
memoni pause 30m       # auto-resume in 30 minutes
memoni pause inf       # never auto-resume (until `memoni pause resume`)
memoni pause next      # resumes after the next 1 clipboard change
memoni pause next 3    # resumes after the next 3 clipboard changes
```

Resume capture:

```sh
memoni pause resume
```

Check the pause state:

```console
$ memoni pause state
not paused
$ memoni pause 30m
paused (auto-resume in 30m)
$ memoni pause state
paused (auto-resume in 29m50s)
```

### Help

For full command and option details, run:

```sh
memoni --help
memoni server --help
```

## Configuration

Customize the behavior and appearance via the configuration file at `$XDG_CONFIG_HOME/memoni/config.toml`. Note that the
application does not automatically create this file; it must be created manually.

See [sample_config.toml](./sample_config.toml) for a complete list of options and examples.

## Credits

- [Noto Sans](https://fonts.google.com/noto/specimen/Noto+Sans) (SIL Open Font License)
- [Noto Sans Math](https://fonts.google.com/noto/specimen/Noto+Sans+Math) (SIL Open Font License)
