use maud::{DOCTYPE, Markup, html};

const REPO: &str = "https://github.com/Princesseuh/chronostasis";

const FAVICON: &str = "data:image/svg+xml,%3Csvg%20xmlns='http://www.w3.org/2000/svg'%20viewBox='0%200%2032%2032'%3E%3Cpath%20d='M16%202%20L28%2016%20L16%2030%20L4%2016%20Z'%20fill='%230e94b6'/%3E%3C/svg%3E";

pub fn layout(title: &str, content: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                meta name="description" content="A modding suite for Final Fantasy XIII, written in Rust. Runs on Windows, Linux and macOS, including under Steam Proton.";
                link rel="icon" href=(FAVICON);
            }
            body class="min-h-screen text-body font-sans antialiased" {
                div class="mx-auto max-w-[52rem] px-6 py-24 sm:py-32" {
                    (content)
                    footer class="mt-24 text-xs text-faint" {
                        "A fan project. Not affiliated with Square Enix. "
                        a class="underline decoration-rule-2 underline-offset-2 hover:text-body" href=(REPO) { "Source" }
                        "."
                    }
                }
            }
        }
    }
}
