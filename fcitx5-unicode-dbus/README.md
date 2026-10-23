# fcitx5-unicode-dbus

A small fcitx5 addon that exposes D-Bus methods to trigger the unicode input
mode of the built-in `unicode` addon. This exists because fcitx5 has no CLI
command for it, and memoni grabs the keyboard while shown, so the registered
unicode keymaps cannot be used from within memoni.

Two methods are provided:

| Method              | Equivalent key (default) | Effect                                |
| ------------------- | ------------------------ | ------------------------------------- |
| `TriggerUnicode`    | `Control+Alt+Shift+U`    | Open the unicode search/picker        |
| `TriggerUnicodeHex` | `Control+Shift+U`        | Enter direct hex codepoint input mode |

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
ime_unicode_input_command = "busctl --user call org.fcitx.Fcitx5 /unicode org.fcitx.Fcitx.Unicode1 TriggerUnicode"
ime_unicode_hex_input_command = "busctl --user call org.fcitx.Fcitx5 /unicode org.fcitx.Fcitx.Unicode1 TriggerUnicodeHex"
```

Or, to test it standalone:

```sh
busctl --user call org.fcitx.Fcitx5 /unicode org.fcitx.Fcitx.Unicode1 TriggerUnicode
busctl --user call org.fcitx.Fcitx5 /unicode org.fcitx.Fcitx.Unicode1 TriggerUnicodeHex
```
