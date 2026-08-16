use crate::components::{inline_code, page_title, section_heading, site_nav};
use crate::layout::layout;
use maud::{Markup, html};
use maudit::assets::StyleOptions;
use maudit::route::prelude::*;

fn entry(cmd: &str, body: Markup) -> Markup {
    html! {
        div {
            code class="rounded bg-paper-2 px-2 py-1 font-mono text-[0.8rem] text-ink ring-1 ring-inset ring-rule" { (cmd) }
            div class="mt-2 max-w-xl leading-relaxed text-body" { (body) }
        }
    }
}

#[route("/cli")]
pub struct CliReference;

impl Route for CliReference {
    fn render(&self, ctx: &mut PageContext) -> impl Into<RenderResult> {
        ctx.assets
            .include_style_with_options("src/prin.css", StyleOptions { tailwind: true })?;

        // Drives both the content and the table of contents.
        let sections: [(&str, &str, Markup); 5] = [
            (
                "setup",
                "Setup",
                html! {
                    (entry("chronostasis install", html! {
                        p {
                            "The guided setup. Finds your game, installs the "
                            (inline_code("d3d9.dll"))
                            " proxy that carries the runtime fixes, offers to unpack the game for mods and to apply the 4GB patch, then prints the Steam launch options. Every prompt has a matching flag ("
                            (inline_code("--mods"))
                            ", "
                            (inline_code("--no-laa"))
                            ", "
                            (inline_code("--force"))
                            "...), so it also runs unattended. For a non-Steam copy, name the game and pass "
                            (inline_code("--path <dir>"))
                            "; the location is remembered for every later command."
                        }
                    }))
                    (entry("chronostasis configure <game>", html! {
                        p {
                            "Opens the game's "
                            (inline_code("chronostasis.ini"))
                            " in your editor. All the proxy settings live there."
                        }
                    }))
                    (entry("chronostasis launch-options", html! {
                        p { "Prints the launch options to paste into Steam when playing under Proton. On Windows there's nothing to set." }
                    }))
                    (entry("chronostasis uninstall <game>", html! {
                        p {
                            "Removes the proxy and puts back any "
                            (inline_code("d3d9.dll"))
                            " it had replaced."
                        }
                    }))
                },
            ),
            (
                "installs",
                "Game installs",
                html! {
                    (entry("chronostasis list", html! {
                        p { "Every install found in your Steam libraries (plus manually registered ones), with its patch status." }
                    }))
                    (entry("chronostasis info <game>", html! {
                        p { "Paths and patch status for one game." }
                    }))
                    (entry("chronostasis patch <game>", html! {
                        p {
                            "Applies the Large Address Aware patch, letting the 32-bit exe use 4 GB of memory instead of 2. "
                            (inline_code("--revert"))
                            " restores the original. XIII and XIII-2 only; Lightning Returns doesn't need it."
                        }
                    }))
                    (entry("chronostasis forget <path>", html! {
                        p { "Drops a manually registered install. Steam-detected ones are unaffected." }
                    }))
                },
            ),
            (
                "archives",
                "Archives",
                html! {
                    (entry("chronostasis unpack <game> --all", html! {
                        p {
                            "Unpacks the whole game (main, script and zone archives) into loose files, which is what mods install into. It takes a while. "
                            (inline_code("--revert"))
                            " deletes the loose files again and returns the game to packed mode, leaving your mods folder alone."
                        }
                    }))
                    (entry("chronostasis unpack <game>", html! {
                        p {
                            "Without "
                            (inline_code("--all"))
                            ", extracts just the main system archive, into "
                            (inline_code("<data>/_unpacked"))
                            " by default. "
                            (inline_code("--only <text>"))
                            " keeps files whose path contains the text."
                        }
                    }))
                    (entry("chronostasis repack <game>", html! {
                        p {
                            "Rebuilds "
                            (inline_code("white_img"))
                            " and its filelist from the loose tree, for mods that should ship packed."
                        }
                    }))
                },
            ),
            (
                "mods",
                "Mods",
                html! {
                    (entry("chronostasis mod install <dir>", html! {
                        p {
                            "Installs an extracted modpack, existing Nova Chrysalia packs included. Game files are backed up first, and "
                            (inline_code("mod uninstall <dir>"))
                            " puts them back. "
                            (inline_code("mod info <dir>"))
                            " shows what a pack contains without touching anything."
                        }
                    }))
                    (entry("chronostasis mod hd-install <dir>", html! {
                        p {
                            "Installs a community HD pack, working out which kind it is from the folder layout. Handles the HD Fonts and GUI pack as well as FF XIII HD. "
                            (inline_code("--dry-run"))
                            " reports what it would write."
                        }
                    }))
                    (entry("chronostasis mod model-swap <game> <character> [donor]", html! {
                        p {
                            "Swaps a character's model using only files the game already ships. Give a costume name ("
                            (inline_code("model-swap xiii lightning lebreau"))
                            ") or a raw model code like "
                            (inline_code("n910"))
                            " for a generic swap, retargeted by bone name. Leave the donor off to list the known costumes."
                        }
                    }))
                    (entry("chronostasis mod import <dir> <game>", html! {
                        p {
                            "Copies a built mod's "
                            (inline_code("models/mod/"))
                            " tree into the game's overrides folder."
                        }
                    }))
                    (entry("chronostasis mod hd-textures / hd-swap / hd-subdivide", html! {
                        p {
                            "The pieces "
                            (inline_code("hd-install"))
                            " is built from, exposed for pack authors: decode an HD pack's obfuscated "
                            (inline_code(".bin"))
                            " textures, build an outfit swap from an HD-Models script, or run a full model edit script against a "
                            (inline_code(".trb"))
                            "."
                        }
                    }))
                },
            ),
            (
                "format-tools",
                "Format tools",
                html! {
                    p class="max-w-xl leading-relaxed text-body" {
                        "For mod authors: each of these turns a game format into something editable and back. Anything they overwrite is backed up once, as a "
                        (inline_code(".bak"))
                        " beside the original."
                    }
                    (entry("chronostasis mod texture <list|extract|replace|xgr-repack>", html! {
                        p {
                            "Textures in an "
                            (inline_code(".imgb"))
                            ": list them, extract to DDS, replace one in place. "
                            (inline_code("xgr-repack"))
                            " swaps a DDS of any size into an "
                            (inline_code(".xgr"))
                            " interface file, rebuilding the imgb around it."
                        }
                    }))
                    (entry("chronostasis mod text <decode|encode>", html! {
                        p {
                            "Localized "
                            (inline_code(".ztr"))
                            " text to an editable "
                            (inline_code(".txt"))
                            " and back. "
                            (inline_code("--compress"))
                            " matches the game's compressed layout."
                        }
                    }))
                    (entry("chronostasis mod wdb <decode|encode>", html! {
                        p {
                            (inline_code(".wdb"))
                            " databases to JSON and back."
                        }
                    }))
                    (entry("chronostasis mod trb <extract|repack>", html! {
                        p {
                            (inline_code(".trb"))
                            " texture bundles: extract every texture to DDS, repack from the edited folder. Resized textures are fine; the imgb is rebuilt to fit."
                        }
                    }))
                    (entry("chronostasis mod audio <extract|replace|extract-all>", html! {
                        p {
                            (inline_code(".scd"))
                            " audio to "
                            (inline_code(".ogg"))
                            " (music) or "
                            (inline_code(".wav"))
                            " (sound effects) and back. "
                            (inline_code("extract-all"))
                            " sweeps a whole directory."
                        }
                    }))
                    (entry("chronostasis mod movie <extract|repack|extract-all>", html! {
                        p {
                            (inline_code(".wmp"))
                            " movie containers to Bink files and back, keeping the "
                            (inline_code("movie_items"))
                            " database in sync."
                        }
                    }))
                },
            ),
        ];

        Ok(layout(
            "CLI reference | Chronostasis",
            html! {
                div class="grid items-start gap-x-12 gap-y-10 sm:grid-cols-[minmax(0,1fr)_16rem]" {
                    div {
                        (page_title("CLI reference"))
                        div class="mt-7 max-w-xl space-y-3 leading-relaxed text-body" {
                            p {
                                "What each "
                                (inline_code("chronostasis"))
                                " command does, grouped by task. Add "
                                (inline_code("--help"))
                                " to any of them for the full list of flags."
                            }
                            p {
                                "Commands that take a game accept "
                                (inline_code("xiii"))
                                ", "
                                (inline_code("xiii2"))
                                " and "
                                (inline_code("lr"))
                                "."
                            }
                        }

                        @for (id, title, body) in &sections {
                            section id=(id) class="mt-12 scroll-mt-8" {
                                (section_heading(title))
                                div class="mt-5 space-y-6" { (body) }
                            }
                        }
                    }

                    div class="space-y-8" {
                        (site_nav("/cli"))
                        nav class="flex flex-col" aria-label="On this page" {
                            div class="mb-2 text-[0.68rem] uppercase tracking-[0.18em] text-faint" { "On this page" }
                            @for (id, title, _) in &sections {
                                a class="py-1 text-sm text-body transition-colors hover:text-cyan" href={ "#" (id) } { (title) }
                            }
                        }
                    }
                }
            },
        ))
    }
}
