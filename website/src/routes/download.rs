use maud::{Markup, PreEscaped, html};
use maudit::route::prelude::*;

const REPO: &str = "https://github.com/Princesseuh/chronostasis";

#[derive(Clone, Copy)]
struct Product {
    slug: &'static str,
    crate_name: &'static str,
    version: &'static str,
}

const PLATFORMS: [(&str, &str, &str); 3] = [
    ("windows", "Windows", "windows-x86_64.zip"),
    ("linux", "Linux", "linux-x86_64.tar.gz"),
    ("macos", "macOS", "macos-aarch64.tar.gz"),
];

// Asset URLs embed this, so deploy-website.yml has to redeploy on a manifest bump.
fn package_version(manifest: &'static str) -> &'static str {
    manifest
        .lines()
        .find_map(|line| line.strip_prefix("version = \""))
        .and_then(|value| value.split('"').next())
        .expect("crate manifest opens with a literal package version")
}

fn gui() -> Product {
    Product {
        slug: "gui",
        crate_name: "chronostasis-gui",
        version: package_version(include_str!("../../../crates/chronostasis-gui/Cargo.toml")),
    }
}

fn cli() -> Product {
    Product {
        slug: "cli",
        crate_name: "chronostasis-cli",
        version: package_version(include_str!("../../../crates/chronostasis-cli/Cargo.toml")),
    }
}

// The proxy DLL is built from an unpublished crate, so it rides in the CLI's release.
fn proxy_dll() -> (String, String) {
    let Product {
        crate_name,
        version,
        ..
    } = cli();
    (
        "dll".to_string(),
        format!("{REPO}/releases/download/{crate_name}-v{version}/d3d9-{version}.dll"),
    )
}

fn downloads() -> Vec<(String, String)> {
    [gui(), cli()]
        .into_iter()
        .flat_map(|product| {
            PLATFORMS.into_iter().flat_map(move |(platform, _, asset)| {
                let url = format!(
                    "{REPO}/releases/download/{name}-v{version}/{name}-{version}-{asset}",
                    name = product.crate_name,
                    version = product.version,
                );
                [
                    (format!("{}-{}", product.slug, platform), url.clone()),
                    (format!("{}-{}", platform, product.slug), url),
                ]
            })
        })
        .chain([proxy_dll()])
        .collect()
}

// All four render so the block still works without JS; the script demotes the rest.
pub fn download_buttons(product: &str, name: &str) -> Markup {
    html! {
        div data-downloads=(name) {
            div class="flex flex-wrap gap-2" {
                @for (platform, label, _) in PLATFORMS {
                    a class="dl-btn" data-platform=(platform) data-label=(label)
                      href={ "/download/" (product) "-" (platform) } { (label) }
                }
            }
            p class="mt-3 text-sm text-body" data-other-platforms hidden {}
        }
    }
}

const DETECT_SCRIPT: &str = r#"
(() => {
  const platform = navigator.userAgentData?.platform || navigator.platform || "";
  const haystack = (platform + " " + navigator.userAgent).toLowerCase();

  let detected = null;
  if (haystack.includes("win")) detected = "windows";
  else if (haystack.includes("mac")) detected = "macos";
  else if (haystack.includes("linux") || haystack.includes("x11")) detected = "linux";
  if (!detected) return;

  for (const block of document.querySelectorAll("[data-downloads]")) {
    const buttons = [...block.querySelectorAll("[data-platform]")];
    const mine = buttons.find((button) => button.dataset.platform === detected);
    const others = buttons.filter((button) => button !== mine);
    if (!mine) continue;

    // .dl-btn transitions its colours, so the swap would otherwise fade in on load.
    mine.style.transition = "none";
    mine.className = "dl-btn dl-btn--primary";
    mine.textContent = `Download ${block.dataset.downloads} for ${mine.dataset.label}`;
    requestAnimationFrame(() => (mine.style.transition = ""));

    const rest = block.querySelector("[data-other-platforms]");
    others.forEach((button, index) => {
      button.className = "dl-link";
      if (index) rest.append(", ");
      rest.append(button);
    });
    rest.hidden = false;
  }
})();
"#;

pub fn platform_detect_script() -> Markup {
    html! { script { (PreEscaped(DETECT_SCRIPT)) } }
}

#[route("/download/[target]")]
pub struct DownloadTarget;

#[derive(Params, Clone)]
pub struct DownloadTargetParams {
    pub target: String,
}

impl Route<DownloadTargetParams> for DownloadTarget {
    fn pages(&self, _ctx: &mut DynamicRouteContext) -> Pages<DownloadTargetParams> {
        downloads()
            .into_iter()
            .map(|(target, _)| Page::from_params(DownloadTargetParams { target }))
            .collect()
    }

    fn render(&self, ctx: &mut PageContext) -> impl Into<RenderResult> {
        let target = ctx.params::<DownloadTargetParams>().target;
        let (_, url) = downloads()
            .into_iter()
            .find(|(slug, _)| *slug == target)
            .expect("every target page comes from `downloads`");

        redirect(&url)
    }
}
