use crate::components::{inline_code, page_title, section_heading, site_nav};
use crate::layout::layout;
use maud::{Markup, html};
use maudit::assets::StyleOptions;
use maudit::route::prelude::*;

const REPO: &str = "https://github.com/Princesseuh/chronostasis";

#[route("/troubleshooting")]
pub struct Troubleshooting;

impl Route for Troubleshooting {
    fn render(&self, ctx: &mut PageContext) -> impl Into<RenderResult> {
        ctx.assets
            .include_style_with_options("src/prin.css", StyleOptions { tailwind: true })?;

        // Drives both the content and the table of contents.
        let items: [(&str, &str, Markup); 1] = [(
            "wont-launch",
            "The game won't launch",
            html! {
                p {
                    "If you get an \"Application load error\", the LAA patch is clashing with Steam's integrity check. On Proton, remove the on-disk patch and add "
                    (inline_code("PROTON_FORCE_LARGE_ADDRESS_AWARE=1"))
                    " to your launch options instead."
                }
            },
        )];

        Ok(layout(
            "Troubleshooting | Chronostasis",
            html! {
                div class="grid items-start gap-x-12 gap-y-10 sm:grid-cols-[minmax(0,1fr)_16rem]" {
                    div {
                        (page_title("Troubleshooting"))
                        p class="mt-7 max-w-xl leading-relaxed text-body" {
                            "A few common issues and how to fix them. Still stuck? "
                            a class="text-cyan underline decoration-cyan/40 underline-offset-2 hover:decoration-cyan" href={ (REPO) "/issues" } { "Open an issue on GitHub" }
                            "."
                        }

                        @for (id, title, body) in &items {
                            section id=(id) class="mt-12 scroll-mt-8" {
                                (section_heading(title))
                                div class="mt-3 space-y-3 leading-relaxed text-body" { (body) }
                            }
                        }
                    }

                    div class="space-y-8" {
                        (site_nav("/troubleshooting"))
                        nav class="flex flex-col" aria-label="On this page" {
                            div class="mb-2 text-[0.68rem] uppercase tracking-[0.18em] text-faint" { "On this page" }
                            @for (id, title, _) in &items {
                                a class="py-1 text-sm text-body transition-colors hover:text-cyan" href={ "#" (id) } { (title) }
                            }
                        }
                    }
                }
            },
        ))
    }
}
