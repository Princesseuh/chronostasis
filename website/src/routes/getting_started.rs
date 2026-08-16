use crate::components::{feature_heading, inline_code, section_heading, site_nav};
use crate::layout::layout;
use maud::{Markup, html};
use maudit::assets::StyleOptions;
use maudit::route::prelude::*;

const RELEASES: &str = "https://github.com/Princesseuh/chronostasis/releases";

fn sub_heading(text: &str) -> Markup {
    html! {
        h4 class="text-base font-medium leading-none tracking-tight text-ink" { (text) }
    }
}

fn page_link(href: &str, text: &str) -> Markup {
    html! {
        a class="text-cyan underline decoration-cyan/40 underline-offset-2 hover:decoration-cyan" href=(href) { (text) }
    }
}

#[route("/getting-started")]
pub struct GettingStarted;

impl Route for GettingStarted {
    fn render(&self, ctx: &mut PageContext) -> impl Into<RenderResult> {
        ctx.assets
            .include_style_with_options("src/prin.css", StyleOptions { tailwind: true })?;

        // Drives both the content and the table of contents.
        let sections: [(&str, &str, Markup); 2] = [
            (
                "players",
                "Players",
                html! {
                    p {
                        "Grab the latest release for your platform from "
                        (page_link(RELEASES, "GitHub Releases"))
                        ". Each release ships the desktop app and the "
                        (inline_code("chronostasis"))
                        " command-line tool; if you're not sure which you want, take the app."
                    }
                    div class="mt-8 space-y-3" {
                        (sub_heading("GUI"))
                        p {
                            "Pick your game and follow the installer. It sets up the fixes and can prepare the game for mods, a one-time unpack that writes tens of GB."
                        }
                        p {
                            "Everyday use lives in the Player tab: fixes, proxy settings and mod management."
                        }
                    }
                    div class="mt-8 space-y-3" {
                        (sub_heading("CLI"))
                        p {
                            (inline_code("chronostasis install"))
                            " walks through the whole setup: it finds the game, installs the proxy DLL that carries the runtime fixes, offers the 4GB patch, and asks whether to unpack the game for mods (tens of GB, done once)."
                        }
                        p {
                            "Under Proton, the installer prints a launch-options line at the end. Paste it into the game's properties in Steam and you're done."
                        }
                        p {
                            (inline_code("chronostasis mod install <pack>"))
                            " handles regular modpacks, existing Nova Chrysalia packs included, and "
                            (inline_code("chronostasis mod hd-install <folder>"))
                            " takes care of the community HD packs. The full command list lives in the "
                            (page_link("/cli", "CLI reference"))
                            "."
                        }
                    }
                },
            ),
            (
                "modders",
                "Modders",
                html! {
                    p {
                        "The app's Modder tab has a file browser with texture, audio and text previews, and a 3D model viewer that renders models with the game's own shaders and animations."
                    }
                    p {
                        "On the command line, the "
                        (page_link("/cli#format-tools", "format tools"))
                        " convert the game's formats to editable ones and back: textures to DDS, text and databases to plain files, plus audio and movies. For swapping character models, see "
                        (page_link("/model-swap", "model swap"))
                        "."
                    }
                },
            ),
        ];

        Ok(layout(
            "Getting started | Chronostasis",
            html! {
                div class="grid items-start gap-x-12 gap-y-10 sm:grid-cols-[minmax(0,1fr)_16rem]" {
                    div {
                        (section_heading("Getting started"))
                        p class="mt-7 max-w-xl leading-relaxed text-body" {
                            "Chronostasis runs on Windows, macOS and Linux. Steam copies running under Proton are fully supported."
                        }

                        @for (id, title, body) in &sections {
                            section id=(id) class="mt-10 scroll-mt-8" {
                                (feature_heading(title))
                                div class="mt-3 max-w-xl space-y-3 leading-relaxed text-body" { (body) }
                            }
                        }
                    }

                    div class="space-y-8" {
                        (site_nav("/getting-started"))
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
