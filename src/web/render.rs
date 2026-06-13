//! Render an Askama template into an HTML response.
//!
//! Askama 0.13+ dropped the bundled axum integration, so we render to a string
//! ourselves and wrap it. Template errors surface as `500`s via [`AppError`].

use crate::error::AppError;
use askama::Template;
use axum::response::Html;

pub fn render<T: Template>(template: &T) -> Result<Html<String>, AppError> {
    Ok(Html(template.render()?))
}
