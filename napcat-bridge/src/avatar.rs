use std::{collections::HashMap, fs, path::{Path, PathBuf}, sync::{Arc, OnceLock}, thread};

use ab_glyph::FontArc;
use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping};
use image::{codecs::jpeg::JpegEncoder, imageops, DynamicImage, Rgba, RgbaImage};
use imageproc::drawing::draw_text_mut;
use imageproc::geometric_transformations::{rotate_about_center, Border, Interpolation};
use serde::Deserialize;
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

pub const MASTER_SIZE: u32 = 1280;
pub const DEFAULT_OUTPUT_SIZE: u32 = 320;
pub const DEFAULT_QUALITY: u8 = 50;
const AVATAR_MANIFEST: &str = "/opt/pulsebridge/assets/heart-rate/avatar.json";

#[derive(Clone, Deserialize)]
struct AvatarManifest {
    background: PathBuf,
    region: TextRegion,
    arc: ArcConfig,
    font: PathBuf,
    font_size: f32,
    effects: EffectsConfig,
}

#[derive(Clone, Deserialize)]
struct TextRegion {
    cx: f32,
    cy: f32,
    width: f32,
    height: f32,
    rotation: f32,
}

impl TextRegion {
    fn scaled(&self, scale: f32) -> Self {
        Self {
            cx: self.cx * scale,
            cy: self.cy * scale,
            width: self.width * scale,
            height: self.height * scale,
            rotation: self.rotation,
        }
    }
}

#[derive(Clone, Deserialize)]
struct ArcConfig {
    curvature: f32,
    x_scale: f32,
}

#[derive(Clone, Deserialize)]
struct EffectsConfig {
    fill: String,
    highlight: String,
    outline: OutlineConfig,
    glow: GlowConfig,
    inner_shadow: ShadowConfig,
}

#[derive(Clone, Deserialize)]
struct OutlineConfig {
    color: String,
    width: u32,
}

#[derive(Clone, Deserialize)]
struct GlowConfig {
    color: String,
    radius: u32,
}

#[derive(Clone, Deserialize)]
struct ShadowConfig {
    color: String,
    offset_x: i32,
    offset_y: i32,
    blur: u32,
}

#[derive(Clone)]
pub struct AvatarConfig {
    pub background: PathBuf,
    pub font: PathBuf,
    pub output_size: u32,
    pub output: AvatarOutput,
    font_size: f32,
    region: TextRegion,
    arc: ArcConfig,
    style: AvatarStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarOutput {
    Quality(u8),
    MaxBytes(usize),
}

struct RenderRequest {
    kind: AvatarKind,
    reply: oneshot::Sender<Result<Arc<Vec<u8>>, String>>,
}

enum AvatarKind {
    Bpm(u16),
    NoData,
    Offline,
}

#[derive(Clone)]
pub struct AvatarRenderer {
    tx: mpsc::Sender<RenderRequest>,
}

impl AvatarRenderer {
    pub async fn start(config: AvatarConfig) -> Result<Self, String> {
        let (tx, mut rx) = mpsc::channel::<RenderRequest>(4);
        let (ready_tx, ready_rx) = oneshot::channel();
        thread::Builder::new()
            .name("pulsebridge-avatar-renderer".into())
            .spawn(move || {
                let mut renderer = match Renderer::new(config) {
                    Ok(renderer) => renderer,
                    Err(error) => {
                        warn!(%error, "avatar renderer failed to initialize");
                        let _ = ready_tx.send(Err(error.clone()));
                        while let Some(request) = rx.blocking_recv() {
                            let _ = request.reply.send(Err(error.clone()));
                        }
                        return;
                    }
                };
                info!("avatar renderer ready; rendering BPM values on demand");
                let _ = ready_tx.send(Ok(()));
                while let Some(request) = rx.blocking_recv() {
                    let result = match request.kind {
                        AvatarKind::Bpm(bpm) => renderer.render(bpm),
                        AvatarKind::NoData => renderer.render_no_data(),
                        AvatarKind::Offline => renderer.render_offline(),
                    };
                    let _ = request.reply.send(result);
                }
            })
            .map_err(|error| format!("failed to start avatar renderer thread: {error}"))?;
        ready_rx.await.map_err(|_| "avatar renderer stopped during startup".to_string())??;
        Ok(Self { tx })
    }

    pub async fn render(&self, bpm: u8) -> Result<Arc<Vec<u8>>, String> {
        self.request(AvatarKind::Bpm(u16::from(bpm))).await
    }

    pub async fn render_no_data(&self) -> Result<Arc<Vec<u8>>, String> {
        self.request(AvatarKind::NoData).await
    }

    pub async fn render_offline(&self) -> Result<Arc<Vec<u8>>, String> {
        self.request(AvatarKind::Offline).await
    }

    async fn request(&self, kind: AvatarKind) -> Result<Arc<Vec<u8>>, String> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(RenderRequest { kind, reply })
            .await
            .map_err(|_| "avatar renderer stopped".to_string())?;
        response.await.map_err(|_| "avatar renderer stopped".to_string())?
    }
}

/// Synchronous renderer used by the local preview CLI. It owns one immutable
/// 1280x1280 master background; every call clones that master before drawing.
pub struct AvatarGenerator {
    renderer: Renderer,
}

impl AvatarGenerator {
    pub fn config_from_manifest(path: &Path, output_size: u32, output: AvatarOutput) -> Result<AvatarConfig, String> {
        load_config(path, output_size, output)
    }

    pub fn new(config: AvatarConfig) -> Result<Self, String> {
        Ok(Self { renderer: Renderer::new(config)? })
    }

    pub fn render(&mut self, bpm: u16) -> Result<Vec<u8>, String> {
        self.renderer.render_fresh(bpm)
    }
}

struct Renderer {
    background: RgbaImage,
    font: FontArc,
    cache: HashMap<u16, Arc<Vec<u8>>>,
    no_data: Option<Arc<Vec<u8>>>,
    offline: Option<Arc<Vec<u8>>>,
    region: TextRegion,
    arc: ArcConfig,
    style: AvatarStyle,
    font_size: f32,
    output_size: u32,
    output: AvatarOutput,
}

impl Renderer {
    fn new(config: AvatarConfig) -> Result<Self, String> {
        let source_background = image::open(&config.background)
            .map_err(|error| format!("cannot open {}: {error}", config.background.display()))?
            .to_rgba8();
        if source_background.width() != source_background.height() {
            return Err(format!(
                "background must be square, got {}x{}",
                source_background.width(),
                source_background.height()
            ));
        }
        let coordinate_scale = MASTER_SIZE as f32 / source_background.width() as f32;
        let background = resize_linear(&source_background, MASTER_SIZE, MASTER_SIZE);
        let font_bytes = fs::read(&config.font)
            .map_err(|error| format!("cannot read {}: {error}", config.font.display()))?;
        let font = FontArc::try_from_vec(font_bytes.clone())
            .map_err(|_| format!("cannot parse font {}", config.font.display()))?;

        // fontdue performs a fast rasterizer sanity check. Actual compositing is
        // done with imageproc/ab_glyph so strokes and shadows remain controllable.
        let _ = fontdue::Font::from_bytes(font_bytes, fontdue::FontSettings::default())
            .map_err(|error| format!("cannot initialize fontdue: {error}"))?
            .metrics('8', MASTER_SIZE as f32 * 0.22);

        // cosmic-text initializes font discovery/fallback and shapes the fixed
        // three-column placeholder. This keeps the renderer ready for CJK/Emoji
        // labels without putting layout work on the Tokio executor.
        let mut font_system = FontSystem::new();
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(MASTER_SIZE as f32 * 0.22, MASTER_SIZE as f32 * 0.26));
        let mut buffer = buffer.borrow_with(&mut font_system);
        buffer.set_size(Some(MASTER_SIZE as f32), Some(MASTER_SIZE as f32));
        buffer.set_text("000", Attrs::new(), Shaping::Advanced);
        buffer.shape_until_scroll(true);

        Ok(Self {
            background,
            font,
            cache: HashMap::new(),
            no_data: None,
            offline: None,
            region: config.region.scaled(coordinate_scale),
            arc: config.arc,
            style: config.style,
            font_size: config.font_size * coordinate_scale,
            output_size: config.output_size,
            output: config.output,
        })
    }

    fn render(&mut self, bpm: u16) -> Result<Arc<Vec<u8>>, String> {
        if let Some(cached) = self.cache.get(&bpm) {
            return Ok(cached.clone());
        }
        let bytes = Arc::new(self.render_fresh(bpm)?);
        self.cache.insert(bpm, bytes.clone());
        Ok(bytes)
    }

    fn render_fresh(&self, bpm: u16) -> Result<Vec<u8>, String> {
        self.render_label(&display_bpm(bpm)).map(|bytes| (*bytes).clone())
    }

    fn render_no_data(&mut self) -> Result<Arc<Vec<u8>>, String> {
        if let Some(cached) = &self.no_data {
            return Ok(cached.clone());
        }
        let bytes = self.render_label("--")?;
        self.no_data = Some(bytes.clone());
        Ok(bytes)
    }

    fn render_offline(&mut self) -> Result<Arc<Vec<u8>>, String> {
        if let Some(cached) = &self.offline {
            return Ok(cached.clone());
        }
        let bytes = self.render_label("OFF")?;
        self.offline = Some(bytes.clone());
        Ok(bytes)
    }

    fn render_label(&self, text: &str) -> Result<Arc<Vec<u8>>, String> {
        let mut image = self.background.clone();
        let region_width = self.region.width.round().max(1.0) as u32;
        let region_height = self.region.height.round().max(1.0) as u32;
        let font_size = self.font_size;
        let font = self.font.clone();
        let scale = font_size;
        let characters: Vec<String> = text.chars().map(|ch| ch.to_string()).collect();
        let advances: Vec<f32> = characters
            .iter()
            .map(|character| {
                let (width, _) = imageproc::drawing::text_size(scale, &font, character);
                width as f32 * self.arc.x_scale
            })
            .collect();
        let total_width: f32 = advances.iter().sum();
        let mut cursor = (region_width as f32 - total_width) / 2.0;
        let mut layer = RgbaImage::new(region_width, region_height);

        for (character, advance) in characters.iter().zip(advances.iter()) {
            let center_x = cursor + advance / 2.0;
            let normalized_x = if total_width > 0.0 {
                ((center_x - total_width / 2.0) / (total_width / 2.0)).clamp(-1.0, 1.0)
            } else {
                0.0
            };
            let center_y = region_height as f32 / 2.0
                - self.arc.curvature * region_height as f32 * (1.0 - normalized_x * normalized_x);
            let angle = if total_width > 0.0 {
                (4.0 * self.arc.curvature * region_height as f32 * normalized_x / total_width).atan()
            } else {
                0.0
            };
            let glyph = self.render_glyph(character, region_height, scale, &font)?;
            let glyph_width = (glyph.width() as f32 * self.arc.x_scale).round().max(1.0) as u32;
            let glyph = if glyph_width == glyph.width() {
                glyph
            } else {
                imageops::resize(&glyph, glyph_width, glyph.height(), imageops::FilterType::Lanczos3)
            };
            let glyph = if angle.abs() > f32::EPSILON {
                rotate_about_center(
                    &glyph,
                    angle,
                    Interpolation::Bilinear,
                    Border::Constant(Rgba([0, 0, 0, 0])),
                )
            } else {
                glyph
            };
            let x = center_x.round() as i64 - i64::from(glyph.width()) / 2;
            let y = center_y.round() as i64 - i64::from(glyph.height()) / 2;
            imageops::overlay(&mut layer, &glyph, x, y);
            cursor += advance;
        }

        let layer = if self.region.rotation.abs() > f32::EPSILON {
            rotate_about_center(
                &layer,
                self.region.rotation.to_radians(),
                Interpolation::Bilinear,
                Border::Constant(Rgba([0, 0, 0, 0])),
            )
        } else {
            layer
        };
        let x = self.region.cx.round() as i64 - i64::from(layer.width()) / 2;
        let y = self.region.cy.round() as i64 - i64::from(layer.height()) / 2;
        imageops::overlay(&mut image, &layer, x, y);

        // Do not resize an already-compressed image. The master is cloned at
        // the beginning of this function, drawn at 1280x1280, and resized
        // exactly once immediately before the final JPEG encode.
        let image = resize_linear(&image, self.output_size, self.output_size);
        let bytes = Arc::new(encode_output(&image, self.output)?);
        Ok(bytes)
    }

    fn render_glyph(
        &self,
        text: &str,
        region_height: u32,
        scale: f32,
        font: &FontArc,
    ) -> Result<RgbaImage, String> {
        let (text_width, text_height) = imageproc::drawing::text_size(scale, font, text);
        let padding = self.style.outline_width
            .saturating_add(self.style.glow.radius)
            .saturating_add(4);
        let width = text_width.saturating_add(padding.saturating_mul(2)).max(1);
        let x = padding as i32;
        let y = ((region_height.saturating_sub(text_height)) / 2) as i32;
        let mut layer = RgbaImage::new(width, region_height.max(1));

        let mut glow = RgbaImage::new(width, region_height.max(1));
        draw_text_mut(&mut glow, self.style.glow.color, x, y, scale, font, text);
        if self.style.glow.radius > 0 {
            glow = imageops::blur(&glow, self.style.glow.radius as f32);
        }
        imageops::overlay(&mut layer, &glow, 0, 0);

        let outline = self.style.outline_width as i32;
        for dx in -outline..=outline {
            for dy in -outline..=outline {
                if dx * dx + dy * dy <= outline * outline {
                    draw_text_mut(&mut layer, self.style.outline, x + dx, y + dy, scale, font, text);
                }
            }
        }
        draw_text_mut(&mut layer, self.style.fill, x, y, scale, font, text);

        let mut mask = RgbaImage::new(width, region_height.max(1));
        draw_text_mut(&mut mask, Rgba([255, 255, 255, 255]), x, y, scale, font, text);

        let shadow = &self.style.inner_shadow;
        let mut inner_shadow = RgbaImage::new(width, region_height.max(1));
        draw_text_mut(
            &mut inner_shadow,
            shadow.color,
            x + shadow.offset_x,
            y + shadow.offset_y,
            scale,
            font,
            text,
        );
        if shadow.blur > 0 {
            inner_shadow = imageops::blur(&inner_shadow, shadow.blur as f32);
        }
        for (shadow_pixel, mask_pixel) in inner_shadow.pixels_mut().zip(mask.pixels()) {
            shadow_pixel.0[3] = shadow_pixel.0[3].min(mask_pixel.0[3]);
        }
        imageops::overlay(&mut layer, &inner_shadow, 0, 0);

        let mut highlight = RgbaImage::new(width, region_height.max(1));
        draw_text_mut(&mut highlight, self.style.highlight, x - 1, y - 1, scale, font, text);
        for (highlight_pixel, mask_pixel) in highlight.pixels_mut().zip(mask.pixels()) {
            highlight_pixel.0[3] = highlight_pixel.0[3].min(mask_pixel.0[3]);
        }
        imageops::overlay(&mut layer, &highlight, 0, 0);
        Ok(layer)
    }
}

fn display_bpm(bpm: u16) -> String {
    format!("{bpm:>3}")
}

#[derive(Clone)]
struct AvatarStyle {
    fill: Rgba<u8>,
    highlight: Rgba<u8>,
    outline: Rgba<u8>,
    outline_width: u32,
    glow: GlowStyle,
    inner_shadow: ShadowStyle,
}

#[derive(Clone)]
struct ShadowStyle {
    color: Rgba<u8>,
    offset_x: i32,
    offset_y: i32,
    blur: u32,
}

#[derive(Clone)]
struct GlowStyle {
    color: Rgba<u8>,
    radius: u32,
}

fn parse_color(value: &str) -> Result<Rgba<u8>, String> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() != 6 && hex.len() != 8 {
        return Err(format!("invalid color {value}; expected #RRGGBB or #RRGGBBAA"));
    }
    let component = |start| u8::from_str_radix(&hex[start..start + 2], 16)
        .map_err(|_| format!("invalid color {value}"));
    Ok(Rgba([
        component(0)?,
        component(2)?,
        component(4)?,
        if hex.len() == 8 { component(6)? } else { 255 },
    ]))
}

fn srgb_to_linear(value: u8) -> f32 {
    static TABLE: OnceLock<[f32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0.0; 256];
        for (index, item) in table.iter_mut().enumerate() {
            let value = index as f32 / 255.0;
            *item = if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            };
        }
        table
    })[usize::from(value)]
}

fn linear_to_srgb(value: f32) -> u8 {
    static TABLE: OnceLock<[u8; 4097]> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0; 4097];
        for (index, item) in table.iter_mut().enumerate() {
            let value = index as f32 / 4096.0;
            let srgb = if value <= 0.0031308 {
                value * 12.92
            } else {
                1.055 * value.powf(1.0 / 2.4) - 0.055
            };
            *item = (srgb.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
        table
    });
    table[(value.clamp(0.0, 1.0) * 4096.0 + 0.5) as usize]
}

/// Resize in linear-light RGB. `image::imageops::resize` interpolates channel
/// values as stored in sRGB, which makes downsampled dark artwork too dark.
/// Area averaging is used for downsampling: every destination pixel receives
/// the weighted average of all source pixels it covers. This is less harsh on
/// fine line art than center-sampled bilinear filtering. Bilinear sampling is
/// retained for upscaling and mixed-dimension fallback cases.
fn resize_linear(image: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    if image.width() == width && image.height() == height {
        return image.clone();
    }
    let source_width = image.width() as usize;
    let source_height = image.height() as usize;
    let linear: Vec<[f32; 4]> = image
        .pixels()
        .map(|pixel| [
            srgb_to_linear(pixel[0]),
            srgb_to_linear(pixel[1]),
            srgb_to_linear(pixel[2]),
            f32::from(pixel[3]) / 255.0,
        ])
        .collect();
    if width < image.width() && height < image.height() {
        let x_weights = area_weights(source_width, width as usize);
        let y_weights = area_weights(source_height, height as usize);
        return RgbaImage::from_fn(width, height, |x, y| {
            let mut pixel = [0.0; 4];
            let mut total_weight = 0.0;
            for &(source_y, y_weight) in &y_weights[y as usize] {
                for &(source_x, x_weight) in &x_weights[x as usize] {
                    let weight = x_weight * y_weight;
                    let source = linear[source_y * source_width + source_x];
                    for channel in 0..4 {
                        pixel[channel] += source[channel] * weight;
                    }
                    total_weight += weight;
                }
            }
            let normalizer = total_weight.recip();
            for channel in &mut pixel {
                *channel *= normalizer;
            }
            Rgba([
                linear_to_srgb(pixel[0]),
                linear_to_srgb(pixel[1]),
                linear_to_srgb(pixel[2]),
                (pixel[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
            ])
        });
    }
    let sample = |x: usize, y: usize| linear[y * source_width + x];
    RgbaImage::from_fn(width, height, |x, y| {
        let source_x = ((x as f32 + 0.5) * source_width as f32 / width as f32 - 0.5).max(0.0);
        let source_y = ((y as f32 + 0.5) * source_height as f32 / height as f32 - 0.5).max(0.0);
        let x0 = (source_x.floor() as usize).min(source_width - 1);
        let y0 = (source_y.floor() as usize).min(source_height - 1);
        let x1 = (x0 + 1).min(source_width - 1);
        let y1 = (y0 + 1).min(source_height - 1);
        let x_weight = source_x - x0 as f32;
        let y_weight = source_y - y0 as f32;
        let top_left = sample(x0, y0);
        let top_right = sample(x1, y0);
        let bottom_left = sample(x0, y1);
        let bottom_right = sample(x1, y1);
        let mut pixel = [0.0; 4];
        for channel in 0..4 {
            let top = top_left[channel] + (top_right[channel] - top_left[channel]) * x_weight;
            let bottom = bottom_left[channel] + (bottom_right[channel] - bottom_left[channel]) * x_weight;
            pixel[channel] = top + (bottom - top) * y_weight;
        }
        Rgba([
            linear_to_srgb(pixel[0]),
            linear_to_srgb(pixel[1]),
            linear_to_srgb(pixel[2]),
            (pixel[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        ])
    })
}

fn area_weights(source_size: usize, target_size: usize) -> Vec<Vec<(usize, f32)>> {
    (0..target_size)
        .map(|target| {
            let start = target as f32 * source_size as f32 / target_size as f32;
            let end = (target + 1) as f32 * source_size as f32 / target_size as f32;
            let first = start.floor() as usize;
            let last = (end.ceil() as usize).min(source_size);
            (first..last)
                .map(|source| {
                    let overlap = (end.min((source + 1) as f32) - start.max(source as f32)).max(0.0);
                    (source, overlap)
                })
                .filter(|(_, weight)| *weight > 0.0)
                .collect()
        })
        .collect()
}

fn encode_jpeg(image: &RgbaImage, quality: u8) -> Result<Vec<u8>, String> {
    let rgb = image::RgbImage::from_fn(image.width(), image.height(), |x, y| {
        let pixel = image.get_pixel(x, y);
        image::Rgb([pixel[0], pixel[1], pixel[2]])
    });
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality)
        .encode_image(&DynamicImage::ImageRgb8(rgb))
        .map_err(|error| format!("JPEG encode failed: {error}"))?;
    Ok(bytes)
}

fn encode_output(image: &RgbaImage, output: AvatarOutput) -> Result<Vec<u8>, String> {
    match output {
        AvatarOutput::Quality(quality) => encode_jpeg(image, quality),
        AvatarOutput::MaxBytes(limit) => {
            let mut low = 1i32;
            let mut high = 100i32;
            let mut best = None;
            while low <= high {
                let quality = ((low + high) / 2) as u8;
                let bytes = encode_jpeg(image, quality)?;
                if bytes.len() <= limit {
                    best = Some(bytes);
                    low = i32::from(quality) + 1;
                } else {
                    high = i32::from(quality) - 1;
                }
            }
            best.ok_or_else(|| format!("cannot encode JPEG within {limit} bytes, even at quality 1"))
        }
    }
}

pub fn default_config() -> Result<AvatarConfig, String> {
    let manifest_path = Path::new(AVATAR_MANIFEST);
    let quality = std::env::var("PB_AVATAR_JPEG_QUALITY")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_QUALITY);
    let output_size = std::env::var("PB_AVATAR_SIZE")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_OUTPUT_SIZE);
    let output = match std::env::var("PB_AVATAR_MAX_BYTES") {
        Ok(value) => AvatarOutput::MaxBytes(parse_byte_limit(&value)?),
        Err(_) => AvatarOutput::Quality(quality),
    };
    load_config(manifest_path, output_size, output)
}

fn parse_byte_limit(value: &str) -> Result<usize, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("PB_AVATAR_MAX_BYTES cannot be empty".into());
    }
    let split_at = value
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(value.len());
    let (number, suffix) = value.split_at(split_at);
    let number: usize = number
        .parse()
        .map_err(|_| format!("invalid PB_AVATAR_MAX_BYTES: {value}"))?;
    let multiplier = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" => 1000,
        "ki" | "kib" => 1024,
        "m" | "mb" => 1000 * 1000,
        "mi" | "mib" => 1024 * 1024,
        _ => return Err(format!("invalid PB_AVATAR_MAX_BYTES suffix in {value}")),
    };
    let bytes = number
        .checked_mul(multiplier)
        .ok_or_else(|| format!("PB_AVATAR_MAX_BYTES is too large: {value}"))?;
    if bytes == 0 {
        return Err("PB_AVATAR_MAX_BYTES must be greater than zero".into());
    }
    Ok(bytes)
}

fn load_config(path: &Path, output_size: u32, output: AvatarOutput) -> Result<AvatarConfig, String> {
    if output_size == 0 || output_size > MASTER_SIZE {
        return Err(format!("output size must be between 1 and {MASTER_SIZE}"));
    }
    if let AvatarOutput::Quality(quality) = output {
        if !(1..=100).contains(&quality) {
            return Err("JPEG quality must be between 1 and 100".into());
        }
    }
    let manifest_path = path;
    let manifest: AvatarManifest = serde_json::from_slice(
        &fs::read(manifest_path).map_err(|error| format!("cannot read {}: {error}", manifest_path.display()))?,
    ).map_err(|error| format!("cannot parse {}: {error}", manifest_path.display()))?;
    if manifest.region.width <= 0.0 || manifest.region.height <= 0.0 {
        return Err("avatar region width and height must be positive".into());
    }
    if manifest.font_size <= 0.0 {
        return Err("avatar font_size must be positive".into());
    }
    if manifest.arc.x_scale <= 0.0 {
        return Err("avatar arc x_scale must be positive".into());
    }
    let base = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let background = if manifest.background.is_absolute() {
        manifest.background
    } else {
        base.join(manifest.background)
    };
    let font = if manifest.font.is_absolute() {
        manifest.font
    } else {
        base.join(manifest.font)
    };
    let style = AvatarStyle {
        fill: parse_color(&manifest.effects.fill)?,
        highlight: parse_color(&manifest.effects.highlight)?,
        outline: parse_color(&manifest.effects.outline.color)?,
        outline_width: manifest.effects.outline.width,
        glow: GlowStyle {
            color: parse_color(&manifest.effects.glow.color)?,
            radius: manifest.effects.glow.radius,
        },
        inner_shadow: ShadowStyle {
            color: parse_color(&manifest.effects.inner_shadow.color)?,
            offset_x: manifest.effects.inner_shadow.offset_x,
            offset_y: manifest.effects.inner_shadow.offset_y,
            blur: manifest.effects.inner_shadow.blur,
        },
    };
    Ok(AvatarConfig {
        background,
        font,
        output_size,
        output,
        font_size: manifest.font_size,
        region: manifest.region,
        arc: manifest.arc,
        style,
    })
}

#[cfg(test)]
mod tests {
    use super::display_bpm;

    #[test]
    fn hides_only_leading_zeroes() {
        assert_eq!(display_bpm(7), "  7");
        assert_eq!(display_bpm(70), " 70");
        assert_eq!(display_bpm(100), "100");
        assert_eq!(display_bpm(105), "105");
    }
}
