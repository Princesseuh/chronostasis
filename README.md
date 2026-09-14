# Chronostasis

Chronostasis is a cross-platform format and modding toolkit for the Final Fantasy XIII trilogy (XIII, XIII-2 and Lightning Returns). It focuses on parsing, inspecting, converting, and authoring game assets; runtime fixes and game launching are handled by dedicated projects such as NovaFix.

<div align="center">
  <img width="45%" alt="image" src="https://github.com/user-attachments/assets/6e643c6d-5332-484f-b68b-484f5ae3cbdf" />
  <img width="45%" height="978" alt="image" src="https://github.com/user-attachments/assets/3b65fb50-7856-4f37-959f-85ef203a98e3" />
</div>


## Getting Started

[Visit the Chronostasis website for more detailed guides and instructions](https://chronostasis.erika.florist), or read below for quick start information.

## Tools

Chronostasis is available on Windows, macOS and Linux as a GUI and CLI. It does not launch or patch the game. Use the tools to inspect game files, unpack/repack archives, preview assets, and build mods.

## Developers & Modders

### Crates

Chronostasis is built as a set of crates you can use in your own tools:

- `ff13-formats` reads and writes the game's file formats.
- `ff13-community` handles various operations related to community formats, such as Nova Modpacks.
- `ff13` is the high-level entry point for game-file discovery, archive unpack/repack, and modding operations.

The following two crates are specific to the two interfaces Chronostasis can be used from.

- `chronostasis-cli` is the `chronostasis` command-line tool.
- `chronostasis-gui` is the desktop app (egui) for format inspection and modder tools.

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
