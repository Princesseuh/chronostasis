use maud::{Markup, html};

/// Sized in `em`, so it stays optically aligned at any heading size.
pub fn crystal() -> Markup {
    html! {
        span class="relative -top-[0.11em] size-[0.28em] shrink-0 rotate-45 rounded-[0.06em] bg-cyan" {}
    }
}

pub fn page_title(text: &str) -> Markup {
    html! {
        h1 class="flex items-center gap-[0.34em] text-3xl font-light leading-none tracking-tight text-ink sm:text-4xl" {
            (crystal())
            (text)
        }
    }
}

pub fn section_heading(text: &str) -> Markup {
    html! {
        h2 class="flex items-center gap-[0.34em] text-2xl font-light leading-none tracking-tight text-ink sm:text-3xl" {
            (crystal())
            (text)
            span class="ml-4 h-px flex-1 bg-rule" {}
        }
    }
}

pub fn feature_heading(text: &str) -> Markup {
    html! {
        h3 class="flex items-center gap-[0.34em] text-lg font-medium leading-none tracking-tight text-ink sm:text-xl" {
            (crystal())
            (text)
        }
    }
}

pub fn inline_code(text: &str) -> Markup {
    html! {
        code class="rounded bg-paper-2 px-1.5 py-0.5 font-mono text-[0.85em] text-ink ring-1 ring-inset ring-rule" { (text) }
    }
}

pub fn github_icon() -> Markup {
    html! {
        svg class="size-4" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" {
            path d="M12 .5C5.37.5 0 5.78 0 12.29c0 5.2 3.44 9.6 8.21 11.16.6.11.82-.25.82-.56 0-.28-.01-1.02-.02-2C5.67 21.65 5 19.27 5 19.27c-.55-1.36-1.33-1.72-1.33-1.72-1.08-.72.08-.71.08-.71 1.2.08 1.83 1.21 1.83 1.21 1.07 1.78 2.81 1.27 3.49.97.11-.76.42-1.27.76-1.56-2.66-.29-5.46-1.3-5.46-5.78 0-1.28.47-2.32 1.24-3.14-.13-.29-.54-1.48.12-3.08 0 0 1.01-.32 3.3 1.2.96-.26 1.98-.39 3-.4 1.02.01 2.04.14 3 .4 2.28-1.52 3.29-1.2 3.29-1.2.66 1.6.25 2.79.12 3.08.77.82 1.24 1.86 1.24 3.14 0 4.49-2.81 5.48-5.48 5.77.43.36.81 1.08.81 2.18 0 1.58-.01 2.85-.01 3.24 0 .31.21.68.83.56C20.57 21.88 24 17.49 24 12.29 24 5.78 18.63.5 12 .5z" {}
        }
    }
}

/// `current` is the active page's path, which is highlighted.
pub fn site_nav(current: &str) -> Markup {
    let pages = [
        ("Home", "/"),
        ("Getting started", "/getting-started"),
        ("CLI reference", "/cli"),
        ("Model swap", "/model-swap"),
        ("Troubleshooting", "/troubleshooting"),
    ];
    html! {
        nav class="ffmenu" aria-label="Pages" {
            @for (label, href) in pages {
                @let active = href == current;
                @let cls = if active { "ffmenu__item text-cyan" } else { "ffmenu__item" };
                a class=(cls) href=(href) aria-current=[active.then_some("page")] { (label) }
            }
        }
    }
}
