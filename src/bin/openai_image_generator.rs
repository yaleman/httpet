use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose;
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::log::info;

/// Generate witty HTTP status animal images.
///
/// Minimal UX:
///   openai_image_generator goat 418
///
/// If you omit the code, it will pick the next missing code for that animal (prompting first).
#[derive(Parser, Debug)]
#[command(name = "openai_image_generator")]
#[command(
    about = "Generate witty HTTP status animal images via an iterative text->prompt pipeline + Images API"
)]
struct Args {
    /// Animal theme for the image (e.g. goat, dog, cat, wombat, puffin)
    animal: String,

    /// HTTP status code (e.g. 404, 418, 204). If omitted, pick the next missing code.
    code: Option<u16>,

    /// OpenAI API key
    #[arg(required = true, long, env = "OPENAI_API_KEY", hide_env_values = true)]
    openai_api_key: String,

    /// Text model used for gag generation + evaluation + prompt compilation
    #[arg(long, default_value = "gpt-5.2")]
    text_model: String,

    /// Image model
    #[arg(long, default_value = "gpt-image-1.5")]
    image_model: String,

    /// Quality: auto / low / medium / high / hd (hd treated like high)
    #[arg(long, value_enum, default_value_t = Quality::High)]
    quality: Quality,

    /// Output directory (final image goes in <dir>/<animal>/<code>.png)
    #[arg(long, default_value = "./images", env = "HTTPET_IMAGE_DIR")]
    out_dir: PathBuf,

    /// If set, write intermediate gag + prompt + raw API responses to debug_* files
    #[arg(long, default_value_t = true)]
    debug: bool,

    /// Max gag attempts before giving up
    #[arg(long, default_value_t = 4)]
    max_attempts: usize,
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

impl Quality {
    fn as_images_quality(self) -> &'static str {
        match self {
            Quality::Auto => "auto",
            Quality::Low => "low",
            Quality::Medium => "medium",
            Quality::High | Quality::Hd => "high",
        }
    }
}

// -----------------------------
// HTTP code "tone" bias
// -----------------------------

#[derive(Debug, Clone, Copy)]
enum HttpTone {
    Absence,
    Refusal,
    Absurd,
    Overload,
    Failure,
    Neutral,
}

fn classify_http_code(code: u16) -> HttpTone {
    match code {
        204 | 205 | 304 => HttpTone::Absence,
        401 | 403 | 407 | 451 => HttpTone::Refusal,
        418 => HttpTone::Absurd,
        429 => HttpTone::Overload,
        500 | 502 | 503 | 504 | 507 | 508 => HttpTone::Failure,
        _ => HttpTone::Neutral,
    }
}

fn tone_label(t: HttpTone) -> &'static str {
    match t {
        HttpTone::Absence => "Absence (success with nothing returned / anticlimax)",
        HttpTone::Refusal => "Refusal (access denied / not allowed / blocked)",
        HttpTone::Absurd => "Absurd (intentionally nonsensical)",
        HttpTone::Overload => "Overload (rate limiting / back off)",
        HttpTone::Failure => "Failure (things are broken)",
        HttpTone::Neutral => "Neutral",
    }
}

// -----------------------------
// Responses API (text)
// -----------------------------

static API_RESPONSE_SEQ: AtomicUsize = AtomicUsize::new(0);

fn write_debug(prefix: &str, ext: &str, bytes: &[u8]) -> Result<PathBuf> {
    let seq = API_RESPONSE_SEQ.fetch_add(1, Ordering::Relaxed);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let filename = format!("debug_{prefix}_{ts}_{seq}.{ext}");
    fs::write(&filename, bytes).with_context(|| format!("Failed to write {filename}"))?;
    Ok(PathBuf::from(filename))
}

/// Robustly extract the text output from a /v1/responses JSON payload.
///
/// The API may include a top-level output_text convenience field, but the
/// canonical form is output[].content[].type == "output_text" with a "text" field.
fn extract_responses_output_text(v: &Value) -> Option<String> {
    if let Some(s) = v.get("output_text").and_then(|x| x.as_str()) {
        if !s.trim().is_empty() {
            return Some(s.to_string());
        }
    }

    // Walk output -> content -> output_text blocks
    let output = v.get("output")?.as_array()?;
    let mut parts: Vec<String> = Vec::new();

    for item in output {
        // Some items are messages; others can be tool calls, etc.
        let content = match item.get("content").and_then(|c| c.as_array()) {
            Some(c) => c,
            None => continue,
        };

        for c in content {
            let ctype = c.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if ctype == "output_text" {
                if let Some(text) = c.get("text").and_then(|t| t.as_str()) {
                    parts.push(text.to_string());
                }
            } else if ctype == "text" {
                // Defensive: sometimes you may see plain text blocks
                if let Some(text) = c.get("text").and_then(|t| t.as_str()) {
                    parts.push(text.to_string());
                }
            }
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(""))
    }
}

async fn responses_json_schema<T: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    instructions: &str,
    user_input: Value,
    schema_name: &str,
    schema: Value,
    debug: bool,
) -> Result<(T, Option<PathBuf>, String)> {
    // Structured outputs: text.format.type = "json_schema".
    // https://platform.openai.com/docs/guides/structured-outputs
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

    let debug_path = if debug {
        Some(write_debug("responses", "json", &bytes)?)
    } else {
        None
    };

    if !status.is_success() {
        return Err(anyhow!(
            "OpenAI Responses API error {status}. {}",
            String::from_utf8_lossy(&bytes)
        ));
    }

    let v: Value = serde_json::from_slice(&bytes).with_context(|| {
        if let Some(p) = &debug_path {
            format!(
                "Failed to parse /v1/responses JSON; saved to {}",
                p.display()
            )
        } else {
            "Failed to parse /v1/responses JSON".to_string()
        }
    })?;

    // Some successful Responses payloads include an "error": null field.
    // Only treat it as an error if it is present AND non-null.
    if let Some(err) = v.get("error") {
        if !err.is_null() {
            return Err(anyhow!("OpenAI Responses API returned error: {err}"));
        }
    }

    let output_text = extract_responses_output_text(&v).ok_or_else(|| {
        if let Some(p) = &debug_path {
            anyhow!(
                "/v1/responses missing output text; saved to {}",
                p.display()
            )
        } else {
            anyhow!("/v1/responses missing output text")
        }
    })?;

    let parsed: T = serde_json::from_str(&output_text)
        .with_context(|| format!("Failed to parse structured output JSON: {output_text}"))?;

    Ok((parsed, debug_path, output_text))
}

// -----------------------------
// Pipeline schemas
// -----------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
struct GagSpec {
    core_joke: String,
    attitude: String,
    emotion: String,
    scene: String,
    physical_metaphor: String,
    why_it_matches_http_code: String,
}

#[derive(Debug, Deserialize)]
struct GagEvaluation {
    verdict: String, // accept | reject
    reason: String,
}

#[derive(Debug, Deserialize)]
struct PromptSpec {
    prompt: String,
}

fn gag_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "core_joke": {"type": "string"},
            "attitude": {"type": "string"},
            "emotion": {"type": "string"},
            "scene": {"type": "string"},
            "physical_metaphor": {"type": "string"},
            "why_it_matches_http_code": {"type": "string"}
        },
        "required": [
            "core_joke",
            "attitude",
            "emotion",
            "scene",
            "physical_metaphor",
            "why_it_matches_http_code"
        ]
    })
}

fn gag_eval_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "verdict": {"type": "string", "enum": ["accept", "reject"]},
            "reason": {"type": "string"}
        },
        "required": ["verdict", "reason"]
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

fn animal_constraints(animal: &str) -> &'static str {
    match animal {
        "dog" | "dogs" => "Dogs must be Maltese terriers, toy poodles, or Pomeranians.",
        "cat" | "cats" => "Cats should be Blue Burmese or pure white cats with vivid blue eyes.",
        "puffin" | "puffins" => "Puffins are cool birds.",
        _ => "",
    }
}

fn gag_instructions() -> &'static str {
    r#"You generate a single strong visual gag for an illustration representing an HTTP status code using an animal.

Avoid symbolic or ritualistic actions.
Prefer blunt, dismissive, or anticlimactic behavior.
If an object is present, it should feel incidental, not meaningful.

Tone:
- Dry, slightly sarcastic, but funny

Rules:
- One joke only.
- The joke must be understandable without text.
- The gag may imply incompetence, stubbornness, bureaucracy, or apathy.
- No art style decisions.
- No camera, lens, lighting, or rendering decisions.
- No references to memes, pop culture, or existing characters.

Important:
Some HTTP status codes represent intentional absence or non-response (e.g., 204, 304, 205).
For these codes:
- No implied action
- No anticipation
- No emotional payoff

Return JSON that matches the provided schema."#
}

fn evaluator_instructions() -> &'static str {
    r#"You are evaluating a proposed visual gag for an HTTP status illustration.

Reject gags that:
- contradict the HTTP status meaning
- imply emotional payoff where none should exist
- introduce anticipation for absence-based codes (e.g. 204)
- would confuse someone familiar with HTTP semantics

Respond ONLY with JSON that matches the provided schema."#
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

Visual language rules:
- This is a cartoon illustration, not a film still
- Objects may be simplified, exaggerated, or toy-like
- Proportions may be unrealistic if it improves the joke
- Physical plausibility is optional
- If realism conflicts with humor, realism must lose

Text rules:
- The HTTP code must appear subtly and naturally in the scene
- No other readable words allowed

Tone preservation:
- Do not soften, justify, or add warmth to the gag
- Preserve sarcasm, indifference, or hostility implied by the gag
- If the gag implies absence, the image must contain no implied motion, reward, interaction, or pending outcome

Avoid:
- realistic appliances
- cinematic lighting
- photoreal materials
- polished interior design

Prefer:
- simplified shapes
- bold colors
- visual shorthand

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

async fn generate_gag(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    animal: &str,
    code: u16,
    tone: HttpTone,
    debug: bool,
) -> Result<(GagSpec, String)> {
    let user = json!({
        "animal": animal,
        "http_code": code,
        "tone_category": tone_label(tone)
    });

    let (gag, _path, raw_text) = responses_json_schema::<GagSpec>(
        client,
        api_key,
        model,
        gag_instructions(),
        user,
        "gag_spec",
        gag_schema(),
        debug,
    )
    .await
    .context("Gag generation failed")?;

    Ok((gag, raw_text))
}

async fn evaluate_gag(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    animal: &str,
    code: u16,
    tone: HttpTone,
    gag: &GagSpec,
    debug: bool,
) -> Result<(GagEvaluation, String)> {
    let user = json!({
        "animal": animal,
        "http_code": code,
        "tone_category": tone_label(tone),
        "gag": gag
    });

    let (eval, _path, raw_text) = responses_json_schema::<GagEvaluation>(
        client,
        api_key,
        model,
        evaluator_instructions(),
        user,
        "gag_evaluation",
        gag_eval_schema(),
        debug,
    )
    .await
    .context("Gag evaluation failed")?;

    Ok((eval, raw_text))
}

async fn compile_prompt(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    animal: &str,
    code: u16,
    tone: HttpTone,
    gag: &GagSpec,
    debug: bool,
) -> Result<(PromptSpec, String)> {
    let user = json!({
        "animal": animal,
        "http_code": code,
        "tone_category": tone_label(tone),
        "gag": gag
    });

    let (prompt, _path, raw_text) = responses_json_schema::<PromptSpec>(
        client,
        api_key,
        model,
        &director_instructions(animal),
        user,
        "prompt_spec",
        prompt_schema(),
        debug,
    )
    .await
    .context("Prompt compilation failed")?;

    Ok((prompt, raw_text))
}

// -----------------------------
// Images API
// -----------------------------

#[derive(Serialize, Debug)]
struct ImagesGenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    n: u8,
    size: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    quality: Option<&'a str>,
    /// For GPT image models, you can request a specific output format (e.g. png, webp).
    /// The API returns base64 in data[].b64_json.
    #[serde(skip_serializing_if = "Option::is_none")]
    output_format: Option<&'a str>,
}

#[derive(Deserialize, Debug)]
struct ImagesGenerateResponse {
    data: Vec<ImageData>,
}

#[derive(Deserialize, Debug)]
struct ImageData {
    #[serde(default)]
    b64_json: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    revised_prompt: Option<String>,
}

async fn generate_image(
    client: &reqwest::Client,
    api_key: &str,
    image_model: &str,
    prompt: &str,
    quality: Quality,
    debug: bool,
) -> Result<Vec<u8>> {
    let req = ImagesGenerateRequest {
        model: image_model,
        prompt,
        n: 1,
        size: "1024x1024",
        quality: Some(quality.as_images_quality()),
        // GPT image models return base64 in data[].b64_json; request PNG bytes.
        output_format: Some("png"),
    };

    let resp = client
        .post("https://api.openai.com/v1/images/generations")
        .bearer_auth(api_key)
        .json(&req)
        .send()
        .await
        .context("Request to /v1/images/generations failed")?;

    let status = resp.status();
    let bytes = resp.bytes().await.context("Failed reading images body")?;
    if debug {
        let _ = write_debug("images", "json", &bytes);
    }

    if !status.is_success() {
        return Err(anyhow!(
            "OpenAI Images API error {status}: {}",
            String::from_utf8_lossy(&bytes)
        ));
    }

    let parsed: ImagesGenerateResponse =
        serde_json::from_slice(&bytes).context("Failed to parse /v1/images/generations JSON")?;

    let first = parsed
        .data
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("No image data returned"))?;

    if let Some(rp) = first.revised_prompt {
        eprintln!("Revised prompt from model: {rp}");
    }

    if let Some(b64) = first.b64_json {
        let png = general_purpose::STANDARD
            .decode(b64)
            .context("Failed to base64-decode PNG")?;
        Ok(png)
    } else if let Some(url) = first.url {
        let png = client
            .get(url)
            .send()
            .await
            .context("Failed to download image")?
            .bytes()
            .await
            .context("Failed to read downloaded image")?;
        Ok(png.to_vec())
    } else {
        Err(anyhow!("Image response missing b64_json and url"))
    }
}

// -----------------------------
// Status code selection helpers (optional)
// -----------------------------

fn load_status_codes() -> Result<Vec<u16>> {
    // Minimal embedded list so you don't need external crates.
    // If you already have a status code list elsewhere, replace this.
    let mut v: Vec<u16> = (100..600).collect();
    // Keep it sensible: only known-ish codes if desired.
    // For now, just return the range.
    v.sort_unstable();
    Ok(v)
}

fn existing_codes_for(animal: &str, out_dir: &PathBuf) -> Result<std::collections::HashSet<u16>> {
    let mut set = std::collections::HashSet::new();
    let dir = out_dir.join(animal);
    if !dir.exists() {
        return Ok(set);
    }
    for entry in fs::read_dir(dir).context("Failed to read output dir")? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("png") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            if let Ok(code) = stem.parse::<u16>() {
                set.insert(code);
            }
        }
    }
    Ok(set)
}

fn confirm_next_code(animal: &str, code: u16) -> Result<bool> {
    eprint!(
        "No code provided. Next missing for '{animal}' appears to be {code}. Generate it? [y/N] "
    );
    io::stderr().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

// -----------------------------
// Main
// -----------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let animal = args.animal.to_ascii_lowercase();

    let status_code = match args.code {
        Some(c) => c,
        None => {
            let codes = load_status_codes()?;
            let existing = existing_codes_for(&animal, &args.out_dir)?;
            let next = codes.into_iter().find(|c| !existing.contains(c));
            let Some(code) = next else {
                return Err(anyhow!("No missing status codes found for {animal}"));
            };
            if !confirm_next_code(&animal, code)? {
                return Err(anyhow!("Aborted"));
            }
            code
        }
    };

    let output_filename = args
        .out_dir
        .join(&animal)
        .join(format!("{status_code}.png"));
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

    let tone = classify_http_code(status_code);

    info!(
        "Generating: animal={animal}, code={status_code}, tone={}, text_model={}, image_model={}, out={}",
        tone_label(tone),
        args.text_model,
        args.image_model,
        output_filename.display()
    );

    let client = reqwest::Client::new();

    // Stage 1 + 1.5: iterate gags until accepted.
    let mut chosen_gag: Option<GagSpec> = None;
    let mut chosen_prompt: Option<String> = None;

    for attempt in 1..=args.max_attempts {
        let (gag, gag_raw) = generate_gag(
            &client,
            &args.openai_api_key,
            &args.text_model,
            &animal,
            status_code,
            tone,
            args.debug,
        )
        .await?;

        if args.debug {
            fs::write("debug_gag.json", &gag_raw).ok();
        }

        let (eval, eval_raw) = evaluate_gag(
            &client,
            &args.openai_api_key,
            &args.text_model,
            &animal,
            status_code,
            tone,
            &gag,
            args.debug,
        )
        .await?;

        if args.debug {
            fs::write("debug_eval.json", &eval_raw).ok();
        }

        if eval.verdict == "accept" {
            eprintln!("Accepted gag on attempt {attempt}.");

            let (prompt_spec, prompt_raw) = compile_prompt(
                &client,
                &args.openai_api_key,
                &args.text_model,
                &animal,
                status_code,
                tone,
                &gag,
                args.debug,
            )
            .await?;

            if args.debug {
                fs::write("debug_compiled_prompt.json", &prompt_raw).ok();
                fs::write("debug_compiled_prompt.txt", &prompt_spec.prompt).ok();
            }

            chosen_gag = Some(gag);
            chosen_prompt = Some(prompt_spec.prompt);
            break;
        } else {
            eprintln!("Rejected gag attempt {attempt}: {}", eval.reason);
        }
    }

    let gag =
        chosen_gag.ok_or_else(|| anyhow!("Failed to produce an acceptable gag after retries"))?;
    let prompt = chosen_prompt.ok_or_else(|| anyhow!("Missing compiled prompt"))?;

    // Stage 3: render
    let png_bytes = generate_image(
        &client,
        &args.openai_api_key,
        &args.image_model,
        &prompt,
        args.quality,
        args.debug,
    )
    .await?;

    fs::write(&output_filename, &png_bytes)
        .with_context(|| format!("Failed to write image to {}", output_filename.display()))?;

    eprintln!("Saved: {}", output_filename.display());

    // Optional: store the gag spec next to it for later auditing
    if args.debug {
        let meta_path = output_filename.with_extension("gag.json");
        let _ = fs::write(
            &meta_path,
            serde_json::to_string_pretty(&gag).unwrap_or_default(),
        );
    }

    Ok(())
}
