# chronostasis-cli

The `chronostasis` command-line tool: mod and fix the Steam release of Final Fantasy XIII on Windows and Linux (Proton).

```sh
cargo install chronostasis-cli
chronostasis list                 # find installed games
chronostasis info xiii            # paths and LAA-patch status
chronostasis patch xiii           # apply the LAA patch
chronostasis mod install <pack>   # install a modpack
```

Game discovery, archive unpack/repack, the LAA patch, the `d3d9.dll` proxy with runtime fixes, mod install and uninstall (Nova Chrysalia packs included), HD texture and model pack installs, and format tools for modders.

Part of [Chronostasis](https://github.com/Princesseuh/chronostasis), a modding suite for the Final Fantasy XIII trilogy.
