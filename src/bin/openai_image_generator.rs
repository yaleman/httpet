use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose;
use clap::{Parser, ValueEnum};
use httpet::status_codes::STATUS_CODES;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use tracing::log::info;

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
    #[arg(long, value_enum, default_value_t = Quality::Hd)]
    quality: Quality,

    /// OpenAI API Key
    #[arg(required = true, long, env = "OPENAI_API_KEY", hide_env_values = true)]
    openai_api_key: String,
}

#[derive(Copy, Clone, Debug, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
enum Quality {
    Auto,
    Low,
    Medium,
    High,
    Hd,
}

/// Request body for POST /v1/images/generations
/// Docs: https://platform.openai.com/docs/api-reference/images
#[derive(Serialize, Debug)]
struct ImagesGenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    n: u8,
    /// The size of the generated images. Must be one of 1024x1024, 1536x1024 (landscape), 1024x1536 (portrait), or auto (default value) for the GPT image models, one of 256x256, 512x512, or 1024x1024 for dall-e-2, and one of 1024x1024, 1792x1024, or 1024x1792 for dall-e-3.
    size: &'a str,
    quality: Quality,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<&'a str>,
    /// The style of the generated images. This parameter is only supported for dall-e-3. Must be one of vivid or natural. Vivid causes the model to lean towards generating hyper-real and dramatic images. Natural causes the model to produce more natural, less hyper-real looking images.
    #[serde(skip_serializing_if = "Option::is_none")]
    style: Option<Style>,
}

/// Response shape shown in the docs for GPT image models (base64 output)
/// Docs show: { created, data: [{ b64_json }], usage: ... }
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct ImagesGenerateResponse {
    #[serde(default)]
    created: u64,
    data: Vec<ImageData>,
    #[serde(default)]
    usage: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Deserialize, Debug)]
pub struct ImageData {
    b64_json: Option<String>,
    url: Option<String>,
    revised_prompt: Option<String>,
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

/// Prompt the user to confirm generating the next code
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
    let mut instruction_lines = vec![
        "We are making images of animals depicting HTTP status codes. They should be entertaining and witty.".to_string(),
        "The images must be square.".to_string(),
    ];
    match animal {
        "dog" | "dogs" => instruction_lines
            .push("Dogs must be Maltese terriers, Poodles, or Pomeranians.".to_string()),
        "cat" | "cats" => instruction_lines
            .push("Cats should be Blue Burmese or pure white cats with blue eyes.".to_string()),
        "puffin" | "puffins" => instruction_lines.push("Puffins are cool birds.".to_string()),
        _ => {}
    }
    let instructions = instruction_lines.join("\n");

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
Square, 1:1 composition. Create a funny, witty image of a {animal} that personifies HTTP status code {code}. The style should be 3d animation with strong character expression.

Scene requirements:
- The number "{code}" must appear subtly in the scene (e.g., tiny tag, badge number, receipt number, postage mark), not as big headline text.
- Avoid text at all costs.
- Clear visual joke that communicates the meaning/vibe of HTTP {code}.

Style:
- 3d animation style with strong character expression.
- Background and props should support the joke without overwhelmning the vibe.

Avoid:
- Watermarks, logos, real brand marks.
- Text.
"#,
    );

    format!(
        "{instructions}\n\n{prompt}",
        instructions = instructions.trim(),
        prompt = prompt.trim()
    )
}

#[derive(Copy, Clone, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    Natural,
    Vivid,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let animal = args.animal.to_ascii_lowercase();
    let status_code = match args.code {
        Some(code) => code,
        None => {
            let existing = existing_codes_for(&animal)?;
            let next = STATUS_CODES.keys().find(|code| !existing.contains(code));
            let Some(code) = next else {
                return Err(anyhow!("No missing status codes found for {animal}"));
            };
            if !confirm_next_code(&animal, *code)? {
                return Err(anyhow!("Aborted by user"));
            }
            *code
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
    let mut req_body = ImagesGenerateRequest {
        model: &args.model,
        prompt: &prompt,
        n: 1,
        size: "1024x1024", // supported by everything
        quality: args.quality,
        response_format: (args.model == "dall-e-2" || args.model == "dall-e-3")
            .then_some("b64_json"),
        style: None,
    };

    if args.model.contains("gpt-image") {
        // GPT image models support larger sizes
        req_body.quality = Quality::High;
    } else if args.model.eq("dall-e-3") {
        // dall-e-3 supports styles
        req_body.style = Some(Style::Natural);
    }

    info!(
        "Requesting image: animal={animal}, code={status_code}, model={}, size={}, quality={:?}, output={}",
        args.model,
        req_body.size,
        req_body.quality,
        output_filename.display()
    );

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

    let response_bytes = resp
        .bytes()
        .await
        .context("Failed to read OpenAI Images API response bytes")?;

    // write them to disk for debugging
    fs::write("debug_image_response.json", &response_bytes)?;

    let parsed: ImagesGenerateResponse = serde_json::from_slice(&response_bytes)
        .context("Failed to parse OpenAI Images API response JSON, it was saved to debug_image_response.json")?;

    let first = parsed
        .data
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("No image data returned"))?;

    let png_bytes = if let Some(b64_json) = first.b64_json {
        general_purpose::STANDARD
            .decode(b64_json)
            .context("Failed to base64-decode image")?
    } else if let Some(url) = first.url {
        let bytes = client
            .get(url)
            .send()
            .await
            .context("Failed to download image URL")?
            .bytes()
            .await
            .context("Failed to read image bytes")?;
        bytes.to_vec()
    } else {
        return Err(anyhow!("Image response missing b64_json and url fields"));
    };

    if let Some(revised_prompt) = first.revised_prompt {
        eprintln!("Revised prompt from OpenAI: {}", revised_prompt);
    }

    fs::write(&output_filename, &png_bytes)
        .with_context(|| format!("Failed to write image to {}", output_filename.display()))?;
    eprintln!("Saved: {}", output_filename.display());

    Ok(())
}
