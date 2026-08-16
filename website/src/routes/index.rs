use crate::components::{feature_heading, github_icon, page_title, section_heading, site_nav};
use crate::layout::layout;
use maud::{Markup, html};
use maudit::assets::{ImageFormat, ImageOptions, RenderWithAlt, StyleOptions};
use maudit::route::prelude::*;

const REPO: &str = "https://github.com/Princesseuh/chronostasis";

// `flip` moves the media to the left on desktop, for an alternating zig-zag.
fn highlight(title: &str, body: Markup, media: Markup, flip: bool) -> Markup {
    let text_class = if flip { "sm:order-2" } else { "" };
    let media_class = if flip { "sm:order-1" } else { "" };
    html! {
        div class="mt-12 grid gap-x-10 gap-y-5 sm:grid-cols-2 sm:items-center" {
            div class=(text_class) {
                (feature_heading(title))
                div class="mt-3 space-y-3 leading-relaxed text-body" { (body) }
            }
            div class=(media_class) { (media) }
        }
    }
}

fn media_placeholder() -> Markup {
    html! {
        div class="grid aspect-video place-items-center rounded-md border border-rule bg-paper-2/60 text-sm text-faint" {
            "Footage soon"
        }
    }
}

#[route("/")]
pub struct Index;

impl Route for Index {
    fn render(&self, ctx: &mut PageContext) -> impl Into<RenderResult> {
        ctx.assets
            .include_style_with_options("src/prin.css", StyleOptions { tailwind: true })?;

        let play_card = ctx.assets.add_image_with_options(
            "src/images/play-card.png",
            ImageOptions {
                format: Some(ImageFormat::WebP),
                ..Default::default()
            },
        )?;

        let model_viewer = ctx.assets.add_image_with_options(
            "src/images/model-viewer.png",
            ImageOptions {
                format: Some(ImageFormat::WebP),
                ..Default::default()
            },
        )?;

        let highlights = [
            (
                "Open source, runs anywhere",
                html! {
                    p {
                        "Chronostasis is free and open source, developed in the open on GitHub."
                    }
                    p {
                        "It runs natively on Windows, macOS and Linux, with full support for Steam installs under Proton."
                    }
                },
                html! {
                    figure class="shot overflow-hidden rounded-md border border-rule" {
                        (play_card.render("A Steam copy of Final Fantasy XIII on Linux, set up and ready to play"))
                    }
                },
            ),
            (
                "Uncapped resolution & framerate",
                html! {
                    p {
                        "FFXIII's PC ports raised the resolution and framerate without updating the engine to match, causing various UI bugs and borked facial animations."
                    }
                    p {
                        "Chronostasis fixes both and unlocks proper high resolution and framerate support across the trilogy."
                    }
                },
                media_placeholder(),
            ),
            (
                "Bring your mods",
                html! {
                    p {
                        "Chronostasis opens the game's archives, with previews for textures, audio and text, and a 3D viewer that draws models with the game's own shaders."
                    }
                    p {
                        "Convert any of it to editable formats and back to build your own mod, or install the community's as-is."
                    }
                },
                html! {
                    figure class="shot overflow-hidden rounded-md border border-rule" {
                        (model_viewer.render("A Final Fantasy XIII enemy model in the built-in 3D viewer"))
                    }
                },
            ),
        ];

        Ok(layout(
            "Chronostasis: Final Fantasy XIII modding suite",
            html! {
                div class="grid items-start gap-x-12 gap-y-10 sm:grid-cols-[minmax(0,1fr)_16rem]" {
                    div {
                        (page_title("Chronostasis"))

                        div class="mt-7 space-y-4 leading-relaxed" {
                            p {
                                "Chronostasis is a modding suite for the Final Fantasy XIII trilogy (XIII, XIII-2 and Lightning Returns)."
                            }
                            p {
                                "It aims to be a one-stop shop for players and modders, bundling quality-of-life improvements, bug fixes, mod management, and modding tools."
                            }
                        }

                        div class="mt-8 flex flex-wrap items-center gap-3" {
                            a class="inline-flex items-center rounded-md bg-cyan px-5 py-2.5 text-sm font-medium leading-none text-white shadow-sm transition hover:bg-cyan-bright" href="/getting-started" {
                                span class="relative top-[2px]" { "Get started" }
                            }
                            a class="inline-flex items-center gap-2 rounded-md border border-rule-2 px-4 py-2.5 text-sm font-medium leading-none text-ink transition hover:border-faint hover:bg-white/60" href=(REPO) {
                                (github_icon())
                                span class="relative top-[2px]" { "GitHub" }
                            }
                        }
                    }

                    (site_nav("/"))
                }

                section class="mt-20" {
                    (section_heading("Highlights"))
                    @for (i, (title, body, media)) in highlights.into_iter().enumerate() {
                        (highlight(title, body, media, i % 2 == 1))
                    }
                }

                figure class="mt-28 text-center" {
                    div class="select-none font-serif text-6xl leading-none text-cyan/50" { "“" }
                    blockquote class="mx-auto -mt-3 max-w-2xl text-balance text-xl font-light italic leading-relaxed text-ink sm:text-2xl" {
                        "Maybe we'd fall short. Maybe we'd never even come close. But someone, someday, would know we'd tried."
                    }
                }
            },
        ))
    }
}
