use std::{collections::HashMap, fs, path::{Path, PathBuf}, sync::{Arc, OnceLock}, thread};

use ab_glyph::{Font, FontArc};
use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping};
use image::{codecs::jpeg::JpegEncoder, imageops, DynamicImage, GenericImageView, Rgba, RgbaImage};
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
#[serde(deny_unknown_fields)]
struct AvatarManifest {
    background: PathBuf,
    #[serde(default)]
    foreground: Option<ForegroundConfig>,
    heart_rate: HeartRateConfig,
    #[serde(default)]
    zones: HashMap<String, AvatarOverride>,
}

#[derive(Clone, Copy, Debug, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum HeartRateLayout {
    #[default]
    Combined,
    Individual,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DisplayMode {
    Text,
    Sprite,
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

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct HeartRateConfig {
    #[serde(default)]
    layout: HeartRateLayout,
    defaults: DisplayConfig,
    #[serde(default)]
    positions: PositionOverrides,
    /// New unified dynamic text objects. The legacy fields above remain
    /// accepted so existing manifests can be migrated incrementally.
    #[serde(default)]
    texts: Vec<TextObjectConfig>,
    #[serde(default)]
    variables: HashMap<String, VariableConfig>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct TextObjectConfig {
    id: String,
    template: String,
    display: DisplayConfig,
}

#[derive(Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct VariableConfig {
    #[serde(default)]
    rules: Vec<VariableRule>,
    default: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct VariableRule {
    when: VariableCondition,
    value: String,
}

#[derive(Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct VariableCondition {
    bpm: Option<u16>,
    bpm_min: Option<u16>,
    bpm_max: Option<u16>,
    zone: Option<String>,
}

#[derive(Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PositionOverrides {
    #[serde(default)]
    hundreds: DisplayOverride,
    #[serde(default)]
    tens: DisplayOverride,
    #[serde(default)]
    ones: DisplayOverride,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DisplayConfig {
    mode: DisplayMode,
    common: CommonConfig,
    #[serde(default)]
    text: Option<TextConfig>,
    #[serde(default)]
    sprite: Option<SpriteConfig>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommonConfig {
    region: TextRegion,
    #[serde(default = "default_true")]
    hide_leading_zeroes: bool,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct TextConfig {
    font: PathBuf,
    #[serde(default)]
    fallback_fonts: Vec<PathBuf>,
    font_size: f32,
    #[serde(default)]
    line_height: Option<f32>,
    #[serde(default)]
    align: TextAlign,
    arc: ArcConfig,
    effects: EffectsConfig,
}

#[derive(Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpriteConfig {
    #[serde(default)]
    spacing: f32,
    #[serde(default = "default_one")]
    scale: f32,
    #[serde(default)]
    effects: SpriteEffectsConfig,
    digits: HashMap<String, SpriteDigitConfig>,
}

#[derive(Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SpriteEffectsConfig {
    #[serde(default = "default_one")]
    opacity: f32,
    tint: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpriteDigitConfig {
    path: PathBuf,
    #[serde(default)]
    rect: Option<SpriteRect>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpriteRect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForegroundConfig {
    path: PathBuf,
    #[serde(default)]
    rect: Option<SpriteRect>,
    #[serde(default)]
    region: Option<TextRegion>,
    #[serde(default = "default_one")]
    opacity: f32,
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
#[serde(deny_unknown_fields)]
struct CommonOverride {
    region: Option<RegionOverride>,
    hide_leading_zeroes: Option<bool>,
}

#[derive(Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TextOverride {
    font: Option<PathBuf>,
    fallback_fonts: Option<Vec<PathBuf>>,
    font_size: Option<f32>,
    line_height: Option<f32>,
    align: Option<TextAlign>,
    arc: Option<ArcOverride>,
    effects: Option<EffectsOverride>,
}

#[derive(Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SpriteEffectsOverride {
    opacity: Option<f32>,
    tint: Option<Option<String>>,
}

#[derive(Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SpriteDigitOverride {
    path: Option<PathBuf>,
    rect: Option<Option<SpriteRect>>,
}

#[derive(Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SpriteOverride {
    spacing: Option<f32>,
    scale: Option<f32>,
    effects: Option<SpriteEffectsOverride>,
    digits: Option<HashMap<String, SpriteDigitOverride>>,
}

#[derive(Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct DisplayOverride {
    mode: Option<DisplayMode>,
    common: Option<CommonOverride>,
    text: Option<TextOverride>,
    sprite: Option<SpriteOverride>,
}

#[derive(Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct HeartRateOverride {
    layout: Option<HeartRateLayout>,
    defaults: Option<DisplayOverride>,
    positions: Option<PositionOverrides>,
    #[serde(default)]
    texts: Option<Vec<TextObjectConfig>>,
    #[serde(default)]
    variables: Option<HashMap<String, VariableConfig>>,
}

#[derive(Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ForegroundOverride {
    path: Option<PathBuf>,
    rect: Option<Option<SpriteRect>>,
    region: Option<RegionOverride>,
    opacity: Option<f32>,
}

#[derive(Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AvatarOverride {
    background: Option<PathBuf>,
    foreground: Option<Option<ForegroundOverride>>,
    heart_rate: Option<HeartRateOverride>,
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
    foreground: Option<ResolvedForegroundConfig>,
    heart_rate: ResolvedHeartRate,
}

#[derive(Clone)]
struct ResolvedHeartRate {
    layout: HeartRateLayout,
    defaults: ResolvedDisplay,
    positions: [ResolvedDisplay; 3],
    texts: Vec<ResolvedTextObject>,
    variables: HashMap<String, VariableConfig>,
}

#[derive(Clone)]
struct ResolvedTextObject {
    id: String,
    template: String,
    display: ResolvedDisplay,
}

struct TextObjectRenderer {
    id: String,
    template: String,
    display: DisplayRenderer,
}

#[derive(Clone)]
struct ResolvedDisplay {
    mode: DisplayMode,
    common: ResolvedCommon,
    text: Option<ResolvedText>,
    sprite: Option<ResolvedSprite>,
}

#[derive(Clone)]
struct ResolvedCommon {
    region: TextRegion,
    hide_leading_zeroes: bool,
}

#[derive(Clone)]
struct ResolvedText {
    font: PathBuf,
    fallback_fonts: Vec<PathBuf>,
    font_size: f32,
    line_height: f32,
    align: TextAlign,
    arc: ArcConfig,
    style: AvatarStyle,
}

#[derive(Clone)]
struct ResolvedSprite {
    spacing: f32,
    scale: f32,
    opacity: f32,
    tint: Option<Rgba<u8>>,
    digits: [ResolvedSpriteDigit; 10],
}

#[derive(Clone)]
struct ResolvedSpriteDigit {
    path: PathBuf,
    rect: Option<SpriteRect>,
}

#[derive(Clone)]
struct ResolvedForegroundConfig {
    path: PathBuf,
    rect: Option<SpriteRect>,
    region: Option<TextRegion>,
    opacity: f32,
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
    heart_rate: HeartRateRenderer,
    foreground: Option<ForegroundLayer>,
}

struct HeartRateRenderer {
    layout: HeartRateLayout,
    defaults: DisplayRenderer,
    positions: [DisplayRenderer; 3],
    texts: Vec<TextObjectRenderer>,
    variables: HashMap<String, VariableConfig>,
}

struct DisplayRenderer {
    mode: DisplayMode,
    common: ResolvedCommon,
    text: Option<TextRenderer>,
    sprite: Option<SpriteRenderer>,
}

struct TextRenderer {
    font: FontArc,
    fallback_fonts: Vec<FontArc>,
    font_size: f32,
    line_height: f32,
    align: TextAlign,
    arc: ArcConfig,
    style: AvatarStyle,
}

struct SpriteRenderer {
    spacing: f32,
    scale: f32,
    opacity: f32,
    tint: Option<Rgba<u8>>,
    digits: [RgbaImage; 10],
}

struct ForegroundLayer {
    image: RgbaImage,
    region: TextRegion,
    opacity: f32,
}

struct TemplateContext<'a> {
    bpm: Option<u16>,
    zone: ZoneId,
    variables: &'a HashMap<String, VariableConfig>,
}

impl<'a> TemplateContext<'a> {
    fn new(bpm: Option<u16>, zone: ZoneId, variables: &'a HashMap<String, VariableConfig>) -> Self {
        Self { bpm, zone, variables }
    }

    fn value(&self, name: &str) -> Result<String, String> {
        match name {
            "bpm" => Ok(self.bpm.map(|value| value.to_string()).unwrap_or_default()),
            "bpm_hundreds" => Ok(self.bpm.and_then(|value| (value >= 100).then_some(value / 100)).map(|value| value.to_string()).unwrap_or_default()),
            "bpm_tens" => Ok(self.bpm.and_then(|value| (value >= 10).then_some((value / 10) % 10)).map(|value| value.to_string()).unwrap_or_default()),
            "bpm_ones" => Ok(self.bpm.map(|value| (value % 10).to_string()).unwrap_or_default()),
            "zone" => Ok(self.zone.as_str().to_ascii_lowercase()),
            "status" => Ok(if self.bpm.is_some() { "online" } else { "offline" }.into()),
            variable => self.variable(variable),
        }
    }

    fn variable(&self, name: &str) -> Result<String, String> {
        let config = self.variables.get(name).ok_or_else(|| format!("unknown avatar template variable {{{name}}}"))?;
        let bpm = self.bpm;
        let zone = self.zone.as_str().to_ascii_lowercase();
        for rule in &config.rules {
            let matches = rule.when.bpm.is_none_or(|expected| bpm == Some(expected))
                && rule.when.bpm_min.is_none_or(|minimum| bpm.is_some_and(|value| value >= minimum))
                && rule.when.bpm_max.is_none_or(|maximum| bpm.is_some_and(|value| value <= maximum))
                && rule.when.zone.as_ref().is_none_or(|expected| expected.eq_ignore_ascii_case(&zone));
            if matches {
                return Ok(rule.value.clone());
            }
        }
        config.default.clone().ok_or_else(|| format!("avatar variable {name} has no matching rule or default"))
    }
}

fn expand_template(template: &str, context: &TemplateContext<'_>) -> Result<String, String> {
    let mut output = String::with_capacity(template.len());
    let mut remaining = template;
    while let Some(start) = remaining.find('{') {
        output.push_str(&remaining[..start]);
        let after_start = &remaining[start + 1..];
        let end = after_start.find('}').ok_or_else(|| format!("unterminated avatar template in {template:?}"))?;
        let name = &after_start[..end];
        if name.is_empty() || name.contains('{') {
            return Err(format!("invalid avatar template variable {{{name}}}"));
        }
        output.push_str(&context.value(name)?);
        remaining = &after_start[end + 1..];
    }
    output.push_str(remaining);
    Ok(output)
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
        let heart_rate = HeartRateRenderer::new(config.heart_rate, coordinate_scale)?;
        let foreground = config
            .foreground
            .map(|foreground| ForegroundLayer::new(foreground, coordinate_scale))
            .transpose()?;

        Ok(Self {
            background,
            heart_rate,
            foreground,
        })
    }
}

impl HeartRateRenderer {
    fn new(config: ResolvedHeartRate, coordinate_scale: f32) -> Result<Self, String> {
        let defaults = DisplayRenderer::new(config.defaults, coordinate_scale)?;
        let positions = config.positions.map(|display| DisplayRenderer::new(display, coordinate_scale))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|_| "avatar renderer expected three heart-rate positions".to_string())?;
        let texts = config
            .texts
            .into_iter()
            .map(|text| {
                Ok(TextObjectRenderer {
                    id: text.id,
                    template: text.template,
                    display: DisplayRenderer::new(text.display, coordinate_scale)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            layout: config.layout,
            defaults,
            positions,
            texts,
            variables: config.variables,
        })
    }

    fn render(&self, image: &mut RgbaImage, label: &str, bpm: Option<u16>, zone: ZoneId) -> Result<(), String> {
        // New text objects own the heart-rate layer. This prevents legacy
        // defaults/positions from being drawn underneath them, especially
        // when a zone override changes an otherwise-hidden sprite opacity.
        if self.texts.is_empty() {
            match self.layout {
                HeartRateLayout::Combined => self.render_display(image, &self.defaults, label)?,
                HeartRateLayout::Individual => {
                    let digits: Vec<char> = label.chars().filter(|character| character.is_ascii_digit()).collect();
                    if digits.is_empty() {
                        self.render_display(image, &self.defaults, label)?;
                    } else {
                        let digits = if self.defaults.common.hide_leading_zeroes {
                            digits
                                .iter()
                                .skip_while(|character| **character == '0')
                                .copied()
                                .collect::<Vec<_>>()
                        } else {
                            digits
                        };
                        let digits = if digits.is_empty() { vec!['0'] } else { digits };
                        let start = 3usize.saturating_sub(digits.len());
                        for (slot, character) in self.positions.iter().skip(start).zip(digits.iter()) {
                            self.render_display(image, slot, &character.to_string())?;
                        }
                    }
                }
            }
        }
        self.render_text_objects(image, bpm, zone)
    }

    fn render_text_objects(&self, image: &mut RgbaImage, bpm: Option<u16>, zone: ZoneId) -> Result<(), String> {
        let context = TemplateContext::new(bpm, zone, &self.variables);
        for text in &self.texts {
            let value = expand_template(&text.template, &context)
                .map_err(|error| format!("text object {}: {error}", text.id))?;
            self.render_display(image, &text.display, &value)?;
        }
        Ok(())
    }

    fn render_display(&self, image: &mut RgbaImage, display: &DisplayRenderer, label: &str) -> Result<(), String> {
        let rendered = match display.mode {
            DisplayMode::Text => display
                .text
                .as_ref()
                .ok_or_else(|| "text mode requires a text configuration".to_string())
                .and_then(|text| render_text_layer(display, text, label)),
            DisplayMode::Sprite => {
                let numeric_label = label.trim();
                if !numeric_label.is_empty() && numeric_label.chars().all(|character| character.is_ascii_digit()) {
                    display
                        .sprite
                        .as_ref()
                        .ok_or_else(|| "sprite mode requires a sprite configuration".to_string())
                        .and_then(|sprite| render_sprite_layer(display, sprite, numeric_label))
                } else {
                    if let Some(text) = &display.text {
                        render_text_layer(display, text, label)
                    } else {
                        Ok(empty_display_layer(display))
                    }
                }
            }
        }?;
        imageops::overlay(image, &rendered, display.common.region.cx.round() as i64 - i64::from(rendered.width()) / 2, display.common.region.cy.round() as i64 - i64::from(rendered.height()) / 2);
        Ok(())
    }
}

fn empty_display_layer(display: &DisplayRenderer) -> RgbaImage {
    let width = display.common.region.width.round().max(1.0) as u32;
    let height = display.common.region.height.round().max(1.0) as u32;
    let layer = RgbaImage::new(width, height);
    if display.common.region.rotation.abs() > f32::EPSILON {
        rotate_about_center(
            &layer,
            display.common.region.rotation.to_radians(),
            Interpolation::Bilinear,
            Border::Constant(Rgba([0, 0, 0, 0])),
        )
    } else {
        layer
    }
}

impl DisplayRenderer {
    fn new(config: ResolvedDisplay, coordinate_scale: f32) -> Result<Self, String> {
        let text = config.text.map(|text| TextRenderer::new(text, coordinate_scale)).transpose()?;
        let sprite = config.sprite.map(|sprite| SpriteRenderer::new(sprite)).transpose()?;
        match config.mode {
            DisplayMode::Text if text.is_none() => return Err("text mode requires a text configuration".into()),
            DisplayMode::Sprite if sprite.is_none() => return Err("sprite mode requires a sprite configuration".into()),
            _ => {}
        }
        Ok(Self {
            mode: config.mode,
            common: ResolvedCommon {
                region: config.common.region.scaled(coordinate_scale),
                hide_leading_zeroes: config.common.hide_leading_zeroes,
            },
            text,
            sprite,
        })
    }
}

impl TextRenderer {
    fn new(config: ResolvedText, coordinate_scale: f32) -> Result<Self, String> {
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
        let fallback_fonts = config
            .fallback_fonts
            .iter()
            .map(|path| {
                let bytes = fs::read(path)
                    .map_err(|error| format!("cannot read fallback font {}: {error}", path.display()))?;
                FontArc::try_from_vec(bytes)
                    .map_err(|_| format!("cannot parse fallback font {}", path.display()))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            font,
            fallback_fonts,
            font_size: config.font_size * coordinate_scale,
            line_height: config.line_height * coordinate_scale,
            align: config.align,
            arc: config.arc,
            style: config.style,
        })
    }

    fn font_for_text(&self, text: &str) -> &FontArc {
        if text.chars().all(|character| self.font.glyph_id(character).0 != 0) {
            return &self.font;
        }
        self.fallback_fonts
            .iter()
            .find(|font| text.chars().all(|character| font.glyph_id(character).0 != 0))
            .unwrap_or(&self.font)
    }
}

impl SpriteRenderer {
    fn new(config: ResolvedSprite) -> Result<Self, String> {
        let mut digits = Vec::with_capacity(10);
        for digit in config.digits {
            let source = image::open(&digit.path)
                .map_err(|error| format!("cannot open {}: {error}", digit.path.display()))?
                .to_rgba8();
            let image = if let Some(rect) = digit.rect {
                validate_rect(rect, source.width(), source.height(), &digit.path)?;
                source.view(rect.x, rect.y, rect.w, rect.h).to_image()
            } else {
                source
            };
            digits.push(image);
        }
        let digits: [RgbaImage; 10] = digits
            .try_into()
            .map_err(|_| "sprite renderer expected ten digit images".to_string())?;
        Ok(Self {
            spacing: config.spacing,
            scale: config.scale,
            opacity: config.opacity,
            tint: config.tint,
            digits,
        })
    }
}

impl ForegroundLayer {
    fn new(config: ResolvedForegroundConfig, coordinate_scale: f32) -> Result<Self, String> {
        let source = image::open(&config.path)
            .map_err(|error| format!("cannot open {}: {error}", config.path.display()))?
            .to_rgba8();
        let source = if let Some(rect) = config.rect {
            validate_rect(rect, source.width(), source.height(), &config.path)?;
            source.view(rect.x, rect.y, rect.w, rect.h).to_image()
        } else {
            source
        };
        let region = config.region
            .map(|region| region.scaled(coordinate_scale))
            .unwrap_or(TextRegion { cx: MASTER_SIZE as f32 / 2.0, cy: MASTER_SIZE as f32 / 2.0, width: MASTER_SIZE as f32, height: MASTER_SIZE as f32, rotation: 0.0 });
        let image = resize_linear(&source, region.width.round().max(1.0) as u32, region.height.round().max(1.0) as u32);
        Ok(Self { image, region, opacity: config.opacity })
    }

    fn overlay(&self, image: &mut RgbaImage) {
        let mut layer = self.image.clone();
        if self.opacity < 1.0 {
            for pixel in layer.pixels_mut() {
                pixel.0[3] = (f32::from(pixel.0[3]) * self.opacity + 0.5) as u8;
            }
        }
        let layer = if self.region.rotation.abs() > f32::EPSILON {
            rotate_about_center(&layer, self.region.rotation.to_radians(), Interpolation::Bilinear, Border::Constant(Rgba([0, 0, 0, 0])))
        } else {
            layer
        };
        imageops::overlay(image, &layer, self.region.cx.round() as i64 - i64::from(layer.width()) / 2, self.region.cy.round() as i64 - i64::from(layer.height()) / 2);
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
        self.render_label(self.zone_scheme.zone_for(bpm), Some(bpm), &display_bpm(bpm))
            .map(|bytes| (*bytes).clone())
    }

    fn render_no_data(&mut self) -> Result<Arc<Vec<u8>>, String> {
        if let Some(cached) = &self.no_data {
            return Ok(cached.clone());
        }
        let bytes = self.render_label(ZoneId::OutOfRange, None, "--")?;
        self.no_data = Some(bytes.clone());
        Ok(bytes)
    }

    fn render_offline(&mut self) -> Result<Arc<Vec<u8>>, String> {
        if let Some(cached) = &self.offline {
            return Ok(cached.clone());
        }
        let bytes = self.render_label(ZoneId::OutOfRange, None, "OFF")?;
        self.offline = Some(bytes.clone());
        Ok(bytes)
    }

    fn render_label(&self, zone: ZoneId, bpm: Option<u16>, text: &str) -> Result<Arc<Vec<u8>>, String> {
        let zone_renderer = zone
            .index()
            .map(|index| &self.zones[index])
            .unwrap_or(&self.base);
        let mut image = zone_renderer.background.clone();
        zone_renderer.heart_rate.render(&mut image, text, bpm, zone)?;
        if let Some(foreground) = &zone_renderer.foreground {
            foreground.overlay(&mut image);
        }

        // Do not resize an already-compressed image. The master is cloned at
        // the beginning of this function, drawn at 1280x1280, and resized
        // exactly once immediately before the final JPEG encode.
        let image = resize_linear(&image, self.output_size, self.output_size);
        let bytes = Arc::new(encode_output(&image, self.output)?);
        Ok(bytes)
    }
}

fn render_text_layer(display: &DisplayRenderer, text_renderer: &TextRenderer, text: &str) -> Result<RgbaImage, String> {
    let region_width = display.common.region.width.round().max(1.0) as u32;
    let region_height = display.common.region.height.round().max(1.0) as u32;
    let lines = text.split('\n').collect::<Vec<_>>();
    if !(1..=3).contains(&lines.len()) {
        return Err(format!("avatar text supports 1 to 3 lines, got {}", lines.len()));
    }
    let line_height = text_renderer.line_height.round().max(1.0) as u32;
    let block_height = line_height
        .checked_mul(lines.len() as u32)
        .ok_or("avatar text block is too tall")?;
    if block_height > region_height {
        return Err(format!(
            "avatar text block height {block_height} exceeds region height {region_height}"
        ));
    }
    let block_top = (region_height - block_height) / 2;
    let mut layer = RgbaImage::new(region_width, region_height);
    for (index, line) in lines.iter().enumerate() {
        let line_layer = render_text_line(text_renderer, line, region_width, line_height)?;
        imageops::overlay(&mut layer, &line_layer, 0, i64::from(block_top + index as u32 * line_height));
    }

    if display.common.region.rotation.abs() > f32::EPSILON {
        Ok(rotate_about_center(
            &layer,
            display.common.region.rotation.to_radians(),
            Interpolation::Bilinear,
            Border::Constant(Rgba([0, 0, 0, 0])),
        ))
    } else {
        Ok(layer)
    }
}

fn render_text_line(text_renderer: &TextRenderer, text: &str, region_width: u32, region_height: u32) -> Result<RgbaImage, String> {
    let scale = text_renderer.font_size;
    let characters: Vec<String> = text.chars().map(|character| character.to_string()).collect();
    let advances: Vec<f32> = characters
        .iter()
        .map(|character| {
            let font = text_renderer.font_for_text(character);
            let (width, _) = imageproc::drawing::text_size(scale, font, character);
            width as f32 * text_renderer.arc.x_scale
        })
        .collect();
    let total_width: f32 = advances.iter().sum();
    let mut cursor = match text_renderer.align {
        TextAlign::Left => 0.0,
        TextAlign::Center => (region_width as f32 - total_width) / 2.0,
        TextAlign::Right => region_width as f32 - total_width,
    };
    let mut layer = RgbaImage::new(region_width, region_height);

    for (character, advance) in characters.iter().zip(advances.iter()) {
        let center_x = cursor + advance / 2.0;
        let normalized_x = if total_width > 0.0 {
            ((center_x - total_width / 2.0) / (total_width / 2.0)).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let center_y = region_height as f32 / 2.0
            - text_renderer.arc.curvature * region_height as f32 * (1.0 - normalized_x * normalized_x);
        let angle = if total_width > 0.0 {
            (4.0 * text_renderer.arc.curvature * region_height as f32 * normalized_x / total_width).atan()
        } else {
            0.0
        };
        let glyph = render_text_glyph(text_renderer, character, region_height, scale)?;
        let glyph_width = (glyph.width() as f32 * text_renderer.arc.x_scale).round().max(1.0) as u32;
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
        imageops::overlay(
            &mut layer,
            &glyph,
            center_x.round() as i64 - i64::from(glyph.width()) / 2,
            center_y.round() as i64 - i64::from(glyph.height()) / 2,
        );
        cursor += advance;
    }

    Ok(layer)
}

fn render_text_glyph(
    text_renderer: &TextRenderer,
    text: &str,
    region_height: u32,
    scale: f32,
) -> Result<RgbaImage, String> {
    let font = text_renderer.font_for_text(text);
    let (text_width, text_height) = imageproc::drawing::text_size(scale, font, text);
    let padding = text_renderer
        .style
        .outline_width
        .saturating_add(text_renderer.style.glow.radius)
        .saturating_add(4);
    let width = text_width.saturating_add(padding.saturating_mul(2)).max(1);
    let x = padding as i32;
    let y = ((region_height.saturating_sub(text_height)) / 2) as i32;
    let mut layer = RgbaImage::new(width, region_height.max(1));

    let mut glow = RgbaImage::new(width, region_height.max(1));
    draw_text_mut(&mut glow, text_renderer.style.glow.color, x, y, scale, font, text);
    if text_renderer.style.glow.radius > 0 {
        glow = imageops::blur(&glow, text_renderer.style.glow.radius as f32);
    }
    imageops::overlay(&mut layer, &glow, 0, 0);

    let outline = text_renderer.style.outline_width as i32;
    for dx in -outline..=outline {
        for dy in -outline..=outline {
            if dx * dx + dy * dy <= outline * outline {
                draw_text_mut(
                    &mut layer,
                    text_renderer.style.outline,
                    x + dx,
                    y + dy,
                    scale,
                    font,
                    text,
                );
            }
        }
    }
    draw_text_mut(&mut layer, text_renderer.style.fill, x, y, scale, font, text);

    let mut mask = RgbaImage::new(width, region_height.max(1));
    draw_text_mut(&mut mask, Rgba([255, 255, 255, 255]), x, y, scale, font, text);

    let shadow = &text_renderer.style.inner_shadow;
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
    draw_text_mut(&mut highlight, text_renderer.style.highlight, x - 1, y - 1, scale, font, text);
    for (highlight_pixel, mask_pixel) in highlight.pixels_mut().zip(mask.pixels()) {
        highlight_pixel.0[3] = highlight_pixel.0[3].min(mask_pixel.0[3]);
    }
    imageops::overlay(&mut layer, &highlight, 0, 0);
    Ok(layer)
}

fn render_sprite_layer(
    display: &DisplayRenderer,
    sprite_renderer: &SpriteRenderer,
    text: &str,
) -> Result<RgbaImage, String> {
    let region_width = display.common.region.width.round().max(1.0) as u32;
    let region_height = display.common.region.height.round().max(1.0) as u32;
    let mut digits = Vec::new();
    for character in text.chars() {
        let index = character
            .to_digit(10)
            .ok_or_else(|| format!("sprite mode cannot render non-numeric character {character}"))?
            as usize;
        let source = &sprite_renderer.digits[index];
        let target_height = (source.height() as f32 * sprite_renderer.scale)
            .round()
            .max(1.0)
            .min(region_height as f32) as u32;
        let target_width = (source.width() as f32 * target_height as f32 / source.height() as f32)
            .round()
            .max(1.0) as u32;
        let mut digit = resize_linear(source, target_width, target_height);
        for pixel in digit.pixels_mut() {
            if let Some(tint) = sprite_renderer.tint {
                pixel.0[0] = (u16::from(pixel.0[0]) * u16::from(tint.0[0]) / 255) as u8;
                pixel.0[1] = (u16::from(pixel.0[1]) * u16::from(tint.0[1]) / 255) as u8;
                pixel.0[2] = (u16::from(pixel.0[2]) * u16::from(tint.0[2]) / 255) as u8;
            }
            pixel.0[3] = (f32::from(pixel.0[3]) * sprite_renderer.opacity + 0.5) as u8;
        }
        digits.push(digit);
    }

    let spacing = sprite_renderer.spacing.max(0.0).round() as u32;
    let mut total_width = digits.iter().map(RgbaImage::width).sum::<u32>();
    total_width = total_width.saturating_add(spacing.saturating_mul(digits.len().saturating_sub(1) as u32));
    if total_width > region_width {
        let ratio = region_width as f32 / total_width as f32;
        for digit in &mut digits {
            let width = (digit.width() as f32 * ratio).round().max(1.0) as u32;
            let height = (digit.height() as f32 * ratio).round().max(1.0) as u32;
            *digit = resize_linear(digit, width, height);
        }
        total_width = digits.iter().map(RgbaImage::width).sum::<u32>();
    }
    let spacing = if digits.len() > 1 {
        let remaining = region_width.saturating_sub(total_width);
        spacing.min(remaining / (digits.len() - 1) as u32)
    } else {
        0
    };
    total_width = digits.iter().map(RgbaImage::width).sum::<u32>()
        + spacing.saturating_mul(digits.len().saturating_sub(1) as u32);
    let mut layer = RgbaImage::new(region_width, region_height);
    let mut x = (region_width.saturating_sub(total_width) / 2) as i64;
    for digit in digits {
        let y = (region_height.saturating_sub(digit.height()) / 2) as i64;
        imageops::overlay(&mut layer, &digit, x, y);
        x += i64::from(digit.width() + spacing);
    }
    if display.common.region.rotation.abs() > f32::EPSILON {
        Ok(rotate_about_center(
            &layer,
            display.common.region.rotation.to_radians(),
            Interpolation::Bilinear,
            Border::Constant(Rgba([0, 0, 0, 0])),
        ))
    } else {
        Ok(layer)
    }
}

fn validate_rect(rect: SpriteRect, width: u32, height: u32, path: &Path) -> Result<(), String> {
    if rect.w == 0 || rect.h == 0 || rect.x.checked_add(rect.w).is_none_or(|right| right > width) || rect.y.checked_add(rect.h).is_none_or(|bottom| bottom > height) {
        return Err(format!(
            "sprite rect ({},{},{},{}) is outside {} ({}x{})",
            rect.x, rect.y, rect.w, rect.h, path.display(), width, height
        ));
    }
    Ok(())
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
    let background = overlay.and_then(|value| value.background.as_ref()).unwrap_or(&manifest.background);
    let heart_rate = resolve_heart_rate(base, &manifest.heart_rate, overlay.and_then(|value| value.heart_rate.as_ref()))?;
    let foreground = resolve_foreground(base, manifest.foreground.as_ref(), overlay.and_then(|value| value.foreground.as_ref()))?;
    Ok(ResolvedZoneConfig {
        background: resolve_path(base, background),
        foreground,
        heart_rate,
    })
}

fn resolve_heart_rate(
    base: &Path,
    config: &HeartRateConfig,
    overlay: Option<&HeartRateOverride>,
) -> Result<ResolvedHeartRate, String> {
    let layout = overlay.and_then(|value| value.layout).unwrap_or(config.layout);
    let defaults = resolve_display(base, &config.defaults, overlay.and_then(|value| value.defaults.as_ref()))?;
    let zone_positions = overlay.and_then(|value| value.positions.as_ref());
    let positions = [
        resolve_position(base, &config.defaults, &config.positions.hundreds, overlay, zone_positions.map(|value| &value.hundreds))?,
        resolve_position(base, &config.defaults, &config.positions.tens, overlay, zone_positions.map(|value| &value.tens))?,
        resolve_position(base, &config.defaults, &config.positions.ones, overlay, zone_positions.map(|value| &value.ones))?,
    ];
    let text_configs = overlay
        .and_then(|value| value.texts.as_ref())
        .unwrap_or(&config.texts);
    let texts = text_configs
        .iter()
        .map(|text| {
            Ok(ResolvedTextObject {
                id: text.id.clone(),
                template: text.template.clone(),
                display: resolve_display(base, &text.display, None)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut variables = config.variables.clone();
    if let Some(overlay_variables) = overlay.and_then(|value| value.variables.as_ref()) {
        variables.extend(overlay_variables.clone());
    }
    Ok(ResolvedHeartRate { layout, defaults, positions, texts, variables })
}

fn resolve_position(
    base: &Path,
    defaults: &DisplayConfig,
    base_override: &DisplayOverride,
    heart_rate_overlay: Option<&HeartRateOverride>,
    position_overlay: Option<&DisplayOverride>,
) -> Result<ResolvedDisplay, String> {
    let mut display = resolve_display(base, defaults, Some(base_override))?;
    if let Some(overlay) = heart_rate_overlay.and_then(|value| value.defaults.as_ref()) {
        apply_display_override(base, &mut display, overlay)?;
    }
    if let Some(overlay) = position_overlay {
        apply_display_override(base, &mut display, overlay)?;
    }
    Ok(display)
}

fn resolve_display(
    base: &Path,
    config: &DisplayConfig,
    overlay: Option<&DisplayOverride>,
) -> Result<ResolvedDisplay, String> {
    let mut display = ResolvedDisplay {
        mode: config.mode,
        common: resolve_common(&config.common)?,
        text: config.text.as_ref().map(|value| resolve_text(base, value)).transpose()?,
        sprite: config.sprite.as_ref().map(|value| resolve_sprite(base, value)).transpose()?,
    };
    if let Some(overlay) = overlay {
        apply_display_override(base, &mut display, overlay)?;
    }
    validate_display(&display)
}

fn validate_display(display: &ResolvedDisplay) -> Result<ResolvedDisplay, String> {
    match display.mode {
        DisplayMode::Text if display.text.is_none() => Err("text mode requires a text configuration".into()),
        DisplayMode::Sprite if display.sprite.is_none() => Err("sprite mode requires a sprite configuration".into()),
        _ => Ok(display.clone()),
    }
}

fn apply_display_override(
    base: &Path,
    display: &mut ResolvedDisplay,
    overlay: &DisplayOverride,
) -> Result<(), String> {
    if let Some(mode) = overlay.mode {
        display.mode = mode;
    }
    if let Some(common) = &overlay.common {
        apply_common_override(&mut display.common, common)?;
    }
    if let Some(text) = &overlay.text {
        if let Some(current) = &mut display.text {
            apply_text_override(base, current, text)?;
        } else {
            display.text = Some(resolve_text_from_override(base, text)?);
        }
    }
    if let Some(sprite) = &overlay.sprite {
        if let Some(current) = &mut display.sprite {
            apply_sprite_override(base, current, sprite)?;
        } else {
            display.sprite = Some(resolve_sprite_from_override(base, sprite)?);
        }
    }
    *display = validate_display(display)?;
    Ok(())
}

fn resolve_common(config: &CommonConfig) -> Result<ResolvedCommon, String> {
    validate_region(&config.region)?;
    Ok(ResolvedCommon {
        region: config.region.clone(),
        hide_leading_zeroes: config.hide_leading_zeroes,
    })
}

fn apply_common_override(common: &mut ResolvedCommon, overlay: &CommonOverride) -> Result<(), String> {
    if let Some(region) = &overlay.region {
        apply_region_override(&mut common.region, region)?;
    }
    if let Some(value) = overlay.hide_leading_zeroes {
        common.hide_leading_zeroes = value;
    }
    Ok(())
}

fn resolve_text(base: &Path, config: &TextConfig) -> Result<ResolvedText, String> {
    validate_arc(&config.arc)?;
    validate_font_size(config.font_size)?;
    let line_height = config.line_height.unwrap_or(config.font_size * 1.15);
    validate_line_height(line_height)?;
    Ok(ResolvedText {
        font: resolve_path(base, &config.font),
        fallback_fonts: config.fallback_fonts.iter().map(|font| resolve_path(base, font)).collect(),
        font_size: config.font_size,
        line_height,
        align: config.align,
        arc: config.arc.clone(),
        style: style_from_config(&config.effects)?,
    })
}

fn resolve_text_from_override(base: &Path, overlay: &TextOverride) -> Result<ResolvedText, String> {
    let font = overlay.font.as_ref().ok_or("text override requires font when no base text configuration exists")?;
    let font_size = overlay.font_size.ok_or("text override requires font_size when no base text configuration exists")?;
    let line_height = overlay.line_height.unwrap_or(font_size * 1.15);
    validate_line_height(line_height)?;
    let arc = overlay.arc.as_ref().ok_or("text override requires arc when no base text configuration exists")?;
    let effects = overlay.effects.as_ref().ok_or("text override requires effects when no base text configuration exists")?;
    let style = style_from_override(effects)?;
    let arc = ArcConfig {
        curvature: arc.curvature.ok_or("text override arc requires curvature")?,
        x_scale: arc.x_scale.ok_or("text override arc requires x_scale")?,
    };
    validate_arc(&arc)?;
    validate_font_size(font_size)?;
    Ok(ResolvedText {
        font: resolve_path(base, font),
        fallback_fonts: overlay.fallback_fonts.as_ref().map(|fonts| fonts.iter().map(|font| resolve_path(base, font)).collect()).unwrap_or_default(),
        font_size,
        line_height,
        align: overlay.align.unwrap_or_default(),
        arc,
        style,
    })
}

fn apply_text_override(base: &Path, text: &mut ResolvedText, overlay: &TextOverride) -> Result<(), String> {
    if let Some(font) = &overlay.font {
        text.font = resolve_path(base, font);
    }
    if let Some(fallback_fonts) = &overlay.fallback_fonts {
        text.fallback_fonts = fallback_fonts.iter().map(|font| resolve_path(base, font)).collect();
    }
    if let Some(font_size) = overlay.font_size {
        validate_font_size(font_size)?;
        text.font_size = font_size;
        if overlay.line_height.is_none() {
            text.line_height = font_size * 1.15;
        }
    }
    if let Some(line_height) = overlay.line_height {
        validate_line_height(line_height)?;
        text.line_height = line_height;
    }
    if let Some(align) = overlay.align {
        text.align = align;
    }
    if let Some(arc) = &overlay.arc {
        apply_arc_override(&mut text.arc, arc)?;
    }
    if let Some(effects) = &overlay.effects {
        apply_effects_override(&mut text.style, effects)?;
    }
    Ok(())
}

fn resolve_sprite(base: &Path, config: &SpriteConfig) -> Result<ResolvedSprite, String> {
    validate_sprite_values(config.spacing, config.scale, config.effects.opacity)?;
    let digits = resolve_digits(base, &config.digits)?;
    Ok(ResolvedSprite {
        spacing: config.spacing,
        scale: config.scale,
        opacity: config.effects.opacity,
        tint: config.effects.tint.as_deref().map(parse_color).transpose()?,
        digits,
    })
}

fn resolve_sprite_from_override(base: &Path, overlay: &SpriteOverride) -> Result<ResolvedSprite, String> {
    let digits = overlay.digits.as_ref().ok_or("sprite override requires digits 0-9 when no base sprite configuration exists")?;
    let mut resolved = Vec::with_capacity(10);
    for index in 0..10 {
        let key = index.to_string();
        let digit = digits.get(&key).ok_or_else(|| format!("sprite override is missing digit {index}"))?;
        let path = digit.path.as_ref().ok_or_else(|| format!("sprite override digit {index} requires path"))?;
        resolved.push(resolve_sprite_digit(base, path, digit.rect.as_ref().and_then(|value| *value))?);
    }
    let spacing = overlay.spacing.unwrap_or(0.0);
    let scale = overlay.scale.unwrap_or(1.0);
    let (opacity, tint) = resolve_sprite_effects(None, overlay.effects.as_ref())?;
    validate_sprite_values(spacing, scale, opacity)?;
    Ok(ResolvedSprite { spacing, scale, opacity, tint, digits: resolved.try_into().map_err(|_| "sprite renderer expected ten digit images".to_string())? })
}

fn apply_sprite_override(base: &Path, sprite: &mut ResolvedSprite, overlay: &SpriteOverride) -> Result<(), String> {
    if let Some(spacing) = overlay.spacing {
        sprite.spacing = spacing;
    }
    if let Some(scale) = overlay.scale {
        sprite.scale = scale;
    }
    if let Some(effects) = &overlay.effects {
        if let Some(opacity) = effects.opacity {
            sprite.opacity = opacity;
        }
        if let Some(tint) = &effects.tint {
            sprite.tint = tint.as_deref().map(parse_color).transpose()?;
        }
    }
    if let Some(digits) = &overlay.digits {
        for (key, digit) in digits {
            let index = key.parse::<usize>().map_err(|_| format!("invalid sprite digit {key}"))?;
            if index >= 10 {
                return Err(format!("invalid sprite digit {key}"));
            }
            let current = &mut sprite.digits[index];
            if let Some(path) = &digit.path {
                current.path = resolve_path(base, path);
            }
            if let Some(rect) = &digit.rect {
                current.rect = *rect;
            }
        }
    }
    validate_sprite_values(sprite.spacing, sprite.scale, sprite.opacity)
}

fn resolve_digits(base: &Path, digits: &HashMap<String, SpriteDigitConfig>) -> Result<[ResolvedSpriteDigit; 10], String> {
    let mut resolved = Vec::with_capacity(10);
    for index in 0..10 {
        let key = index.to_string();
        let digit = digits.get(&key).ok_or_else(|| format!("sprite configuration is missing digit {index}"))?;
        resolved.push(resolve_sprite_digit(base, &digit.path, digit.rect)?);
    }
    resolved.try_into().map_err(|_| "sprite renderer expected ten digit images".to_string())
}

fn resolve_sprite_digit(base: &Path, path: &Path, rect: Option<SpriteRect>) -> Result<ResolvedSpriteDigit, String> {
    Ok(ResolvedSpriteDigit { path: resolve_path(base, path), rect })
}

fn resolve_sprite_effects(
    base: Option<&SpriteEffectsConfig>,
    overlay: Option<&SpriteEffectsOverride>,
) -> Result<(f32, Option<Rgba<u8>>), String> {
    let opacity = overlay.and_then(|value| value.opacity)
        .or_else(|| base.map(|value| value.opacity))
        .unwrap_or(1.0);
    let tint = if let Some(value) = overlay.and_then(|value| value.tint.as_ref()) {
        value.as_deref().map(parse_color).transpose()?
    } else {
        base.and_then(|value| value.tint.as_deref()).map(parse_color).transpose()?
    };
    Ok((opacity, tint))
}

fn validate_sprite_values(spacing: f32, scale: f32, opacity: f32) -> Result<(), String> {
    if spacing < 0.0 {
        return Err("sprite spacing must not be negative".into());
    }
    if scale <= 0.0 {
        return Err("sprite scale must be positive".into());
    }
    if !(0.0..=1.0).contains(&opacity) {
        return Err("sprite opacity must be between 0 and 1".into());
    }
    Ok(())
}

fn resolve_foreground(
    base: &Path,
    config: Option<&ForegroundConfig>,
    overlay: Option<&Option<ForegroundOverride>>,
) -> Result<Option<ResolvedForegroundConfig>, String> {
    let Some(overlay) = overlay else {
        return config.map(|value| resolve_foreground_config(base, value)).transpose();
    };
    let Some(overlay) = overlay else {
        return Ok(None);
    };
    let mut resolved = config
        .map(|value| ResolvedForegroundConfig {
            path: resolve_path(base, &value.path),
            rect: value.rect,
            region: value.region.clone(),
            opacity: value.opacity,
        });
    if resolved.is_none() && overlay.path.is_none() {
        return Err("foreground override requires path when no base foreground exists".into());
    }
    if let Some(path) = &overlay.path {
        resolved.get_or_insert(ResolvedForegroundConfig {
            path: resolve_path(base, path),
            rect: None,
            region: None,
            opacity: 1.0,
        }).path = resolve_path(base, path);
    }
    let resolved = resolved.as_mut().ok_or("invalid foreground override")?;
    if let Some(rect) = &overlay.rect {
        resolved.rect = *rect;
    }
    if let Some(region) = &overlay.region {
        let mut current = resolved.region.clone().unwrap_or(TextRegion { cx: 640.0, cy: 640.0, width: 1280.0, height: 1280.0, rotation: 0.0 });
        apply_region_override(&mut current, region)?;
        resolved.region = Some(current);
    }
    if let Some(opacity) = overlay.opacity {
        resolved.opacity = opacity;
    }
    validate_foreground(resolved).map(Some)
}

fn resolve_foreground_config(base: &Path, value: &ForegroundConfig) -> Result<ResolvedForegroundConfig, String> {
    let resolved = ResolvedForegroundConfig {
        path: resolve_path(base, &value.path),
        rect: value.rect,
        region: value.region.clone(),
        opacity: value.opacity,
    };
    validate_foreground(&resolved)
}

fn validate_foreground(value: &ResolvedForegroundConfig) -> Result<ResolvedForegroundConfig, String> {
    if !(0.0..=1.0).contains(&value.opacity) {
        return Err("foreground opacity must be between 0 and 1".into());
    }
    if let Some(region) = &value.region {
        validate_region(region)?;
    }
    Ok(value.clone())
}

fn default_true() -> bool {
    true
}

fn default_one() -> f32 {
    1.0
}

fn validate_region(region: &TextRegion) -> Result<(), String> {
    if region.width <= 0.0 || region.height <= 0.0 {
        return Err("avatar region width and height must be positive".into());
    }
    Ok(())
}

fn apply_region_override(region: &mut TextRegion, overlay: &RegionOverride) -> Result<(), String> {
    if let Some(value) = overlay.cx {
        region.cx = value;
    }
    if let Some(value) = overlay.cy {
        region.cy = value;
    }
    if let Some(value) = overlay.width {
        region.width = value;
    }
    if let Some(value) = overlay.height {
        region.height = value;
    }
    if let Some(value) = overlay.rotation {
        region.rotation = value;
    }
    validate_region(region)
}

fn validate_arc(arc: &ArcConfig) -> Result<(), String> {
    if !(-1.0..=1.0).contains(&arc.curvature) {
        return Err("avatar arc curvature must be between -1 and 1".into());
    }
    if arc.x_scale <= 0.0 {
        return Err("avatar arc x_scale must be positive".into());
    }
    Ok(())
}

fn apply_arc_override(arc: &mut ArcConfig, overlay: &ArcOverride) -> Result<(), String> {
    if let Some(value) = overlay.curvature {
        arc.curvature = value;
    }
    if let Some(value) = overlay.x_scale {
        arc.x_scale = value;
    }
    validate_arc(arc)
}

fn validate_font_size(font_size: f32) -> Result<(), String> {
    if font_size <= 0.0 {
        return Err("avatar font_size must be positive".into());
    }
    Ok(())
}

fn validate_line_height(line_height: f32) -> Result<(), String> {
    if line_height <= 0.0 {
        return Err("avatar line_height must be positive".into());
    }
    Ok(())
}

fn style_from_config(config: &EffectsConfig) -> Result<AvatarStyle, String> {
    Ok(AvatarStyle {
        fill: parse_color(&config.fill)?,
        highlight: parse_color(&config.highlight)?,
        outline: parse_color(&config.outline.color)?,
        outline_width: config.outline.width,
        glow: GlowStyle {
            color: parse_color(&config.glow.color)?,
            radius: config.glow.radius,
        },
        inner_shadow: ShadowStyle {
            color: parse_color(&config.inner_shadow.color)?,
            offset_x: config.inner_shadow.offset_x,
            offset_y: config.inner_shadow.offset_y,
            blur: config.inner_shadow.blur,
        },
    })
}

fn style_from_override(overlay: &EffectsOverride) -> Result<AvatarStyle, String> {
    let outline = overlay.outline.as_ref().ok_or("text override effects requires outline")?;
    let glow = overlay.glow.as_ref().ok_or("text override effects requires glow")?;
    let shadow = overlay.inner_shadow.as_ref().ok_or("text override effects requires inner_shadow")?;
    Ok(AvatarStyle {
        fill: parse_color(overlay.fill.as_deref().ok_or("text override effects requires fill")?)?,
        highlight: parse_color(overlay.highlight.as_deref().ok_or("text override effects requires highlight")?)?,
        outline: parse_color(outline.color.as_deref().ok_or("text override outline requires color")?)?,
        outline_width: outline.width.ok_or("text override outline requires width")?,
        glow: GlowStyle {
            color: parse_color(glow.color.as_deref().ok_or("text override glow requires color")?)?,
            radius: glow.radius.ok_or("text override glow requires radius")?,
        },
        inner_shadow: ShadowStyle {
            color: parse_color(shadow.color.as_deref().ok_or("text override inner_shadow requires color")?)?,
            offset_x: shadow.offset_x.ok_or("text override inner_shadow requires offset_x")?,
            offset_y: shadow.offset_y.ok_or("text override inner_shadow requires offset_y")?,
            blur: shadow.blur.ok_or("text override inner_shadow requires blur")?,
        },
    })
}

fn apply_effects_override(style: &mut AvatarStyle, overlay: &EffectsOverride) -> Result<(), String> {
    if let Some(value) = &overlay.fill {
        style.fill = parse_color(value)?;
    }
    if let Some(value) = &overlay.highlight {
        style.highlight = parse_color(value)?;
    }
    if let Some(value) = &overlay.outline {
        if let Some(color) = &value.color {
            style.outline = parse_color(color)?;
        }
        if let Some(width) = value.width {
            style.outline_width = width;
        }
    }
    if let Some(value) = &overlay.glow {
        if let Some(color) = &value.color {
            style.glow.color = parse_color(color)?;
        }
        if let Some(radius) = value.radius {
            style.glow.radius = radius;
        }
    }
    if let Some(value) = &overlay.inner_shadow {
        if let Some(color) = &value.color {
            style.inner_shadow.color = parse_color(color)?;
        }
        if let Some(offset_x) = value.offset_x {
            style.inner_shadow.offset_x = offset_x;
        }
        if let Some(offset_y) = value.offset_y {
            style.inner_shadow.offset_y = offset_y;
        }
        if let Some(blur) = value.blur {
            style.inner_shadow.blur = blur;
        }
    }
    Ok(())
}

fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() { path.to_path_buf() } else { base.join(path) }
}

#[cfg(test)]
mod tests {
    use super::{display_bpm, expand_template, parse_custom_zones, HashMap, TemplateContext, VariableConfig, VariableRule, VariableCondition, ZoneAlgorithm, ZoneId, ZoneRange, ZoneScheme};

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

    #[test]
    fn expands_bpm_and_digit_templates_deterministically() {
        let variables = HashMap::new();
        let context = TemplateContext::new(Some(58), ZoneId::Z1, &variables);
        assert_eq!(expand_template("{bpm}|{bpm_hundreds}|{bpm_tens}|{bpm_ones}|{zone}", &context).unwrap(), "58||5|8|z1");
    }

    #[test]
    fn resolves_exact_variable_rules_before_default() {
        let mut variables = HashMap::new();
        variables.insert("fun".into(), VariableConfig {
            rules: vec![VariableRule {
                when: VariableCondition { bpm: Some(11), ..Default::default() },
                value: "不对吧".into(),
            }],
            default: Some("正常".into()),
        });
        let eleven = TemplateContext::new(Some(11), ZoneId::Z1, &variables);
        let twelve = TemplateContext::new(Some(12), ZoneId::Z1, &variables);
        assert_eq!(expand_template("{fun}", &eleven).unwrap(), "不对吧");
        assert_eq!(expand_template("{fun}", &twelve).unwrap(), "正常");
    }
}
