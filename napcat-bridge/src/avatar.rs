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
    #[serde(default)]
    zones: HashMap<String, AvatarOverride>,
}

#[derive(Clone, Deserialize)]
struct TextRegion {
    cx: f32,
    cy: f32,
    width: f32,
    height: f32,
    rotation: f32,
}

#[derive(Clone, Deserialize, Default)]
struct RegionOverride {
    cx: Option<f32>,
    cy: Option<f32>,
    width: Option<f32>,
    height: Option<f32>,
    rotation: Option<f32>,
}

#[derive(Clone, Deserialize, Default)]
struct ArcOverride {
    curvature: Option<f32>,
    x_scale: Option<f32>,
}

#[derive(Clone, Deserialize, Default)]
struct ColorEffectOverride {
    color: Option<String>,
    width: Option<u32>,
}

#[derive(Clone, Deserialize, Default)]
struct GlowOverride {
    color: Option<String>,
    radius: Option<u32>,
}

#[derive(Clone, Deserialize, Default)]
struct ShadowOverride {
    color: Option<String>,
    offset_x: Option<i32>,
    offset_y: Option<i32>,
    blur: Option<u32>,
}

#[derive(Clone, Deserialize, Default)]
struct EffectsOverride {
    fill: Option<String>,
    highlight: Option<String>,
    outline: Option<ColorEffectOverride>,
    glow: Option<GlowOverride>,
    inner_shadow: Option<ShadowOverride>,
}

#[derive(Clone, Deserialize, Default)]
struct AvatarOverride {
    background: Option<PathBuf>,
    region: Option<RegionOverride>,
    arc: Option<ArcOverride>,
    font: Option<PathBuf>,
    font_size: Option<f32>,
    effects: Option<EffectsOverride>,
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

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ZoneAlgorithm {
    MaxHr,
    LactateThreshold,
    Custom,
}

impl Default for ZoneAlgorithm {
    fn default() -> Self {
        Self::MaxHr
    }
}

pub fn parse_zone_algorithm(value: &str) -> Result<ZoneAlgorithm, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "max_hr" => Ok(ZoneAlgorithm::MaxHr),
        "lactate_threshold" => Ok(ZoneAlgorithm::LactateThreshold),
        "custom" => Ok(ZoneAlgorithm::Custom),
        _ => Err(format!(
            "invalid zone algorithm {value}; use max_hr, lactate_threshold, or custom"
        )),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct ZoneRange {
    pub min: u16,
    pub max: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ZoneId {
    Z1,
    Z2,
    Z3,
    Z4,
    Z5,
    OutOfRange,
}

impl ZoneId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Z1 => "Z1",
            Self::Z2 => "Z2",
            Self::Z3 => "Z3",
            Self::Z4 => "Z4",
            Self::Z5 => "Z5",
            Self::OutOfRange => "--",
        }
    }

    fn index(self) -> Option<usize> {
        match self {
            Self::Z1 => Some(0),
            Self::Z2 => Some(1),
            Self::Z3 => Some(2),
            Self::Z4 => Some(3),
            Self::Z5 => Some(4),
            Self::OutOfRange => None,
        }
    }

    fn all() -> [Self; 5] {
        [Self::Z1, Self::Z2, Self::Z3, Self::Z4, Self::Z5]
    }
}

#[derive(Clone, Debug)]
pub struct ZoneScheme {
    pub algorithm: ZoneAlgorithm,
    pub max_hr: u16,
    pub lactate_threshold: Option<u16>,
    ranges: [ZoneRange; 5],
}

impl ZoneScheme {
    pub fn max_hr(max_hr: u16) -> Self {
        let max_hr = u32::from(max_hr);
        let boundary = |percent: u32| (max_hr * percent / 100).min(u32::from(u16::MAX)) as u16;
        let z1_max = boundary(60);
        let z2_max = boundary(70);
        let z3_max = boundary(80);
        let z4_max = boundary(90);
        Self {
            algorithm: ZoneAlgorithm::MaxHr,
            max_hr: max_hr as u16,
            lactate_threshold: None,
            ranges: [
                ZoneRange { min: boundary(50), max: z1_max },
                ZoneRange { min: z1_max.saturating_add(1), max: z2_max },
                ZoneRange { min: z2_max.saturating_add(1), max: z3_max },
                ZoneRange { min: z3_max.saturating_add(1), max: z4_max },
                ZoneRange { min: z4_max.saturating_add(1), max: max_hr as u16 },
            ],
        }
    }

    pub fn from_runtime(
        algorithm: ZoneAlgorithm,
        max_hr: u16,
        lactate_threshold: Option<u16>,
        custom_ranges: Option<[ZoneRange; 5]>,
    ) -> Result<Self, String> {
        if !(1..=999).contains(&max_hr) {
            return Err("max_hr must be between 1 and 999".into());
        }
        match algorithm {
            ZoneAlgorithm::MaxHr => Ok(Self::max_hr(max_hr)),
            ZoneAlgorithm::LactateThreshold => {
                let threshold = lactate_threshold
                    .ok_or("lactate_threshold is required for lactate_threshold")?;
                if !(1..=999).contains(&threshold) {
                    return Err("lactate_threshold must be between 1 and 999".into());
                }
                if threshold >= max_hr {
                    return Err("lactate_threshold must be below max_hr".into());
                }
                let boundary = |percent: u32| (u32::from(threshold) * percent / 100) as u16;
                let z1_max = boundary(85);
                let z2_max = boundary(90);
                let z3_max = boundary(95);
                Ok(Self {
                    algorithm,
                    max_hr,
                    lactate_threshold: Some(threshold),
                    ranges: [
                        ZoneRange { min: 0, max: z1_max },
                        ZoneRange { min: z1_max.saturating_add(1), max: z2_max },
                        ZoneRange { min: z2_max.saturating_add(1), max: z3_max },
                        ZoneRange { min: z3_max.saturating_add(1), max: threshold },
                        ZoneRange { min: threshold.saturating_add(1), max: max_hr },
                    ],
                })
            }
            ZoneAlgorithm::Custom => {
                let ranges = custom_ranges.ok_or("custom_zones is required for custom")?;
                for (index, range) in ranges.iter().enumerate() {
                    if range.min > range.max || range.max > 999 {
                        return Err(format!("custom zone z{} is invalid", index + 1));
                    }
                }
                Ok(Self {
                    algorithm,
                    max_hr,
                    lactate_threshold,
                    ranges,
                })
            }
        }
    }

    pub fn zone_for(&self, bpm: u16) -> ZoneId {
        self.ranges
            .iter()
            .enumerate()
            .find(|(_, range)| bpm >= range.min && bpm <= range.max)
            .map(|(index, _)| ZoneId::all()[index])
            .unwrap_or(ZoneId::OutOfRange)
    }
}

pub fn parse_custom_zones(value: &str) -> Result<[ZoneRange; 5], String> {
    let values: Vec<ZoneRange> = value
        .split(',')
        .enumerate()
        .map(|(index, item)| {
            let (min, max) = item
                .trim()
                .split_once('-')
                .ok_or_else(|| format!("custom zone {} must use MIN-MAX", index + 1))?;
            let min = min
                .trim()
                .parse::<u16>()
                .map_err(|_| format!("invalid minimum for custom zone {}", index + 1))?;
            let max = max
                .trim()
                .parse::<u16>()
                .map_err(|_| format!("invalid maximum for custom zone {}", index + 1))?;
            Ok(ZoneRange { min, max })
        })
        .collect::<Result<_, String>>()?;
    values.try_into().map_err(|values: Vec<ZoneRange>| {
        format!(
            "custom-zones requires exactly five MIN-MAX ranges, got {}",
            values.len()
        )
    })
}

#[derive(Clone)]
pub struct AvatarConfig {
    pub output_size: u32,
    pub output: AvatarOutput,
    pub zone_scheme: ZoneScheme,
    base: ResolvedZoneConfig,
    zones: [ResolvedZoneConfig; 5],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarOutput {
    Quality(u8),
    MaxBytes(usize),
}

#[derive(Clone)]
struct ResolvedZoneConfig {
    background: PathBuf,
    font: PathBuf,
    font_size: f32,
    region: TextRegion,
    arc: ArcConfig,
    style: AvatarStyle,
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
        Self::config_from_manifest_with_zones(path, output_size, output, ZoneScheme::max_hr(200))
    }

    pub fn config_from_manifest_with_zones(
        path: &Path,
        output_size: u32,
        output: AvatarOutput,
        zone_scheme: ZoneScheme,
    ) -> Result<AvatarConfig, String> {
        load_config(path, output_size, output, zone_scheme)
    }

    pub fn new(config: AvatarConfig) -> Result<Self, String> {
        Ok(Self { renderer: Renderer::new(config)? })
    }

    pub fn render(&mut self, bpm: u16) -> Result<Vec<u8>, String> {
        self.renderer.render_fresh(bpm)
    }
}

struct Renderer {
    base: ZoneRenderer,
    zones: [ZoneRenderer; 5],
    zone_scheme: ZoneScheme,
    cache: HashMap<(ZoneId, u16), Arc<Vec<u8>>>,
    no_data: Option<Arc<Vec<u8>>>,
    offline: Option<Arc<Vec<u8>>>,
    output_size: u32,
    output: AvatarOutput,
}

struct ZoneRenderer {
    background: RgbaImage,
    font: FontArc,
    region: TextRegion,
    arc: ArcConfig,
    style: AvatarStyle,
    font_size: f32,
}

impl ZoneRenderer {
    fn new(config: ResolvedZoneConfig) -> Result<Self, String> {
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

        let _ = fontdue::Font::from_bytes(font_bytes, fontdue::FontSettings::default())
            .map_err(|error| format!("cannot initialize fontdue: {error}"))?
            .metrics('8', MASTER_SIZE as f32 * 0.22);

        let mut font_system = FontSystem::new();
        let mut buffer = Buffer::new(
            &mut font_system,
            Metrics::new(MASTER_SIZE as f32 * 0.22, MASTER_SIZE as f32 * 0.26),
        );
        let mut buffer = buffer.borrow_with(&mut font_system);
        buffer.set_size(Some(MASTER_SIZE as f32), Some(MASTER_SIZE as f32));
        buffer.set_text("000", Attrs::new(), Shaping::Advanced);
        buffer.shape_until_scroll(true);

        Ok(Self {
            background,
            font,
            region: config.region.scaled(coordinate_scale),
            arc: config.arc,
            style: config.style,
            font_size: config.font_size * coordinate_scale,
        })
    }
}

impl Renderer {
    fn new(config: AvatarConfig) -> Result<Self, String> {
        let mut zone_renderers = Vec::with_capacity(5);
        for zone in config.zones {
            zone_renderers.push(ZoneRenderer::new(zone)?);
        }
        let zones: [ZoneRenderer; 5] = zone_renderers
            .try_into()
            .map_err(|_| "avatar renderer expected five zone configurations".to_string())?;
        Ok(Self {
            base: ZoneRenderer::new(config.base)?,
            zones,
            zone_scheme: config.zone_scheme,
            cache: HashMap::new(),
            no_data: None,
            offline: None,
            output_size: config.output_size,
            output: config.output,
        })
    }

    fn render(&mut self, bpm: u16) -> Result<Arc<Vec<u8>>, String> {
        let zone = self.zone_scheme.zone_for(bpm);
        if let Some(cached) = self.cache.get(&(zone, bpm)) {
            return Ok(cached.clone());
        }
        let bytes = Arc::new(self.render_fresh(bpm)?);
        self.cache.insert((zone, bpm), bytes.clone());
        Ok(bytes)
    }

    fn render_fresh(&self, bpm: u16) -> Result<Vec<u8>, String> {
        self.render_label(self.zone_scheme.zone_for(bpm), &display_bpm(bpm))
            .map(|bytes| (*bytes).clone())
    }

    fn render_no_data(&mut self) -> Result<Arc<Vec<u8>>, String> {
        if let Some(cached) = &self.no_data {
            return Ok(cached.clone());
        }
        let bytes = self.render_label(ZoneId::OutOfRange, "--")?;
        self.no_data = Some(bytes.clone());
        Ok(bytes)
    }

    fn render_offline(&mut self) -> Result<Arc<Vec<u8>>, String> {
        if let Some(cached) = &self.offline {
            return Ok(cached.clone());
        }
        let bytes = self.render_label(ZoneId::OutOfRange, "OFF")?;
        self.offline = Some(bytes.clone());
        Ok(bytes)
    }

    fn render_label(&self, zone: ZoneId, text: &str) -> Result<Arc<Vec<u8>>, String> {
        let zone_renderer = zone
            .index()
            .map(|index| &self.zones[index])
            .unwrap_or(&self.base);
        let mut image = zone_renderer.background.clone();
        let region_width = zone_renderer.region.width.round().max(1.0) as u32;
        let region_height = zone_renderer.region.height.round().max(1.0) as u32;
        let font_size = zone_renderer.font_size;
        let font = zone_renderer.font.clone();
        let scale = font_size;
        let characters: Vec<String> = text.chars().map(|ch| ch.to_string()).collect();
        let advances: Vec<f32> = characters
            .iter()
            .map(|character| {
                let (width, _) = imageproc::drawing::text_size(scale, &font, character);
                width as f32 * zone_renderer.arc.x_scale
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
                - zone_renderer.arc.curvature * region_height as f32 * (1.0 - normalized_x * normalized_x);
            let angle = if total_width > 0.0 {
                (4.0 * zone_renderer.arc.curvature * region_height as f32 * normalized_x / total_width).atan()
            } else {
                0.0
            };
            let glyph = Self::render_glyph(zone_renderer, character, region_height, scale, &font)?;
            let glyph_width = (glyph.width() as f32 * zone_renderer.arc.x_scale).round().max(1.0) as u32;
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

        let layer = if zone_renderer.region.rotation.abs() > f32::EPSILON {
            rotate_about_center(
                &layer,
                zone_renderer.region.rotation.to_radians(),
                Interpolation::Bilinear,
                Border::Constant(Rgba([0, 0, 0, 0])),
            )
        } else {
            layer
        };
        let x = zone_renderer.region.cx.round() as i64 - i64::from(layer.width()) / 2;
        let y = zone_renderer.region.cy.round() as i64 - i64::from(layer.height()) / 2;
        imageops::overlay(&mut image, &layer, x, y);

        // Do not resize an already-compressed image. The master is cloned at
        // the beginning of this function, drawn at 1280x1280, and resized
        // exactly once immediately before the final JPEG encode.
        let image = resize_linear(&image, self.output_size, self.output_size);
        let bytes = Arc::new(encode_output(&image, self.output)?);
        Ok(bytes)
    }

    fn render_glyph(
        zone_renderer: &ZoneRenderer,
        text: &str,
        region_height: u32,
        scale: f32,
        font: &FontArc,
    ) -> Result<RgbaImage, String> {
        let (text_width, text_height) = imageproc::drawing::text_size(scale, font, text);
        let padding = zone_renderer.style.outline_width
            .saturating_add(zone_renderer.style.glow.radius)
            .saturating_add(4);
        let width = text_width.saturating_add(padding.saturating_mul(2)).max(1);
        let x = padding as i32;
        let y = ((region_height.saturating_sub(text_height)) / 2) as i32;
        let mut layer = RgbaImage::new(width, region_height.max(1));

        let mut glow = RgbaImage::new(width, region_height.max(1));
        draw_text_mut(&mut glow, zone_renderer.style.glow.color, x, y, scale, font, text);
        if zone_renderer.style.glow.radius > 0 {
            glow = imageops::blur(&glow, zone_renderer.style.glow.radius as f32);
        }
        imageops::overlay(&mut layer, &glow, 0, 0);

        let outline = zone_renderer.style.outline_width as i32;
        for dx in -outline..=outline {
            for dy in -outline..=outline {
                if dx * dx + dy * dy <= outline * outline {
                    draw_text_mut(&mut layer, zone_renderer.style.outline, x + dx, y + dy, scale, font, text);
                }
            }
        }
        draw_text_mut(&mut layer, zone_renderer.style.fill, x, y, scale, font, text);

        let mut mask = RgbaImage::new(width, region_height.max(1));
        draw_text_mut(&mut mask, Rgba([255, 255, 255, 255]), x, y, scale, font, text);

        let shadow = &zone_renderer.style.inner_shadow;
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
        draw_text_mut(&mut highlight, zone_renderer.style.highlight, x - 1, y - 1, scale, font, text);
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
    let zone_scheme = default_zone_scheme()?;
    load_config(manifest_path, output_size, output, zone_scheme)
}

pub fn default_zone_scheme() -> Result<ZoneScheme, String> {
    let algorithm = std::env::var("PB_ZONE_ALGORITHM")
        .ok()
        .map(|value| parse_zone_algorithm(&value))
        .transpose()?
        .unwrap_or_default();
    let max_hr = std::env::var("PB_MAX_HR")
        .ok()
        .map(|value| {
            value
                .parse::<u16>()
                .map_err(|_| "PB_MAX_HR must be between 1 and 999".to_string())
        })
        .transpose()?
        .unwrap_or(200);
    let lactate_threshold = std::env::var("PB_LACTATE_THRESHOLD")
        .ok()
        .map(|value| {
            value
                .parse::<u16>()
                .map_err(|_| "PB_LACTATE_THRESHOLD must be a BPM value".to_string())
        })
        .transpose()?;
    let custom_ranges = std::env::var("PB_CUSTOM_ZONES")
        .ok()
        .map(|value| parse_custom_zones(&value))
        .transpose()?;
    ZoneScheme::from_runtime(algorithm, max_hr, lactate_threshold, custom_ranges)
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

fn load_config(
    path: &Path,
    output_size: u32,
    output: AvatarOutput,
    zone_scheme: ZoneScheme,
) -> Result<AvatarConfig, String> {
    if output_size == 0 || output_size > MASTER_SIZE {
        return Err(format!("output size must be between 1 and {MASTER_SIZE}"));
    }
    if let AvatarOutput::Quality(quality) = output {
        if !(1..=100).contains(&quality) {
            return Err("JPEG quality must be between 1 and 100".into());
        }
    }
    let manifest_path = path;
    let manifest = read_manifest(manifest_path)?;
    let base = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let base_config = resolved_zone_config(base, &manifest, None)?;
    let zones = ZoneId::all().map(|zone| {
        resolved_zone_config(base, &manifest, manifest.zones.get(&zone.as_str().to_ascii_lowercase()))
    });
    let zones: Result<Vec<ResolvedZoneConfig>, String> = zones.into_iter().collect();
    let zones: [ResolvedZoneConfig; 5] = zones?
        .try_into()
        .map_err(|_| "avatar manifest must produce five zone configurations".to_string())?;
    Ok(AvatarConfig {
        output_size,
        output,
        zone_scheme,
        base: base_config,
        zones,
    })
}

fn read_manifest(path: &Path) -> Result<AvatarManifest, String> {
    serde_json::from_slice(
        &fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?,
    )
    .map_err(|error| format!("cannot parse {}: {error}", path.display()))
}

fn resolved_zone_config(
    base: &Path,
    manifest: &AvatarManifest,
    overlay: Option<&AvatarOverride>,
) -> Result<ResolvedZoneConfig, String> {
    let region = overlay.and_then(|value| value.region.as_ref());
    let arc = overlay.and_then(|value| value.arc.as_ref());
    let effects = overlay.and_then(|value| value.effects.as_ref());
    let outline = effects.and_then(|value| value.outline.as_ref());
    let glow = effects.and_then(|value| value.glow.as_ref());
    let inner_shadow = effects.and_then(|value| value.inner_shadow.as_ref());
    let region = TextRegion {
        cx: region.and_then(|value| value.cx).unwrap_or(manifest.region.cx),
        cy: region.and_then(|value| value.cy).unwrap_or(manifest.region.cy),
        width: region.and_then(|value| value.width).unwrap_or(manifest.region.width),
        height: region.and_then(|value| value.height).unwrap_or(manifest.region.height),
        rotation: region.and_then(|value| value.rotation).unwrap_or(manifest.region.rotation),
    };
    if region.width <= 0.0 || region.height <= 0.0 {
        return Err("avatar region width and height must be positive".into());
    }
    let arc = ArcConfig {
        curvature: arc.and_then(|value| value.curvature).unwrap_or(manifest.arc.curvature),
        x_scale: arc.and_then(|value| value.x_scale).unwrap_or(manifest.arc.x_scale),
    };
    if arc.x_scale <= 0.0 {
        return Err("avatar arc x_scale must be positive".into());
    }
    let font_size = overlay
        .and_then(|value| value.font_size)
        .unwrap_or(manifest.font_size);
    if font_size <= 0.0 {
        return Err("avatar font_size must be positive".into());
    }
    let style = AvatarStyle {
        fill: parse_color(effects.and_then(|value| value.fill.as_deref()).unwrap_or(&manifest.effects.fill))?,
        highlight: parse_color(effects.and_then(|value| value.highlight.as_deref()).unwrap_or(&manifest.effects.highlight))?,
        outline: parse_color(outline.and_then(|value| value.color.as_deref()).unwrap_or(&manifest.effects.outline.color))?,
        outline_width: outline.and_then(|value| value.width).unwrap_or(manifest.effects.outline.width),
        glow: GlowStyle {
            color: parse_color(glow.and_then(|value| value.color.as_deref()).unwrap_or(&manifest.effects.glow.color))?,
            radius: glow.and_then(|value| value.radius).unwrap_or(manifest.effects.glow.radius),
        },
        inner_shadow: ShadowStyle {
            color: parse_color(inner_shadow.and_then(|value| value.color.as_deref()).unwrap_or(&manifest.effects.inner_shadow.color))?,
            offset_x: inner_shadow.and_then(|value| value.offset_x).unwrap_or(manifest.effects.inner_shadow.offset_x),
            offset_y: inner_shadow.and_then(|value| value.offset_y).unwrap_or(manifest.effects.inner_shadow.offset_y),
            blur: inner_shadow.and_then(|value| value.blur).unwrap_or(manifest.effects.inner_shadow.blur),
        },
    };
    let background = overlay
        .and_then(|value| value.background.as_ref())
        .unwrap_or(&manifest.background);
    let font = overlay.and_then(|value| value.font.as_ref()).unwrap_or(&manifest.font);
    Ok(ResolvedZoneConfig {
        background: resolve_path(base, background),
        font: resolve_path(base, font),
        font_size,
        region,
        arc,
        style,
    })
}

fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() { path.to_path_buf() } else { base.join(path) }
}

#[cfg(test)]
mod tests {
    use super::{display_bpm, parse_custom_zones, ZoneAlgorithm, ZoneId, ZoneRange, ZoneScheme};

    #[test]
    fn hides_only_leading_zeroes() {
        assert_eq!(display_bpm(7), "  7");
        assert_eq!(display_bpm(70), " 70");
        assert_eq!(display_bpm(100), "100");
        assert_eq!(display_bpm(105), "105");
    }

    #[test]
    fn default_max_hr_scheme_uses_classic_boundaries() {
        let scheme = ZoneScheme::max_hr(200);
        assert_eq!(scheme.algorithm, ZoneAlgorithm::MaxHr);
        assert_eq!(scheme.zone_for(99), ZoneId::OutOfRange);
        assert_eq!(scheme.zone_for(100), ZoneId::Z1);
        assert_eq!(scheme.zone_for(121), ZoneId::Z2);
        assert_eq!(scheme.zone_for(141), ZoneId::Z3);
        assert_eq!(scheme.zone_for(161), ZoneId::Z4);
        assert_eq!(scheme.zone_for(181), ZoneId::Z5);
        assert_eq!(scheme.zone_for(201), ZoneId::OutOfRange);
    }

    #[test]
    fn custom_zone_gaps_are_out_of_range() {
        let scheme = ZoneScheme {
            algorithm: ZoneAlgorithm::Custom,
            max_hr: 200,
            lactate_threshold: None,
            ranges: [
                ZoneRange { min: 50, max: 100 },
                ZoneRange { min: 120, max: 140 },
                ZoneRange { min: 141, max: 160 },
                ZoneRange { min: 161, max: 180 },
                ZoneRange { min: 181, max: 200 },
            ],
        };
        assert_eq!(scheme.zone_for(99), ZoneId::Z1);
        assert_eq!(scheme.zone_for(110), ZoneId::OutOfRange);
        assert_eq!(scheme.zone_for(201), ZoneId::OutOfRange);
    }

    #[test]
    fn lactate_threshold_scheme_has_an_upper_out_of_range_state() {
        let scheme = ZoneScheme::from_runtime(
            ZoneAlgorithm::LactateThreshold,
            200,
            Some(170),
            None,
        )
        .expect("valid lactate-threshold scheme");
        assert_eq!(scheme.zone_for(144), ZoneId::Z1);
        assert_eq!(scheme.zone_for(145), ZoneId::Z2);
        assert_eq!(scheme.zone_for(154), ZoneId::Z3);
        assert_eq!(scheme.zone_for(162), ZoneId::Z4);
        assert_eq!(scheme.zone_for(171), ZoneId::Z5);
        assert_eq!(scheme.zone_for(201), ZoneId::OutOfRange);
    }

    #[test]
    fn parses_five_custom_ranges() {
        let ranges = parse_custom_zones("50-100, 101-140,141-160,161-180,181-200")
            .expect("valid custom ranges");
        assert_eq!(ranges[0], ZoneRange { min: 50, max: 100 });
        assert_eq!(ranges[4], ZoneRange { min: 181, max: 200 });
        assert!(parse_custom_zones("50-100,101-140").is_err());
    }
}
