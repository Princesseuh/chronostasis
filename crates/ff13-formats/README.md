# ff13-formats

Readers and writers for the file formats of the Steam release of Final Fantasy XIII: the White archive (`white_img` and its filelist), WPD/WDB containers, IMGB textures, TRB resource bundles, ZTR localized text, SCD audio, and the model, skeleton, and animation resources inside them.

- Parse and rebuild archives byte-exactly (round-trip tested against real game data).
- Extract and repack textures (DDS in and out).
- Decode models, skeletons, and animation clips into a simple in-memory form.
- Decrypt and re-encrypt the XIII-2 and Lightning Returns filelists.

Plain Rust with no GUI or graphics dependencies, so it fits CLI tools, build scripts, and modding pipelines alike.

Part of [Chronostasis](https://github.com/Princesseuh/chronostasis), a modding suite for the Final Fantasy XIII trilogy.
