use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose;
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

const INSTRUCTIONS: &str = r#"We are making images of animals depicting HTTP status codes. They should be entertaining and witty. 

The images *must* be square. 

When generating dogs, they must be Maltese terriers, Poodles or Pomeranian’s. 

Cats? I prefer Blue Burmese or pure white cats with blue eyes. 

Puffins are cool birds. 

When I say “next one” make whatever is next numerically in the list that we haven’t made yet. Your choice, just tell me what it is, and include the number somewhere subtle in the image."#;

#[derive(Parser, Debug)]
#[command(name = "openai_image_generator")]
#[command(about = "Generate witty HTTP status pet images via OpenAI Images API", long_about = None)]
struct Args {
    /// Animal theme for the image
    animal: String,

    /// HTTP status code (e.g. 404, 418, 204)
    #[arg(long)]
    code: Option<u16>,

    /// GPT image model to use
    ///
    /// Examples from docs include dall-e-2, dall-e-3, or a GPT image model (gpt-image-1, gpt-image-1-mini, gpt-image-1.5)
    #[arg(long, default_value = "dall-e-3")]
    model: String,

    /// Quality: low / medium / high / auto
    #[arg(long, value_enum, default_value_t = Quality::Auto)]
    quality: Quality,

    /// OpenAI API Key
    #[arg(required = true, long, env = "OPENAI_API_KEY")]
    openai_api_key: String,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Quality {
    Auto,
    Low,
    Medium,
    High,
}

impl Quality {
    fn as_api_str(self) -> &'static str {
        match self {
            Quality::Auto => "auto",
            Quality::Low => "low",
            Quality::Medium => "medium",
            Quality::High => "high",
        }
    }
}

/// Request body for POST /v1/images/generations
/// Docs: https://platform.openai.com/docs/api-reference/images
#[derive(Serialize, Debug)]
struct ImagesGenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    n: u8,
    size: &'a str,
    quality: &'a str,
    output_format: &'a str,
}

/// Response shape shown in the docs for GPT image models (base64 output)
/// Docs show: { created, data: [{ b64_json }], usage: ... }
#[derive(Deserialize, Debug)]
struct ImagesGenerateResponse {
    data: Vec<ImageData>,
}

#[derive(Deserialize, Debug)]
struct ImageData {
    b64_json: String,
}

fn load_status_codes() -> Result<Vec<u16>> {
    let raw = fs::read_to_string("./data/status_codes.json")
        .context("Failed to read data/status_codes.json")?;
    let parsed: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(&raw).context("Failed to parse status_codes.json")?;
    let mut codes = Vec::with_capacity(parsed.len());
    for key in parsed.keys() {
        let code: u16 = key
            .parse()
            .with_context(|| format!("Invalid status code key {key}"))?;
        codes.push(code);
    }
    codes.sort_unstable();
    Ok(codes)
}

fn existing_codes_for(animal: &str) -> Result<std::collections::HashSet<u16>> {
    let mut existing = std::collections::HashSet::new();
    let dir = PathBuf::from(format!("./images/{animal}"));
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(existing),
        Err(err) => return Err(anyhow!("Failed to read {}: {}", dir.display(), err)),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Ok(code) = stem.parse::<u16>() {
            existing.insert(code);
        }
    }
    Ok(existing)
}

fn confirm_next_code(animal: &str, code: u16) -> Result<bool> {
    let mut stdout = io::stdout();
    write!(
        stdout,
        "Next missing code for {animal} is {code}. Generate? [y/N] "
    )?;
    stdout.flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_ascii_lowercase();
    Ok(matches!(answer.as_str(), "y" | "yes"))
}

fn build_prompt(code: u16, animal: &str) -> String {
    // Constraints from your project instructions:
    // - must be square
    // - dogs must be Maltese terrier, Poodle, or Pomeranian
    // - cats preferred Blue Burmese or pure white with blue eyes
    // - puffins are cool birds
    // - include the number subtly in the image

    // Keep visible text minimal (you usually want that), but still make the HTTP code clear.
    // Include the code subtly (badge, jersey number, tiny label, etc.)
    let prompt = format!(
        r#"
Square, 1:1 composition. Create a funny, witty illustration that personifies HTTP status code {code}.
Subject: {animal}

Scene requirements:
- The number "{code}" must appear subtly in the scene (e.g., tiny tag, badge number, receipt number, postage mark), not as big headline text.
- Minimal readable text overall (avoid big banners/paragraphs).
- Clear visual joke that communicates the meaning/vibe of HTTP {code}.

Style:
- Clean, high-quality, colorful illustration with strong character expression.
- Background and props should support the joke without clutter.

Avoid:
- Watermarks, logos, real brand marks.
- Excessive on-image text.
"#,
    );

    format!(
        "{instructions}\n\n{prompt}",
        instructions = INSTRUCTIONS.trim(),
        prompt = prompt.trim()
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let animal = args.animal.to_ascii_lowercase();
    let status_code = match args.code {
        Some(code) => code,
        None => {
            let codes = load_status_codes()?;
            let existing = existing_codes_for(&animal)?;
            let next = codes.into_iter().find(|code| !existing.contains(code));
            let Some(code) = next else {
                return Err(anyhow!("No missing status codes found for {animal}"));
            };
            if !confirm_next_code(&animal, code)? {
                return Err(anyhow!("Aborted by user"));
            }
            code
        }
    };
    let output_filename = PathBuf::from(format!("./images/{}/{}.png", animal, status_code));
    if output_filename.exists() {
        return Err(anyhow!(
            "Image already exists: {}",
            output_filename.display()
        ));
    }
    if let Some(parent) = output_filename.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    let prompt = build_prompt(status_code, &animal);

    // Per API docs, GPT image models support 1024x1024 (and some rectangular sizes). We'll stick to square.
    // Docs: https://platform.openai.com/docs/api-reference/images
    let req_body = ImagesGenerateRequest {
        model: &args.model,
        prompt: &prompt,
        n: 1,
        size: "1024x1024",
        quality: args.quality.as_api_str(),
        output_format: "png",
    };

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.openai.com/v1/images/generations")
        .bearer_auth(&args.openai_api_key)
        .json(&req_body)
        .send()
        .await
        .context("Request to OpenAI Images API failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("OpenAI API error {status}: {body}"));
    }

    let parsed: ImagesGenerateResponse = resp
        .json()
        .await
        .context("Failed to parse OpenAI Images API response JSON")?;

    let first = parsed
        .data
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("No image data returned"))?;

    let png_bytes = general_purpose::STANDARD
        .decode(first.b64_json)
        .context("Failed to base64-decode image")?;

    fs::write(&output_filename, &png_bytes)
        .with_context(|| format!("Failed to write image to {}", output_filename.display()))?;
    eprintln!("Saved: {}", output_filename.display());

    Ok(())
}
