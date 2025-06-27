use axum::{http, response::IntoResponse};
use base64::prelude::BASE64_STANDARD;
use base64::prelude::*;

pub use rust_embed::Embed;

#[derive(Embed)]
#[folder = "dist"]
#[exclude = ".vite"]
pub struct Dist;

#[derive(Embed)]
#[folder = "dist/.vite"]
pub struct DistVite;

impl DistVite {
    pub fn get_manifest() -> vite_manifest::Manifest {
        let data = Self::get("manifest.json").expect("Failed to get vite manifest");
        serde_json::from_slice(data.data.as_ref()).expect("Failed to parse vite manifest")
    }

    pub fn get_html_tags_for_asset<F: rust_embed::Embed>(path: &str) -> String {
        let manifest = Self::get_manifest();
        manifest
            .imported_chunks(path)
            .into_iter()
            .chain([manifest.manifest().get(path).expect("Asset to exist").to_owned()])
            .map(|chunk| {
                let path = chunk.file;
                let file = F::get(&path).expect("Failed to get asset from embed");
                let hash = BASE64_STANDARD.encode(file.metadata.sha256_hash());
                match file.metadata.mimetype() {
                    "application/javascript" if path.ends_with(".mjs") => {
                        format!(
                            "<script type=\"module\" src=\"/{path}\" integrity=\"sha256-{hash}\"></script>"
                        )
                    }
                    "application/javascript" => {
                        format!(
                            "<script src=\"/{path}\" integrity=\"sha256-{hash}\"></script>"
                        )
                    }
                    "text/css" => {
                        format!(
                            "<link rel=\"stylesheet\" href=\"/{path}\" integrity=\"sha256-{hash}\">"
                        )
                    }
                    _ => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub async fn static_service<F: rust_embed::Embed>(uri: http::Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    match F::get(path) {
        Some(content) => {
            let mime = content.metadata.mimetype();
            ([(http::header::CONTENT_TYPE, mime)], content.data).into_response()
        }
        None => handle_404().await.into_response(),
    }
}

pub async fn handle_404() -> (http::StatusCode, &'static str) {
    (http::StatusCode::NOT_FOUND, "Not found")
}
