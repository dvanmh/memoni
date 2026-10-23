# fcitx5-cursorrect

A small fcitx5 addon that exposes D-Bus method to get the focused input
context's cursor rectangle, without touching fcitx5's classic UI addon.

    service    org.fcitx.Fcitx5
    object     /cursorrect
    interface  org.fcitx.Fcitx.CursorRect

    GetCursorRect() -> (iiii)  x, y, w, h
    errors with org.example.CursorRect.Error.NoFocus when nothing is focused

## Build

Requires fcitx5 development files and CMake:

```sh
cmake -B build
cmake --build build
sudo cmake --install build
```

Then restart fcitx5:
```sh
fcitx5 -r
```

## Usage

Set the following in memoni's config:

```toml
text_caret_command = "busctl --user call org.fcitx.Fcitx5 /cursorrect org.fcitx.Fcitx.CursorRect GetCursorRect | cut -d ' ' -f2-"
```

Or, to test it standalone: focus a text field in an IM-aware app, then:

```sh
busctl --user call org.fcitx.Fcitx5 /cursorrect org.example.CursorRect GetCursorRect
```

## Caveat: keymap-triggered activation

When memoni is opened via a standard window manager keybinding, holding the
keymap can leave the previously focused window unfocused. In that state,
`GetCursorRect` has no focused input context to query and fails, so memoni
falls back to the next window position mode (see `sample_config.toml`).

To use fcitx5-cursorrect correctly, map memoni's opening keymap with
[keyd](https://github.com/rvaiya/keyd) or something similar instead, which
keeps the focused window focused while memoni launches.
