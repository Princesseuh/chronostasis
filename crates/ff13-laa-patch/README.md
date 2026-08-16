# ff13-laa-patch

Applies and reverts the Large Address Aware patch on Final Fantasy XIII's 32-bit executable, letting the game use more than 2 GB of address space (needed for heavy texture mods).

The patch flips the `IMAGE_FILE_LARGE_ADDRESS_AWARE` characteristic in the PE header, keeps an untouched copy of the executable so the patch can be reverted cleanly, and refuses files that are not PE executables.

Part of [Chronostasis](https://github.com/Princesseuh/chronostasis), a modding suite for the Final Fantasy XIII trilogy.
