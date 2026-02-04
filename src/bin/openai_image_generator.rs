use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose;
use clap::Parser;
use httpet::status_codes::STATUS_CODES;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Generate witty HTTP status animal images.
///
/// Minimal UX:
///   openai_image_generator goat 418
#[derive(Parser, Debug)]
#[command(name = "openai_image_generator")]
#[command(
    about = "Generate witty HTTP status animal images via a 2-step text->prompt pipeline + Images API"
)]
struct Args {
    /// Animal theme for the image (e.g. goat, dog, cat, wombat, puffin)
    animal: String,

    /// HTTP status code (e.g. 404, 418, 204)
    code: Option<u16>,

    /// OpenAI API key
    #[arg(required = true, long, env = "OPENAI_API_KEY", hide_env_values = true)]
    openai_api_key: String,

    /// Text model used for gag generation + prompt compilation
    #[arg(long, default_value = "gpt-5.2")]
    text_model: String,

    /// Image model
    #[arg(long, default_value = "gpt-image-1.5")]
    image_model: String,

    /// Output directory (final image goes in <dir>/<animal>/<code>.png)
    #[arg(long, default_value = "./images", env = "HTTPET_IMAGE_DIR")]
    out_dir: PathBuf,

    /// If set, write the intermediate gag + prompt to ./debug_* files
    #[arg(long, default_value_t = true)]
    debug: bool,
}

// -----------------------------
// Responses API (text)
// -----------------------------

#[derive(Debug, Deserialize, Serialize)]
struct ResponsesCreateResponse {
    #[serde(default)]
    output_text: Option<String>,
    #[serde(default)]
    output: Vec<ResponseOutputItem>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ResponseOutputItem {
    #[serde(default)]
    content: Vec<ResponseContentItem>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
enum ResponseContentItem {
    #[serde(rename = "output_text")]
    OutputText { text: String },
    #[serde(other)]
    Other,
}

fn write_api_response(prefix: &str, ext: &str, bytes: &[u8]) -> Result<PathBuf> {
    static API_RESPONSE_SEQ: AtomicUsize = AtomicUsize::new(0);
    let seq = API_RESPONSE_SEQ.fetch_add(1, Ordering::Relaxed);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let filename = format!("debug_{prefix}_{ts}_{seq}.{ext}");
    fs::write(&filename, bytes).with_context(|| format!("Failed to write {filename}"))?;
    Ok(PathBuf::from(filename))
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct GagSpec {
    core_joke: String,
    emotion: String,
    scene: String,
    physical_metaphor: String,
    why_it_matches_http_code: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct PromptSpec {
    prompt: String,
}

async fn responses_json_schema<T: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    instructions: &str,
    user_input: Value,
    schema_name: &str,
    schema: Value,
) -> Result<T> {
    // Structured outputs: text.format.type = "json_schema".
    // Docs: https://platform.openai.com/docs/guides/structured-outputs
    // Create response endpoint: https://platform.openai.com/docs/api-reference/responses
    let req_body = json!({
        "model": model,
        "instructions": instructions,
        "input": [
            {"role": "user", "content": [{"type": "input_text", "text": user_input.to_string()}]}
        ],
        "text": {
            "format": {
                "type": "json_schema",
                "name": schema_name,
                "strict": true,
                "schema": schema
            }
        }
    });

    let resp = client
        .post("https://api.openai.com/v1/responses")
        .bearer_auth(api_key)
        .json(&req_body)
        .send()
        .await
        .context("Request to /v1/responses failed")?;

    let status = resp.status();

    let bytes = resp
        .bytes()
        .await
        .context("Failed reading /v1/responses body")?;

    let debug_path = write_api_response("responses", "json", &bytes)?;
    if !status.is_success() {
        return Err(anyhow!(
            "OpenAI Responses API error {status}; response saved to {}: {}",
            debug_path.display(),
            String::from_utf8_lossy(&bytes)
        ));
    }

    let parsed: ResponsesCreateResponse = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "Failed to parse /v1/responses JSON, response saved to {}",
            debug_path.display()
        )
    })?;
    if let Some(err) = parsed.error {
        return Err(anyhow!(
            "OpenAI Responses API returned error, response saved to {}: {err}",
            debug_path.display()
        ));
    }

    let output_text = parsed
        .output_text
        .or_else(|| {
            parsed
                .output
                .iter()
                .flat_map(|item| item.content.iter())
                .find_map(|content| {
                    if let ResponseContentItem::OutputText { text } = content {
                        Some(text.clone())
                    } else {
                        None
                    }
                })
        })
        .ok_or_else(|| {
            anyhow!(
                "/v1/responses missing output_text, response saved to {}",
                debug_path.display()
            )
        })?;

    serde_json::from_str(&output_text)
        .with_context(|| format!("Failed to parse structured output JSON: {output_text}"))
}

// -----------------------------
// Images API
// -----------------------------

/// Request body for POST /v1/images/generations
/// Docs: https://platform.openai.com/docs/api-reference/images
#[derive(Serialize, Debug)]
struct ImagesGenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    n: u8,
    size: &'a str,

    // For GPT image models.
    #[serde(skip_serializing_if = "Option::is_none")]
    quality: Option<&'a str>,

    #[serde(skip_serializing_if = "Option::is_none")]
    output_format: Option<&'a str>,

    // For dall-e models.
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<&'a str>,

    #[serde(skip_serializing_if = "Option::is_none")]
    style: Option<&'a str>,
}

#[derive(Deserialize, Debug)]
struct ImagesGenerateResponse {
    data: Vec<ImageData>,
}

#[derive(Deserialize, Debug)]
struct ImageData {
    b64_json: Option<String>,
    url: Option<String>,
    revised_prompt: Option<String>,
}

async fn generate_image(
    client: &reqwest::Client,
    api_key: &str,
    image_model: &str,
    prompt: &str,
) -> Result<Vec<u8>> {
    // GPT image models always return base64, and support output_format.
    // DALL·E models can return url or b64_json.
    // Docs: https://platform.openai.com/docs/api-reference/images
    let is_gpt_image = image_model.starts_with("gpt-image");

    let req_body = if is_gpt_image {
        ImagesGenerateRequest {
            model: image_model,
            prompt,
            n: 1,
            size: "1024x1024",
            quality: Some("high"),
            output_format: Some("png"),
            response_format: None,
            style: None,
        }
    } else if image_model == "dall-e-3" {
        ImagesGenerateRequest {
            model: image_model,
            prompt,
            n: 1,
            size: "1024x1024",
            quality: Some("hd"),
            output_format: None,
            response_format: Some("b64_json"),
            style: Some("natural"),
        }
    } else {
        // dall-e-2 etc
        ImagesGenerateRequest {
            model: image_model,
            prompt,
            n: 1,
            size: "1024x1024",
            quality: None,
            output_format: None,
            response_format: Some("b64_json"),
            style: None,
        }
    };

    let resp = client
        .post("https://api.openai.com/v1/images/generations")
        .bearer_auth(api_key)
        .json(&req_body)
        .send()
        .await
        .context("Request to /v1/images/generations failed")?;

    let status = resp.status();
    let resp_bytes = resp
        .bytes()
        .await
        .context("Failed reading /v1/images/generations body")?;
    let debug_path = write_api_response("images_generate", "json", &resp_bytes)?;
    if !status.is_success() {
        return Err(anyhow!(
            "OpenAI Images API error {status}; response saved to {}: {}",
            debug_path.display(),
            String::from_utf8_lossy(&resp_bytes)
        ));
    }

    let parsed: ImagesGenerateResponse =
        serde_json::from_slice(&resp_bytes).with_context(|| {
            format!(
                "Failed to parse /v1/images/generations JSON, response saved to {}",
                debug_path.display()
            )
        })?;

    let first = parsed
        .data
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("No image data returned"))?;

    if let Some(revised_prompt) = first.revised_prompt {
        eprintln!("Revised prompt from OpenAI: {revised_prompt}");
    }

    if let Some(b64_json) = first.b64_json {
        let bytes = general_purpose::STANDARD
            .decode(b64_json)
            .context("Failed to base64-decode image")?;
        Ok(bytes)
    } else if let Some(url) = first.url {
        let resp = client
            .get(url)
            .send()
            .await
            .context("Failed to download image URL")?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .context("Failed to read downloaded image bytes")?;
        let debug_path = write_api_response("image_download", "bin", &bytes)?;
        if !status.is_success() {
            return Err(anyhow!(
                "Image download error {status}; response saved to {}",
                debug_path.display()
            ));
        }
        Ok(bytes.to_vec())
    } else {
        Err(anyhow!("Image response missing b64_json and url fields"))
    }
}

// -----------------------------
// Prompt pipeline
// -----------------------------

fn animal_constraints(animal: &str) -> &'static str {
    match animal {
        "dog" | "dogs" => "Dogs must be Maltese terriers, toy poodles, or Pomeranians.",
        "cat" | "cats" => "Cats should be Blue Burmese or pure white cats with vivid blue eyes.",
        "puffin" | "puffins" => "Puffins are cool birds.",
        _ => "",
    }
}

fn gag_instructions() -> &'static str {
    // Keep it simple: one gag, no art direction.
    r#"You generate a single strong visual gag for an illustration representing an HTTP status code using an animal.

Rules:
- One joke only.
- The joke must be understandable without text.
- No art style decisions.
- No camera, lens, lighting, or rendering decisions.
- No references to memes, pop culture, or existing characters.
- Keep it simple and visual.

Return JSON that matches the provided schema."#
}

fn director_instructions(animal: &str) -> String {
    let mut s = String::new();
    s.push_str(
        r#"You are an art director generating prompts for a high-quality stylized 3D animated illustration.

House style (never change):
- Square 1:1
- Stylized 3D animated film still
- Cinematic but clean
- Strong visual clarity
- Minimalism preferred
- One readable prop at most
- No clutter

Text rules:
- The HTTP code must appear subtly and naturally in the scene
- No other readable words allowed

Hard avoid:
- watermarks, logos, brand marks
- UI overlays
- extra text (only the HTTP code)
- messy backgrounds
- weird anatomy or extra limbs

Return JSON that matches the provided schema."#,
    );

    let c = animal_constraints(animal);
    if !c.is_empty() {
        s.push_str("\n\nAnimal constraints:\n");
        s.push_str(c);
    }

    s
}

fn gag_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "core_joke": {"type": "string"},
            "emotion": {"type": "string"},
            "scene": {"type": "string"},
            "physical_metaphor": {"type": "string"},
            "why_it_matches_http_code": {"type": "string"}
        },
        "required": [
            "core_joke",
            "emotion",
            "scene",
            "physical_metaphor",
            "why_it_matches_http_code"
        ]
    })
}

fn prompt_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "prompt": {"type": "string"}
        },
        "required": ["prompt"]
    })
}

async fn build_image_prompt(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    animal: &str,
    code: u16,
) -> Result<(String, GagSpec)> {
    let animal_lc = animal.to_ascii_lowercase();

    // Stage 1: gag generation.
    let gag_input = json!({"animal": animal_lc, "http_code": code});
    let gag: GagSpec = responses_json_schema(
        client,
        api_key,
        model,
        gag_instructions(),
        gag_input,
        "gag_spec",
        gag_schema(),
    )
    .await
    .context("Gag generation failed")?;

    // Stage 2: compile into an image prompt (still structured, but schema is just {prompt}).
    let director_input = json!({
        "animal": animal_lc,
        "http_code": code,
        "gag": gag.clone()
    });

    let prompt_spec: PromptSpec = responses_json_schema(
        client,
        api_key,
        model,
        &director_instructions(&animal_lc),
        director_input,
        "image_prompt",
        prompt_schema(),
    )
    .await
    .context("Prompt compilation failed")?;

    Ok((prompt_spec.prompt.trim().to_string(), gag))
}

fn existing_codes_for(animal: &str) -> Result<std::collections::HashSet<u16>> {
    let mut existing = std::collections::HashSet::new();
    let dir = PathBuf::from(format!("./images/{animal}"));
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(existing),
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

// -----------------------------
// Main
// -----------------------------

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

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let animal = args.animal.to_ascii_lowercase();
    let code = match args.code {
        Some(c) => c,
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

    let output_filename = args.out_dir.join(&animal).join(format!("{code}.png"));

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

    let client = reqwest::Client::new();

    // Build prompt via 2-step pipeline.
    let (compiled_prompt, gag) = build_image_prompt(
        &client,
        &args.openai_api_key,
        &args.text_model,
        &animal,
        code,
    )
    .await?;

    if args.debug {
        fs::write(
            "debug_gag.json",
            serde_json::to_vec_pretty(&gag).unwrap_or_default(),
        )
        .ok();
        fs::write("debug_compiled_prompt.txt", &compiled_prompt).ok();
    }

    // Render image.
    let png_bytes = generate_image(
        &client,
        &args.openai_api_key,
        &args.image_model,
        &compiled_prompt,
    )
    .await?;

    fs::write(&output_filename, &png_bytes)
        .with_context(|| format!("Failed to write {}", output_filename.display()))?;

    eprintln!("Saved: {}", output_filename.display());
    Ok(())
}
