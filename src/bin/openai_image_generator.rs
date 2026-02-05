use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose;
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::{Duration, timeout};
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

static START_TIMESTAMP: LazyLock<u64> = LazyLock::new(|| {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
});

/// For debugging: write raw API responses to files with a unique name.
fn write_debug(prefix: &str, ext: &str, bytes: &[u8]) -> Result<PathBuf> {
    let seq = API_RESPONSE_SEQ.fetch_add(1, Ordering::Relaxed);
    let filename = format!("debug_{prefix}_{ts}_{seq}.{ext}", ts = *START_TIMESTAMP);
    fs::write(&filename, bytes).with_context(|| format!("Failed to write {filename}"))?;
    Ok(PathBuf::from(filename))
}

/// Robustly extract the text output from a /v1/responses JSON payload.
///
/// The API may include a top-level output_text convenience field, but the
/// canonical form is output[].content[].type == "output_text" with a "text" field.
fn extract_responses_output_text(v: &Value) -> Option<String> {
    if let Some(s) = v.get("output_text").and_then(|x| x.as_str())
        && !s.trim().is_empty()
    {
        return Some(s.to_string());
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

#[allow(clippy::too_many_arguments)]
async fn responses_json_schema<T: for<'de> Deserialize<'de>>(
    args: &Args,
    client: &reqwest::Client,
    model: &str,
    instructions: &str,
    user_input: Value,
    schema_name: &str,
    schema: Value,
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
        .bearer_auth(&args.openai_api_key)
        .json(&req_body)
        .send()
        .await
        .context("Request to /v1/responses failed")?;

    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .context("Failed reading /v1/responses body")?;

    let debug_path = if args.debug {
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
    if let Some(err) = v.get("error")
        && !err.is_null()
    {
        return Err(anyhow!("OpenAI Responses API returned error: {err}"));
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

#[derive(Debug, Deserialize)]
struct FunScore {
    score: i32, // 1..5
    reason: String,
}

fn fun_score_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "score": {"type": "integer", "minimum": 1, "maximum": 5},
            "reason": {"type": "string"}
        },
        "required": ["score", "reason"]
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

Tone:
- Dry
- Slightly sarcastic
- Mildly contemptuous of the situation
- Never wholesome, never cute for its own sake

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

Also:
- Avoid symbolic or ritualistic actions.
- Prefer blunt, dismissive, or anticlimactic behavior.
- If an object is present, it should feel incidental, not meaningful.

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

fn fun_evaluator_instructions() -> &'static str {
    r#"You are judging proposed visual gag ideas for humor and memorability for an HTTP status animal illustration.

Prefer ideas that:
- exaggerate the situation beyond realism
- use visual shorthand and a clear, punchy gag
- escalate the scenario (within the boundaries of the HTTP meaning)
- would be funny even if the viewer doesn't know HTTP

Avoid ideas that are:
- calm, tasteful, minimal-for-its-own-sake
- merely correct without being entertaining
- "product photo" scenes or realistic daily life without a twist

Respond ONLY with JSON that matches the provided schema."#
}

fn director_instructions(animal: &str) -> String {
    let mut s = String::new();
    s.push_str(
        r#"You are an art director generating prompts for a funny HTTP-status cartoon illustration.

House style (default):
- Square 1:1
- Bold, playful cartoon illustration (NOT a cinematic film still)
- Simple shapes, exaggerated expressions, visual shorthand
- Clean readability, but not "polished" or "realistic"
- Physical plausibility is optional; humor wins
- Prefer one clear gag; minimal clutter

Visual language rules:
- Avoid realistic appliances, realistic interiors, photoreal materials, and "product photo" vibes
- Prefer toy-like props, simplified backgrounds, and exaggerated proportions
- Use expressive faces and poses; the emotion should read instantly

Text rules:
- The HTTP code number must appear subtly and naturally in the scene (tag, label, tiny sign, badge)
- No other readable words allowed (ONLY the number)

Tone preservation:
- Do not soften, justify, or add warmth to the gag
- Preserve sarcasm, indifference, petty refusal, or annoyance implied by the gag
- For absence codes (e.g., 204/304/205), no implied action, anticipation, reward, or payoff

Hard avoid:
- watermarks, logos, brand marks
- UI overlays
- extra text (only the HTTP number)
- messy backgrounds that distract from the gag
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
    args: &Args,
    client: &reqwest::Client,
    model: &str,
    code: u16,
    tone: HttpTone,
) -> Result<(GagSpec, String)> {
    eprintln!(
        "Generating gag for attempt with tone {}...",
        tone_label(tone)
    );
    let user = json!({
        "animal": args.animal,
        "http_code": code,
        "tone_category": tone_label(tone)
    });

    let (gag, _path, raw_text) = responses_json_schema::<GagSpec>(
        args,
        client,
        model,
        gag_instructions(),
        user,
        "gag_spec",
        gag_schema(),
    )
    .await
    .context("Gag generation failed")?;

    Ok((gag, raw_text))
}

async fn evaluate_gag(
    args: &Args,
    client: &reqwest::Client,
    user_input: UserInput<'_>,
) -> Result<(GagEvaluation, String)> {
    let (eval, _path, raw_text) = responses_json_schema::<GagEvaluation>(
        args,
        client,
        &args.text_model,
        evaluator_instructions(),
        user_input.as_json(),
        "gag_evaluation",
        gag_eval_schema(),
    )
    .await
    .context("Gag evaluation failed")?;

    Ok((eval, raw_text))
}

async fn score_fun(
    args: &Args,
    client: &reqwest::Client,
    user_input: UserInput<'_>,
) -> Result<(FunScore, String)> {
    let (score, _path, raw_text) = responses_json_schema::<FunScore>(
        args,
        client,
        &args.text_model,
        fun_evaluator_instructions(),
        user_input.as_json(),
        "fun_score",
        fun_score_schema(),
    )
    .await
    .context("Fun scoring failed")?;

    Ok((score, raw_text))
}

struct UserInput<'a> {
    animal: &'a str,
    status_code: u16,
    tone: HttpTone,
    gag: &'a GagSpec,
}

impl UserInput<'_> {
    fn as_json(&self) -> Value {
        json!({
            "animal": self.animal,
            "http_code": self.status_code,
            "tone_category": tone_label(self.tone),
            "gag": self.gag
        })
    }
}

async fn compile_prompt(
    args: &Args,
    client: &reqwest::Client,
    user_input: UserInput<'_>,
) -> Result<(PromptSpec, String)> {
    let user = user_input.as_json();

    let (prompt, _path, raw_text) = responses_json_schema::<PromptSpec>(
        args,
        client,
        &args.text_model,
        &director_instructions(user_input.animal),
        user,
        "prompt_spec",
        prompt_schema(),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    output_format: Option<&'a str>, // e.g. "png"
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

fn existing_codes_for(animal: &str, out_dir: &Path) -> Result<std::collections::HashSet<u16>> {
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
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            && let Ok(code) = stem.parse::<u16>()
        {
            set.insert(code);
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

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .context("Failed to generate reqwest client")?;

    // Stage 1 + 1.5 + 1.6:
    // Generate multiple gag candidates, reject semantically-wrong ones, then pick the funniest.
    let mut accepted: Vec<(GagSpec, FunScore)> = Vec::new();

    // Aim for at least 3 candidates for variety.
    let target_candidates = std::cmp::max(3, args.max_attempts);

    for attempt in 1..=target_candidates {
        let (gag, gag_raw) = timeout(
            Duration::from_secs(45),
            generate_gag(&args, &client, &animal, status_code, tone),
        )
        .await??;

        if args.debug {
            let _ = fs::write(format!("debug_gag_{attempt}.json"), &gag_raw);
        }

        let (eval, eval_raw) = timeout(
            Duration::from_secs(45),
            evaluate_gag(
                &args,
                &client,
                UserInput {
                    animal: &animal,
                    status_code,
                    tone,
                    gag: &gag,
                },
            ),
        )
        .await??;

        if args.debug {
            let _ = fs::write(format!("debug_eval_{attempt}.json"), &eval_raw);
        }

        if eval.verdict != "accept" {
            eprintln!("Rejected gag attempt {attempt}: {}", eval.reason);
            continue;
        }

        // Fun score (higher is better). This is the missing "taste" bias.
        let user_input = UserInput {
            animal: &animal,
            status_code,
            tone,
            gag: &gag,
        };
        let (fun, fun_raw) = score_fun(&args, &client, user_input).await?;

        if args.debug {
            let _ = fs::write(format!("debug_fun_{attempt}.json"), &fun_raw);
        }

        eprintln!(
            "Accepted gag attempt {attempt} with fun score {}: {}",
            fun.score, fun.reason
        );
        accepted.push((gag, fun));
    }

    if accepted.is_empty() {
        return Err(anyhow!(
            "Failed to produce any acceptable gags after {target_candidates} attempts"
        ));
    }

    // Pick the highest fun score. If tied, keep the first (deterministic).
    accepted.sort_by(|a, b| b.1.score.cmp(&a.1.score));
    let (gag, best_fun) = accepted
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("Unexpected empty accepted list"))?;

    eprintln!("Selected gag with fun score {}.", best_fun.score);

    // Stage 2: compile the final image prompt once from the chosen gag.
    let (prompt_spec, prompt_raw) = compile_prompt(
        &args,
        &client,
        UserInput {
            animal: &animal,
            status_code,
            tone,
            gag: &gag,
        },
    )
    .await?;

    if args.debug {
        let _ = fs::write("debug_compiled_prompt.json", &prompt_raw);
        let _ = fs::write("debug_compiled_prompt.txt", &prompt_spec.prompt);
    }

    let prompt = prompt_spec.prompt;

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
