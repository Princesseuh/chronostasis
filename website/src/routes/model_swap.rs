use crate::components::{page_title, section_heading, site_nav};
use crate::layout::layout;
use maud::html;
use maudit::assets::StyleOptions;
use maudit::route::prelude::*;

#[route("/model-swap")]
pub struct ModelSwap;

impl Route for ModelSwap {
    fn render(&self, ctx: &mut PageContext) -> impl Into<RenderResult> {
        ctx.assets
            .include_style_with_options("src/prin.css", StyleOptions { tailwind: true })?;

        // Mirrors the recipes in `ff13::swaps`.
        let swaps = [
            ("Lightning", "Lebreau"),
            ("Sazh", "Gadot"),
            ("Hope", "Maqui"),
            ("Hope", "Deportee"),
            ("Vanille", "Nora"),
            ("Vanille", "Deportee"),
        ];

        Ok(layout(
            "Model swap | Chronostasis",
            html! {
                div class="grid items-start gap-x-12 gap-y-10 sm:grid-cols-[minmax(0,1fr)_16rem]" {
                    div {
                        (page_title("Model swap"))

                        div class="mt-7 space-y-4 leading-relaxed text-body" {
                            p {
                                "Swap character models and outfits across the Final Fantasy XIII trilogy."
                            }
                            p {
                                "Chronostasis applies community model swaps and includes the tools to build your own, from simple outfit swaps to imported meshes."
                            }
                        }

                        section class="mt-12" {
                            (section_heading("Supported swaps"))
                            p class="mt-5 max-w-xl leading-relaxed text-body" {
                                "On top of HD, cutscene-quality gameplay models for the whole party (Lightning, Snow, Sazh, Hope, Vanille and Fang), these costume swaps ship built in:"
                            }
                            ul class="mt-5 grid max-w-xl gap-x-8 gap-y-2 sm:grid-cols-2" {
                                @for (character, costume) in swaps {
                                    li class="flex items-baseline gap-2" {
                                        span class="font-medium text-ink" { (character) }
                                        span class="text-faint" { "→" }
                                        span class="text-body" { (costume) }
                                    }
                                }
                            }
                        }

                        div class="mt-12 grid aspect-video place-items-center rounded-md border border-rule bg-paper-2/60 text-sm text-faint" {
                            "Screenshots soon"
                        }
                    }

                    (site_nav("/model-swap"))
                }
            },
        ))
    }
}
