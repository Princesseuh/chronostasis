# Chronostasis

Chronostasis is a modding suite for the Final Fantasy XIII trilogy (XIII, XIII-2 and Lightning Returns). It aims to be a one-stop shop for players and modders, bundling
quality-of-life improvements, bug fixes, mod management, and modding tools into one cross-platform, fully open source project.

## Getting Started

[Visit the Chronostasis website for more detailed guides and instructions](https://chronostasis.erika.florist), or read below for quick start information.

## Players

Visit the [Releases](https://github.com/Princesseuh/chronostasis/releases) page and download the latest version for your system. Chronostasis is available on Windows, macOS and Linux, as both an easy-to-use GUI and a CLI. If you're unsure, download the GUI version.

### GUI

Follow the instructions to install both fixes and mods.

### CLI

Run `chronostasis install` and follow the prompts.

## Developers & Modders

### Crates

Chronostasis is built as a set of crates you can use in your own tools:

- `ff13-formats` reads and writes the game's file formats.
- `ff13-laa-patch` applies and reverts the Large Address Aware (LAA) PE patch.
- `ff13-community` handles various operations related to community formats, such as Nova Modpacks.
- `ff13-hooks` is the in-process `d3d9.dll` proxy that carries the runtime fixes.
- `ff13` is the high-level entry point, re-exporting many of the above crates and adding game discovery, archive unpack/repack, proxy setup, and modpack install.

The following two crates are specific to the two interfaces Chronostasis can be used from.

- `chronostasis-cli` is the `chronostasis` command-line tool.
- `chronostasis-gui` is the desktop app (egui), with the Player and Modder tabs.

## Acknowledgements

Chronostasis builds upon the reverse engineering work done by the greater FFXIII community since the game's release.

Special thanks to the following people for their absolutely amazing work over the years, without which it would not have been close to possible to make Chronostasis.

- [Krisan Thyme](https://www.patreon.com/illusiovitae)
- [GreenThumb2](https://www.nexusmods.com/profile/GreenThumb2/mods)
- [Surihix](https://github.com/Surihix)
- [Joschka](https://github.com/Joschuka)
- [rebtd7](https://github.com/rebtd7)
- [Dendonflo](https://github.com/Dendonflo)

## License

Chronostasis is licensed under the [MIT License](./LICENSE).
