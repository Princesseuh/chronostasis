# ff13

Everything needed to mod the Steam release of Final Fantasy XIII from Rust: find installed copies (Steam or manually registered, Windows and Proton), unpack and repack the game archives, apply the LAA patch, set up the `d3d9.dll` proxy with its runtime fixes, and install, uninstall, and import modpacks, including existing Nova Chrysalia `.ncmp` packs.

Building your own tool on top of Chronostasis? Depend on this crate: the Chronostasis CLI and GUI are both thin layers over it, so anything they can do, your tool can too.

Part of [Chronostasis](https://github.com/Princesseuh/chronostasis), a modding suite for the Final Fantasy XIII trilogy.
