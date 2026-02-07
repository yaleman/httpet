use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose;
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::future::Future;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::task::JoinSet;
use tokio::time::{Duration, sleep, timeout};
use tracing::log::{debug, info, warn};

use httpet::config;
use httpet::status_codes::STATUS_CODES;

/// Generate witty HTTP status animal images.
///
/// Minimal UX:
///   openai_image_generator goat 418
///
/// If you omit the code, it will pick the next missing code for that animal (prompting first).
#[derive(Parser, Debug, Clone)]
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

    /// Timeout (seconds) for Responses API calls
    #[arg(long, default_value_t = 90)]
    responses_timeout_secs: u64,

    /// Retries for Responses API calls (non-image)
    #[arg(long, default_value_t = 2)]
    responses_retries: usize,

    /// Backoff base in milliseconds between Responses retries (non-image)
    #[arg(long, default_value_t = 750)]
    responses_backoff_ms: u64,

    /// Timeout (seconds) for Images API calls
    #[arg(long, default_value_t = 120)]
    images_timeout_secs: u64,
}

#[derive(Clone, Debug)]
struct StatusContext {
    name: String,
    summary: String,
}

fn status_context(code: u16) -> StatusContext {
    if let Some(info) = STATUS_CODES.get(&code) {
        StatusContext {
            name: info.name.clone(),
            summary: info.summary.clone(),
        }
    } else {
        warn!("Missing status code metadata for {code}; using fallback");
        StatusContext {
            name: "Unknown Status".to_string(),
            summary: "No summary available.".to_string(),
        }
    }
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
static DEBUG_RUN_PREFIX: OnceLock<String> = OnceLock::new();

static START_TIMESTAMP: LazyLock<u64> = LazyLock::new(|| {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
});

/// For debugging: write raw API responses to files with a unique name.
fn write_debug(prefix: &str, ext: &str, bytes: &[u8]) -> Result<PathBuf> {
    let seq = API_RESPONSE_SEQ.fetch_add(1, Ordering::Relaxed);
    let run_prefix = debug_run_prefix()?;
    let filename = format!("{run_prefix}_{prefix}_{seq}.{ext}");
    let path = debug_dir()?.join(filename);
    fs::write(&path, bytes).with_context(|| format!("Failed to write {}", path.display()))?;
    info!("Wrote debug file {}", path.display());
    Ok(path)
}

fn debug_run_prefix() -> Result<&'static str> {
    DEBUG_RUN_PREFIX
        .get()
        .map(|s| s.as_str())
        .ok_or_else(|| anyhow!("Debug prefix not initialized"))
}

fn debug_dir() -> Result<PathBuf> {
    let dir = PathBuf::from("debug");
    if !dir.exists() {
        fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;
        info!("Created debug directory {}", dir.display());
    }
    Ok(dir)
}

fn debug_file_path(label: &str, ext: &str) -> Result<PathBuf> {
    let run_prefix = debug_run_prefix()?;
    Ok(debug_dir()?.join(format!("{run_prefix}_{label}.{ext}")))
}

fn init_debug_prefix(animal: &str, code: u16) -> String {
    let prefix = format!("{ts}_{animal}_{code}", ts = *START_TIMESTAMP);
    let _ = DEBUG_RUN_PREFIX.set(prefix.clone());
    prefix
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

async fn with_timeout<T, F>(label: &str, duration: Duration, fut: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    let start = Instant::now();
    match timeout(duration, fut).await {
        Ok(res) => {
            info!("{label} completed in {}ms", start.elapsed().as_millis());
            res
        }
        Err(_) => {
            warn!("{label} timed out after {}s", duration.as_secs());
            Err(anyhow!(TimeoutError::new(label, duration)))
        }
    }
}

#[derive(Debug)]
struct TimeoutError {
    label: String,
    duration: Duration,
}

impl TimeoutError {
    fn new(label: &str, duration: Duration) -> Self {
        Self {
            label: label.to_string(),
            duration,
        }
    }
}

impl std::fmt::Display for TimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} timed out after {}s",
            self.label,
            self.duration.as_secs()
        )
    }
}

impl std::error::Error for TimeoutError {}

async fn with_retries<T, F, Fut>(
    label: &str,
    duration: Duration,
    max_retries: usize,
    backoff: Duration,
    mut op: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let attempts = max_retries + 1;
    for attempt in 1..=attempts {
        info!("{label} attempt {attempt}/{attempts}");
        match with_timeout(label, duration, op()).await {
            Ok(value) => return Ok(value),
            Err(err) => {
                let is_timeout = err.downcast_ref::<TimeoutError>().is_some();
                if !is_timeout {
                    warn!("{label} failed without timeout: {err}");
                    return Err(err);
                }
                if attempt >= attempts {
                    return Err(err);
                }
                let base_ms = backoff.as_millis().min(u128::from(u64::MAX)) as u64;
                let shift = (attempt - 1) as u32;
                let multiplier = if shift >= 63 { u64::MAX } else { 1u64 << shift };
                let delay_ms = base_ms.saturating_mul(multiplier);
                let delay = Duration::from_millis(delay_ms);
                warn!(
                    "{label} timed out, retrying in {}ms (attempt {}/{})",
                    delay_ms,
                    attempt + 1,
                    attempts
                );
                sleep(delay).await;
            }
        }
    }
    Err(anyhow!("{label} failed after {attempts} attempts"))
}

#[allow(clippy::too_many_arguments)]
async fn responses_json_schema<T: for<'de> Deserialize<'de>>(
    args: &Args,
    client: &reqwest::Client,
    instructions: &str,
    user_input: Value,
    schema_name: &str,
    schema: Value,
) -> Result<(T, Option<PathBuf>, String)> {
    // Structured outputs: text.format.type = "json_schema".
    // https://platform.openai.com/docs/guides/structured-outputs
    info!(
        "Responses API request: model={}, schema={schema_name}",
        args.text_model
    );
    let req_body = json!({
        "model": args.text_model,
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

    let debug_path = Some(write_debug("responses", "json", &bytes)?);
    info!(
        "Responses API status={}, bytes={}, saved={}",
        status,
        bytes.len(),
        debug_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "none".to_string())
    );

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
- You are given the status code name and summary. The gag must convey a humorous or absurd situation caused or inspired by that meaning.
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
- fail to be caused or inspired by the provided status name or summary

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

The prompt must convey a humorous or absurd situation caused or inspired by the provided status name and summary.

House style (default):
- Square 1:1
- Semi-realistic 3D illustration (Pixar-ish, NOT a cinematic film still)
- Dramatic lighting with strong highlights and soft shadows
- Expressive faces and readable silhouettes, but with richer materials
- Clean readability, polished 3D shading, not photoreal
- Physical plausibility is optional; humor wins
- Prefer one clear gag; minimal clutter

Visual language rules:
- Avoid realistic appliances, realistic interiors, and "product photo" vibes
- Prefer tactile 3D props, simplified backgrounds, and exaggerated proportions
- Use expressive faces and poses; the emotion should read instantly

Text rules:
- The HTTP code number must appear subtly and naturally in the scene (tag, label, tiny sign, badge)
- You may include one short status label derived from the status name (e.g., "Forbidden", "Not Found")
- No other readable words allowed beyond the HTTP number and that single short label

Tone preservation:
- Do not soften, justify, or add warmth to the gag
- Preserve sarcasm, indifference, petty refusal, or annoyance implied by the gag
- For absence codes (e.g., 204/304/205), no implied action, anticipation, reward, or payoff

Hard avoid:
- watermarks, logos, brand marks
- UI overlays
- extra text beyond the HTTP number and the single short status label
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
    code: u16,
    tone: HttpTone,
    status: &StatusContext,
) -> Result<(GagSpec, String)> {
    info!(
        "Generating gag: model={}, code={code}, tone={}",
        args.text_model,
        tone_label(tone)
    );
    let user = json!({
        "animal": args.animal,
        "http_code": code,
        "status_name": status.name.as_str(),
        "status_summary": status.summary.as_str(),
        "tone_category": tone_label(tone)
    });

    let (gag, _path, raw_text) = responses_json_schema::<GagSpec>(
        args,
        client,
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
    info!(
        "Evaluating gag for code={} tone={}",
        user_input.status_code,
        tone_label(user_input.tone)
    );
    let (eval, _path, raw_text) = responses_json_schema::<GagEvaluation>(
        args,
        client,
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
    info!(
        "Scoring fun for code={} tone={}",
        user_input.status_code,
        tone_label(user_input.tone)
    );
    let (score, _path, raw_text) = responses_json_schema::<FunScore>(
        args,
        client,
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
    status_name: &'a str,
    status_summary: &'a str,
    tone: HttpTone,
    gag: &'a GagSpec,
}

impl UserInput<'_> {
    fn as_json(&self) -> Value {
        json!({
            "animal": self.animal,
            "http_code": self.status_code,
            "status_name": self.status_name,
            "status_summary": self.status_summary,
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
    info!(
        "Compiling prompt for code={} tone={}",
        user_input.status_code,
        tone_label(user_input.tone)
    );
    let user = user_input.as_json();

    let (prompt, _path, raw_text) = responses_json_schema::<PromptSpec>(
        args,
        client,
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

async fn generate_image(args: &Args, client: &reqwest::Client, prompt: &str) -> Result<Vec<u8>> {
    info!(
        "Generating image: model={}, quality={}, prompt='{}'",
        args.image_model,
        args.quality.as_images_quality(),
        prompt
    );
    let req = ImagesGenerateRequest {
        model: &args.image_model,
        prompt,
        n: 1,
        size: "1024x1024",
        quality: Some(args.quality.as_images_quality()),
        // GPT image models return base64 in data[].b64_json; request PNG bytes.
        output_format: Some("png"),
    };

    let resp = client
        .post("https://api.openai.com/v1/images/generations")
        .bearer_auth(&args.openai_api_key)
        .json(&req)
        .send()
        .await
        .context("Request to /v1/images/generations failed")?;

    let status = resp.status();
    let bytes = resp.bytes().await.context("Failed reading images body")?;
    let debug_path = write_debug("images_generate", "json", &bytes)?;
    info!(
        "Images API status={}, bytes={}, saved={}",
        status,
        bytes.len(),
        debug_path.display()
    );

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
        info!("Revised prompt from model: {rp}");
    }

    if let Some(b64) = first.b64_json {
        let png = general_purpose::STANDARD
            .decode(b64)
            .context("Failed to base64-decode PNG")?;
        Ok(png)
    } else if let Some(url) = first.url {
        info!("Downloading image from url");
        let resp = client
            .get(url)
            .send()
            .await
            .context("Failed to download image")?;
        let status = resp.status();
        let png = resp
            .bytes()
            .await
            .context("Failed to read downloaded image")?;
        let download_path = write_debug("images_download", "png", &png)?;
        info!(
            "Image download status={}, bytes={}, saved={}",
            status,
            png.len(),
            download_path.display()
        );
        if !status.is_success() {
            return Err(anyhow!(
                "OpenAI Images download error {status} (saved to {})",
                download_path.display()
            ));
        }
        Ok(png.to_vec())
    } else {
        Err(anyhow!("Image response missing b64_json and url"))
    }
}

#[allow(clippy::too_many_arguments)]
async fn gag_attempt_pipeline(
    attempt: usize,
    total_attempts: usize,
    args: Args,
    client: reqwest::Client,
    animal: String,
    status: StatusContext,
    status_code: u16,
    tone: HttpTone,
) -> Result<Option<(GagSpec, FunScore)>> {
    info!("Gag attempt {attempt}/{total_attempts} starting");
    let (gag, gag_raw) = with_retries(
        "Gag generation",
        Duration::from_secs(args.responses_timeout_secs),
        args.responses_retries,
        Duration::from_millis(args.responses_backoff_ms),
        || generate_gag(&args, &client, status_code, tone, &status),
    )
    .await?;

    let gag_path = debug_file_path(&format!("gag_attempt_{attempt}"), "json")?;
    fs::write(&gag_path, &gag_raw)
        .with_context(|| format!("Failed to write {}", gag_path.display()))?;
    info!("Wrote gag spec to {}", gag_path.display());
    debug!("Gag core_joke: {}", gag.core_joke);

    let (eval, eval_raw) = with_retries(
        "Gag evaluation",
        Duration::from_secs(args.responses_timeout_secs),
        args.responses_retries,
        Duration::from_millis(args.responses_backoff_ms),
        || {
            evaluate_gag(
                &args,
                &client,
                UserInput {
                    animal: &animal,
                    status_code,
                    status_name: &status.name,
                    status_summary: &status.summary,
                    tone,
                    gag: &gag,
                },
            )
        },
    )
    .await?;

    let eval_path = debug_file_path(&format!("eval_attempt_{attempt}"), "json")?;
    fs::write(&eval_path, &eval_raw)
        .with_context(|| format!("Failed to write {}", eval_path.display()))?;
    info!("Wrote evaluation to {}", eval_path.display());

    if eval.verdict != "accept" {
        warn!("Rejected gag attempt {attempt}: {}", eval.reason);
        return Ok(None);
    }
    info!("Accepted gag attempt {attempt}");

    let (fun, fun_raw) = with_retries(
        "Fun scoring",
        Duration::from_secs(args.responses_timeout_secs),
        args.responses_retries,
        Duration::from_millis(args.responses_backoff_ms),
        || {
            score_fun(
                &args,
                &client,
                UserInput {
                    animal: &animal,
                    status_code,
                    status_name: &status.name,
                    status_summary: &status.summary,
                    tone,
                    gag: &gag,
                },
            )
        },
    )
    .await?;

    let fun_path = debug_file_path(&format!("fun_attempt_{attempt}"), "json")?;
    fs::write(&fun_path, &fun_raw)
        .with_context(|| format!("Failed to write {}", fun_path.display()))?;
    info!("Wrote fun score to {}", fun_path.display());

    info!(
        "Accepted gag attempt {attempt} with fun score {}: {}",
        fun.score, fun.reason
    );
    Ok(Some((gag, fun)))
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
    config::setup_logging(args.debug).context("Failed to initialize logging")?;
    let animal = args.animal.to_ascii_lowercase();

    info!(
        "Starting image generator: animal={}, code_arg={:?}, text_model={}, image_model={}, quality={}, debug={}, max_attempts={}",
        animal,
        args.code,
        args.text_model,
        args.image_model,
        args.quality.as_images_quality(),
        args.debug,
        args.max_attempts
    );
    info!(
        "Timeouts: responses={}s (retries={}, backoff={}ms), images={}s",
        args.responses_timeout_secs,
        args.responses_retries,
        args.responses_backoff_ms,
        args.images_timeout_secs
    );

    let status_code = match args.code {
        Some(c) => c,
        None => {
            info!("No code provided; scanning for next missing status code");
            let codes = load_status_codes()?;
            let existing = existing_codes_for(&animal, &args.out_dir)?;
            info!("Found {} existing images for {}", existing.len(), animal);
            let next = codes.into_iter().find(|c| !existing.contains(c));
            let Some(code) = next else {
                return Err(anyhow!("No missing status codes found for {animal}"));
            };
            info!("Next missing status code appears to be {code}");
            if !confirm_next_code(&animal, code)? {
                return Err(anyhow!("Aborted"));
            }
            code
        }
    };
    let debug_prefix = init_debug_prefix(&animal, status_code);
    info!("Debug prefix set to {debug_prefix}");

    let output_filename = args
        .out_dir
        .join(&animal)
        .join(format!("{status_code}.png"));
    info!("Output file will be {}", output_filename.display());
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
    let status = status_context(status_code);
    info!(
        "Status context: code={}, name=\"{}\" summary=\"{}\"",
        status_code, status.name, status.summary
    );

    info!(
        "Generating: animal={animal}, code={status_code}, tone={}, text_model={}, image_model={}, out={}",
        tone_label(tone),
        args.text_model,
        args.image_model,
        output_filename.display()
    );

    let request_timeout_secs =
        std::cmp::max(args.responses_timeout_secs, args.images_timeout_secs) + 15;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(request_timeout_secs))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .context("Failed to generate reqwest client")?;

    // Stage 1 + 1.5 + 1.6:
    // Generate multiple gag candidates, reject semantically-wrong ones, then pick the funniest.
    let mut accepted: Vec<(GagSpec, FunScore)> = Vec::new();

    // Aim for at least 3 candidates for variety.
    let target_candidates = std::cmp::max(3, args.max_attempts);
    info!("Target gag attempts: {target_candidates}");

    let mut join_set = JoinSet::new();
    for attempt in 1..=target_candidates {
        let args = args.clone();
        let client = client.clone();
        let animal = animal.clone();
        let status = status.clone();
        join_set.spawn(gag_attempt_pipeline(
            attempt,
            target_candidates,
            args,
            client,
            animal,
            status,
            status_code,
            tone,
        ));
    }

    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok(Some(item))) => accepted.push(item),
            Ok(Ok(None)) => {}
            Ok(Err(err)) => {
                join_set.abort_all();
                return Err(err);
            }
            Err(err) => {
                join_set.abort_all();
                return Err(anyhow!("Gag attempt task failed: {err}"));
            }
        }
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

    info!("Selected gag with fun score {}.", best_fun.score);

    // Stage 2: compile the final image prompt once from the chosen gag.
    let (prompt_spec, prompt_raw) = with_retries(
        "Prompt compilation",
        Duration::from_secs(args.responses_timeout_secs),
        args.responses_retries,
        Duration::from_millis(args.responses_backoff_ms),
        || {
            compile_prompt(
                &args,
                &client,
                UserInput {
                    animal: &animal,
                    status_code,
                    status_name: &status.name,
                    status_summary: &status.summary,
                    tone,
                    gag: &gag,
                },
            )
        },
    )
    .await?;

    let compiled_prompt_json = debug_file_path("compiled_prompt", "json")?;
    fs::write(&compiled_prompt_json, &prompt_raw)
        .with_context(|| format!("Failed to write {}", compiled_prompt_json.display()))?;
    let compiled_prompt_txt = debug_file_path("compiled_prompt", "txt")?;
    fs::write(&compiled_prompt_txt, &prompt_spec.prompt)
        .with_context(|| format!("Failed to write {}", compiled_prompt_txt.display()))?;
    info!(
        "Wrote compiled prompt debug files: {} and {}",
        compiled_prompt_json.display(),
        compiled_prompt_txt.display()
    );
    debug!("Prompt length: {}", prompt_spec.prompt.len());

    let prompt = prompt_spec.prompt;

    // Stage 3: render
    let png_bytes = with_timeout(
        "Image generation",
        Duration::from_secs(args.images_timeout_secs),
        generate_image(&args, &client, &prompt),
    )
    .await?;

    fs::write(&output_filename, &png_bytes)
        .with_context(|| format!("Failed to write image to {}", output_filename.display()))?;

    info!("Saved: {}", output_filename.display());

    // Store the gag spec for later auditing
    let meta_path = debug_file_path("gag", "json")?;
    fs::write(
        &meta_path,
        serde_json::to_string_pretty(&gag).unwrap_or_default(),
    )
    .with_context(|| format!("Failed to write {}", meta_path.display()))?;
    info!("Wrote gag metadata to {}", meta_path.display());

    Ok(())
}
