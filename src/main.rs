use std::{io::Cursor, net::SocketAddr};

use anyhow::{Context, Result};
use axum::{
    extract::{Path, Query, Request},
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use axum_response_cache::CacheLayer;
use clap::Parser;
use html_escape::decode_html_entities;
use image::{DynamicImage, GenericImageView as _, ImageBuffer, Rgba};
use regex::Regex;
use reqwest::{header, Method};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use url::Url;

#[derive(Clone, Parser)]
struct Args {
    #[clap(short, long, default_value = "[::]:3319")]
    listen: SocketAddr,
    #[clap(short, long, default_value = "https://zz.nekok500.com")]
    base_url: Url,
}

struct AppError(anyhow::Error);
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Something went wrong: {}", self.0),
        )
            .into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

#[derive(Serialize, Clone)]
struct Metadata {
    owner_name: String,
}

async fn event(Path(event_id): Path<usize>) -> Result<Json<Metadata>, AppError> {
    let first = reqwest::get(format!("https://zaiko.io/event/{}", event_id))
        .await?
        .error_for_status()?
        .text()
        .await?;
    let second_url = Regex::new(";url='(.+)'\" />")
        .unwrap()
        .captures(&first)
        .context("first: no context")?
        .get(1)
        .unwrap()
        .as_str();

    let second = reqwest::get(second_url)
        .await?
        .error_for_status()?
        .text()
        .await?;

    let site_name = decode_html_entities(
        Regex::new(r#"<meta property="og:site_name" content="(.+)" />"#)
            .unwrap()
            .captures(&second)
            .context("second: og:site_name no context")?
            .get(1)
            .unwrap()
            .as_str(),
    );
    Ok(Json(Metadata {
        owner_name: site_name.to_string(),
    }))
}

#[derive(Deserialize)]
struct SquareParams {
    u: String,
}

async fn square(query: Query<SquareParams>) -> Result<Response, AppError> {
    if !query.u.starts_with("https://media.zaiko.io/") {
        return Ok((StatusCode::BAD_REQUEST, "url not allowed").into_response());
    }

    let image_bytes = reqwest::get(query.u.clone())
        .await?
        .error_for_status()?
        .bytes()
        .await?
        .to_vec();

    let img = image::load_from_memory(&image_bytes)?;
    let resized_img = resize_image(&img, 400, 400);

    let mut buffer = Cursor::new(Vec::new());
    resized_img.write_to(&mut buffer, image::ImageFormat::Png)?;

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "image/png")
        .body(buffer.into_inner().into())
        .unwrap())
}

fn resize_image(img: &DynamicImage, width: u32, height: u32) -> DynamicImage {
    let resized_img = img.resize(width, height, image::imageops::FilterType::Lanczos3);

    let mut output_img = ImageBuffer::from_pixel(width, height, Rgba([0, 0, 0, 0]));
    let (new_width, new_height) = resized_img.dimensions();
    let x_offset = (width - new_width) / 2;
    let y_offset = (height - new_height) / 2;

    image::imageops::overlay(
        &mut output_img,
        &resized_img,
        x_offset.into(),
        y_offset.into(),
    );
    DynamicImage::ImageRgba8(output_img)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::try_parse()?;

    let app = Router::new()
        .route("/zaiko/events/:event_id", get(event))
        .route("/square.png", get(square))
        .layer(middleware::from_fn(set_static_cache_control))
        .layer(CacheLayer::with_lifespan(3600))
        .layer(
            CorsLayer::new()
                .allow_methods([Method::GET])
                .allow_origin("https://zaiko.io".parse::<HeaderValue>().unwrap()),
        );
    let listener = TcpListener::bind(args.listen).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn set_static_cache_control(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let seconds = match response.status() {
        StatusCode::OK => 3600,
        _ => 300,
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_str(&format!("public, max-age={seconds}")).unwrap(),
    );
    response
}
