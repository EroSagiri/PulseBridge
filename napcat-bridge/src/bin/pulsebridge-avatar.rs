use std::{fs, path::PathBuf, process::ExitCode};

use clap::Parser;
use pulsebridge_napcat_bridge::avatar::{AvatarGenerator, AvatarOutput, DEFAULT_OUTPUT_SIZE, DEFAULT_QUALITY};

#[derive(Debug, Parser)]
#[command(
    name = "pulsebridge-avatar",
    about = "Render heart-rate avatar JPEGs from an avatar.json manifest",
    long_about = "Renders every image independently from the manifest's high-resolution background.\nThe BPM list is cycled when --count is larger than the number of BPM values."
)]
struct Args {
    /// Path to avatar.json. Relative background and font paths are resolved beside it.
    #[arg(value_name = "AVATAR_JSON")]
    manifest: PathBuf,

    /// Heart-rate values, for example: --bpm 66,80,180
    #[arg(long, value_name = "BPM[,BPM...]", value_delimiter = ',', required = true,
        value_parser = clap::value_parser!(u16).range(0..=999))]
    bpm: Vec<u16>,

    /// Number of JPEGs to generate; BPM values repeat as needed.
    #[arg(long, default_value_t = 60, value_name = "N")]
    count: usize,

    /// Final square image size in pixels.
    #[arg(long, default_value_t = DEFAULT_OUTPUT_SIZE, value_name = "PIXELS")]
    size: u32,

    /// JPEG quality mode (1-100). This is the default output mode.
    #[arg(long, value_name = "1-100", conflicts_with = "max_bytes",
        value_parser = clap::value_parser!(u8).range(1..=100))]
    quality: Option<u8>,

    /// File-size mode; use values such as 10k, 100k, or 1m (k/m are decimal).
    #[arg(long, value_name = "BYTES", conflicts_with = "quality",
        value_parser = parse_max_bytes)]
    max_bytes: Option<usize>,

    /// Directory receiving generated .jpg files.
    #[arg(long, default_value = "avatar-preview", value_name = "DIR")]
    output: PathBuf,
}

fn parse_max_bytes(value: &str) -> Result<usize, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("max-bytes cannot be empty".into());
    }
    let split_at = value.find(|character: char| !character.is_ascii_digit()).unwrap_or(value.len());
    let (number, suffix) = value.split_at(split_at);
    let number: usize = number.parse().map_err(|_| format!("invalid byte limit: {value}"))?;
    let multiplier = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" => 1000,
        "ki" | "kib" => 1024,
        "m" | "mb" => 1000 * 1000,
        "mi" | "mib" => 1024 * 1024,
        "g" | "gb" => 1000 * 1000 * 1000,
        "gi" | "gib" => 1024 * 1024 * 1024,
        _ => return Err(format!("invalid byte suffix in {value}; use b, k, or m")),
    };
    let bytes = number.checked_mul(multiplier).ok_or_else(|| format!("byte limit is too large: {value}"))?;
    if bytes == 0 {
        return Err("max-bytes must be greater than zero".into());
    }
    Ok(bytes)
}

fn main() -> ExitCode {
    let args = Args::parse();
    if args.count == 0 {
        eprintln!("error: count must be greater than zero");
        return ExitCode::from(2);
    }
    if args.size == 0 {
        eprintln!("error: size must be greater than zero");
        return ExitCode::from(2);
    }

    let output_mode = args
        .max_bytes
        .map(AvatarOutput::MaxBytes)
        .unwrap_or_else(|| AvatarOutput::Quality(args.quality.unwrap_or(DEFAULT_QUALITY)));
    let config = match AvatarGenerator::config_from_manifest(&args.manifest, args.size, output_mode) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };
    let mut generator = match AvatarGenerator::new(config) {
        Ok(generator) => generator,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };
    if let Err(error) = fs::create_dir_all(&args.output) {
        eprintln!("error: cannot create {}: {error}", args.output.display());
        return ExitCode::from(1);
    }

    for index in 0..args.count {
        let bpm = args.bpm[index % args.bpm.len()];
        let bytes = match generator.render(bpm) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("error: failed to render BPM {bpm}: {error}");
                return ExitCode::from(1);
            }
        };
        let path = args.output.join(format!("heart-rate-{:03}-{:03}.jpg", bpm, index + 1));
        if let Err(error) = fs::write(&path, &bytes) {
            eprintln!("error: cannot write {}: {error}", path.display());
            return ExitCode::from(1);
        }
        println!("{}: {} bytes", path.display(), bytes.len());
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::parse_max_bytes;

    #[test]
    fn parses_human_byte_limits() {
        assert_eq!(parse_max_bytes("100k"), Ok(100 * 1000));
        assert_eq!(parse_max_bytes("10KB"), Ok(10 * 1000));
        assert_eq!(parse_max_bytes("10KiB"), Ok(10 * 1024));
        assert_eq!(parse_max_bytes("1m"), Ok(1000 * 1000));
        assert_eq!(parse_max_bytes("1MiB"), Ok(1024 * 1024));
    }

    #[test]
    fn rejects_invalid_byte_limits() {
        assert!(parse_max_bytes("0").is_err());
        assert!(parse_max_bytes("10x").is_err());
        assert!(parse_max_bytes("k").is_err());
    }
}
